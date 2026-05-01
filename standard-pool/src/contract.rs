//! Standard-pool entry points: instantiate, execute dispatch, reply,
//! migrate.
//!
//! Query dispatch lives in `crate::query`. The `reply` entry point
//! handles a single id today — `DEPOSIT_VERIFY_REPLY_ID` — used by
//! the SubMsg-based CW20 balance verification on deposits and
//! add-to-position. Position-NFT ownership is still accepted lazily on
//! the first deposit via `pool_state.nft_ownership_accepted`; no
//! separate reply id is needed for that path.

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, MigrateMsg};
use cosmwasm_std::{
    entry_point, Addr, Decimal, DepsMut, Env, MessageInfo, Response, StdError, StdResult, Storage,
    Uint128,
};
use cw2::set_contract_version;
use pool_core::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw, execute_emergency_withdraw_core_drain,
    execute_emergency_withdraw_initiate, execute_pause, execute_unpause,
    execute_update_config_from_factory,
};
use pool_core::asset::{PoolPairType, TokenInfoPoolExt, TokenType};
use pool_core::balance_verify::handle_deposit_verify_reply;
use pool_core::liquidity::{
    execute_add_to_position_with_verify, execute_collect_fees,
    execute_deposit_liquidity_with_verify, execute_remove_all_liquidity,
    execute_remove_partial_liquidity, execute_remove_partial_liquidity_by_percent,
};
use pool_core::state::DEPOSIT_VERIFY_REPLY_ID;
use pool_core::msg::CommitFeeInfo;
use pool_core::state::{
    ExpectedFactory, OracleInfo, PoolAnalytics, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs,
    PoolState, Position, COMMITFEEINFO, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID, ORACLE_INFO, OWNER_POSITIONS, PENDING_EMERGENCY_WITHDRAW, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE,
};
use pool_core::swap::{execute_swap_cw20, simple_swap};
use pool_factory_interfaces::StandardPoolInstantiateMsg;

/// cw2 contract name written at instantiate / migrate; identifies this binary in on-chain version metadata.
const CONTRACT_NAME: &str = "bluechip-contracts-standard-pool";
/// cw2 contract version (sourced from Cargo.toml); compared against the stored value in `migrate` to reject downgrades.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Instantiate
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: StandardPoolInstantiateMsg,
) -> Result<Response, ContractError> {
    let cfg = ExpectedFactory {
        expected_factory_address: msg.used_factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;
    if info.sender != cfg.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    // Pair validation — each side must be a valid TokenType and the two
    // sides must differ. Defense-in-depth: the factory already validates
    // this, but rejecting again keeps the pool self-defending against a
    // buggy factory migration.
    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;
    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
    }
    for t in msg.pool_token_info.iter() {
        if let TokenType::Native { denom } = t {
            if denom.trim().is_empty() {
                return Err(ContractError::Std(StdError::generic_err(
                    "Standard pool: Native denom must be non-empty",
                )));
            }
        }
    }

    // `PoolInfo.token_address` is a legacy commit-pool field — its
    // semantic meaning ("address of the freshly-minted creator CW20")
    // doesn't apply to standard pools, which wrap pre-existing assets
    // and never mint a new CW20. Shared liquidity and swap code in
    // pool-core dispatches per-TokenType on `asset_infos[i]` and
    // doesn't read this field, so it's a value-only placeholder.
    //
    // Keep the wire-format type stable as `Addr` (avoids a state
    // migration on every existing pool record), but choose the
    // placeholder value carefully:
    //   - if any side is a CreatorToken, use that side's CW20 address
    //     (matches creator-pool's convention; lets external indexers
    //     that historically read this field still resolve to a real
    //     CW20 on Native+CW20 standard pools).
    //   - otherwise (Native+Native shapes such as the ATOM/bluechip
    //     anchor), set it to the pool's OWN contract address. The
    //     pool's address is always valid bech32, can never be confused
    //     with a creator CW20 (it isn't one), and is self-referential
    //     enough to make "this field is a placeholder, not a token
    //     address" obvious to any future reader.
    let token_address_placeholder = msg
        .pool_token_info
        .iter()
        .find_map(|t| match t {
            TokenType::CreatorToken { contract_addr } => Some(contract_addr.clone()),
            _ => None,
        })
        .unwrap_or_else(|| env.contract.address.clone());

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pool_info: PoolDetails {
            contract_addr: env.contract.address.clone(),
            asset_infos: msg.pool_token_info.clone(),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: msg.used_factory_addr.clone(),
        token_address: token_address_placeholder,
        position_nft_address: msg.position_nft_address.clone(),
    };

    // Placeholder position at id "0" so iteration/pagination over
    // LIQUIDITY_POSITIONS behaves the same as creator-pool. The first
    // real LP position lands at id "1" because NEXT_POSITION_ID
    // increments before use in `execute_deposit_liquidity`.
    let liquidity_position = Position {
        liquidity: Uint128::zero(),
        owner: env.contract.address.clone(),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier: Decimal::one(),
        unclaimed_fees_0: Uint128::zero(),
        unclaimed_fees_1: Uint128::zero(),
        // Sentinel position at id "0" — no actual liquidity, no lock.
        locked_liquidity: Uint128::zero(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3), // 0.3% LP fee
        min_commit_interval: 13,      // seconds; used by swap rate limit
    };

    // Zero-valued fee placeholder. Two reasons we save it:
    //   - emergency_withdraw_core_drain reads `bluechip_wallet_address`
    //     as the drain recipient. It MUST be a wallet (not the factory
    //     contract) — the factory has no withdrawal path, so funds sent
    //     there are permanently locked.
    //   - query_fee_info dereferences COMMITFEEINFO unconditionally.
    // The fee rates are zero on a standard pool, so the creator wallet
    // is never paid out in normal flow; we still store the factory's
    // configured bluechip wallet there as a safe placeholder.
    let fee_info = CommitFeeInfo {
        bluechip_wallet_address: msg.bluechip_wallet_address.clone(),
        creator_wallet_address: msg.bluechip_wallet_address.clone(),
        commit_fee_bluechip: Decimal::zero(),
        commit_fee_creator: Decimal::zero(),
    };

    // nft_ownership_accepted starts false; shared execute_deposit_liquidity
    // sends the Cw721 AcceptOwnership message on the first deposit and
    // flips this flag. No reply handler needed on standard-pool.
    let pool_state = PoolState {
        pool_contract_address: env.contract.address.clone(),
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(),
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        nft_ownership_accepted: false,
    };

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.used_factory_addr.clone(),
    };

    COMMITFEEINFO.save(deps.storage, &fee_info)?;
    // Standard pools are "threshold-hit" from birth — shared swap and
    // liquidity handlers gate on IS_THRESHOLD_HIT so this flips it open
    // for the first caller.
    IS_THRESHOLD_HIT.save(deps.storage, &true)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    OWNER_POSITIONS.save(deps.storage, (&env.contract.address, "0"), &true)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    POOL_ANALYTICS.save(deps.storage, &PoolAnalytics::default())?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool_kind", "standard")
        .add_attribute("pool", env.contract.address.to_string()))
}

// ---------------------------------------------------------------------------
// Execute dispatch
// ---------------------------------------------------------------------------

/// Liquidity-write gate: every deposit / add / remove / collect path must
/// fail closed when an emergency drain has been kicked off OR when an
/// admin has paused the pool. Inlining this pair was correct but copy-
/// pasted into every gated arm of `execute`; centralising it keeps the
/// behaviour identical and the dispatch arms shorter.
fn check_pool_writable(storage: &dyn Storage) -> Result<(), ContractError> {
    ensure_not_drained(storage)?;
    if POOL_PAUSED.may_load(storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    Ok(())
}

/// Deposit-side gate: same semantics as creator-pool's
/// `check_pool_writable_for_deposit`. Rejects admin / emergency hard
/// pauses but accepts auto-pause-on-low-liquidity so deposits can
/// restore reserves.
fn check_pool_writable_for_deposit(storage: &dyn Storage) -> Result<(), ContractError> {
    use pool_core::state::{pause_kind, PauseKind};
    ensure_not_drained(storage)?;
    match pause_kind(storage)? {
        PauseKind::None | PauseKind::AutoLowLiquidity => Ok(()),
        PauseKind::Hard => Err(ContractError::PoolPausedLowLiquidity {}),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::Receive(cw20_msg) => execute_swap_cw20(deps, env, info, cw20_msg),
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        } => {
            offer_asset.confirm_sent_native_balance(&info)?;
            let sender = info.sender.clone();
            let to_addr: Option<Addr> = to
                .map(|s| deps.api.addr_validate(&s))
                .transpose()?;
            simple_swap(
                deps,
                env,
                info,
                sender,
                offer_asset,
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        ExecuteMsg::UpdateConfigFromFactory { update } => {
            execute_update_config_from_factory(deps, env, info, update)
        }
        ExecuteMsg::Pause {} => execute_pause(deps, env, info),
        ExecuteMsg::Unpause {} => execute_unpause(deps, env, info),
        ExecuteMsg::EmergencyWithdraw {} => execute_emergency_withdraw(deps, env, info),
        ExecuteMsg::CancelEmergencyWithdraw {} => {
            execute_cancel_emergency_withdraw(deps, env, info)
        }
        ExecuteMsg::AcceptNftOwnership {} => execute_accept_nft_ownership(deps, info),
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            // Deposit-side gate permits auto-pause so reserves can
            // be restored. Hard pauses still reject.
            check_pool_writable_for_deposit(deps.storage)?;
            let sender = info.sender.clone();
            // Standard pools wrap arbitrary CW20s, so we route every
            // deposit through the SubMsg-based balance verification path.
            // The reply lands in `reply()` below, where a fee-on-transfer /
            // rebasing CW20 mismatch reverts the entire transaction.
            execute_deposit_liquidity_with_verify(
                deps,
                env,
                info,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            check_pool_writable_for_deposit(deps.storage)?;
            let sender = info.sender.clone();
            // Same SubMsg verify path as DepositLiquidity above.
            execute_add_to_position_with_verify(
                deps,
                env,
                info,
                position_id,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        ExecuteMsg::CollectFees { position_id } => {
            check_pool_writable(deps.storage)?;
            execute_collect_fees(deps, env, info, position_id)
        }
        ExecuteMsg::RemovePartialLiquidity {
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            // Block during admin pause / pending emergency withdraw so LPs
            // can't race the drain (matches creator-pool's behavior).
            check_pool_writable(deps.storage)?;
            execute_remove_partial_liquidity(
                deps,
                env,
                info,
                position_id,
                liquidity_to_remove,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            check_pool_writable(deps.storage)?;
            execute_remove_partial_liquidity_by_percent(
                deps,
                env,
                info,
                position_id,
                percentage,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            check_pool_writable(deps.storage)?;
            execute_remove_all_liquidity(
                deps,
                env,
                info,
                position_id,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
    }
}

/// Accept NFT ownership in the same transaction as pool creation.
///
/// Background: the factory's `finalize_standard_pool` reply chain
/// already dispatches `Cw721ExecuteMsg::TransferOwnership { new_owner:
/// pool }` to the position-NFT contract. cw_ownable's two-step transfer
/// pattern leaves the pool as `pending_owner` until the pool itself
/// sends `AcceptOwnership`. Without this handler, acceptance would be
/// deferred until the first user deposit (`pool_state.nft_ownership_accepted`
/// gate), opening a "pending-ownership window" between pool creation
/// and first deposit during which the NFT contract's owner is still
/// the factory.
///
/// The factory appends an `AcceptNftOwnership {}` execute call to the
/// pool to its finalize response, immediately after the TransferOwnership
/// to the NFT. This handler then sends the matching AcceptOwnership
/// message back to the NFT and flips the flag. Pool-core's deposit
/// handler still has its `if !nft_ownership_accepted` branch as a
/// backstop for any path that bypasses this trigger (e.g. test
/// fixtures), so the lazy-accept behavior remains available as a
/// fallback.
///
/// Idempotent: returns Ok with no messages if the flag is already set.
/// Authorisation: sender must equal `POOL_INFO.factory_addr` — only
/// the factory has any reason to invoke this, and accepting from any
/// other sender would let an attacker race the legitimate flow.
fn execute_accept_nft_ownership(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    if pool_state.nft_ownership_accepted {
        // Already accepted (likely via the deposit-side fallback in
        // pool-core). Don't dispatch a second AcceptOwnership — the
        // NFT contract would reject it with NoPendingOwner, failing
        // the entire create-pool transaction.
        return Ok(Response::new()
            .add_attribute("action", "accept_nft_ownership_noop")
            .add_attribute("pool", pool_info.pool_info.contract_addr.to_string()));
    }

    let accept_msg = cosmwasm_std::WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(),
        msg: cosmwasm_std::to_json_binary(
            &pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::<()>::UpdateOwnership(
                pool_factory_interfaces::cw721_msgs::Action::AcceptOwnership,
            ),
        )?,
        funds: vec![],
    };
    pool_state.nft_ownership_accepted = true;
    POOL_STATE.save(deps.storage, &pool_state)?;

    Ok(Response::new()
        .add_message(cosmwasm_std::CosmosMsg::Wasm(accept_msg))
        .add_attribute("action", "accept_nft_ownership")
        .add_attribute("pool", pool_info.pool_info.contract_addr.to_string())
        .add_attribute("nft", pool_info.position_nft_address.to_string()))
}

/// Standard-pool emergency withdraw: no commit-only bookkeeping. Dispatches
/// directly to the pool-core Phase 1 / Phase 2 handlers with zero
/// accumulation_drain amounts (no CREATOR_EXCESS_POSITION to sweep, no
/// DISTRIBUTION_STATE to halt).
fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_none() {
        return execute_emergency_withdraw_initiate(deps, env, info);
    }
    let drain = execute_emergency_withdraw_core_drain(
        deps,
        env.clone(),
        info,
        Uint128::zero(),
        Uint128::zero(),
    )?;
    Ok(Response::new()
        .add_messages(drain.messages)
        .add_attribute("action", "emergency_withdraw")
        .add_attribute("recipient", drain.recipient)
        .add_attribute("amount0", drain.total_0)
        .add_attribute("amount1", drain.total_1)
        .add_attribute("total_liquidity", drain.total_liquidity_at_withdrawal)
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Reply
// ---------------------------------------------------------------------------

/// Only one reply id is wired today — `DEPOSIT_VERIFY_REPLY_ID`,
/// which the verify-aware deposit / add-to-position paths emit on their
/// final SubMsg. The handler queries each CW20 side's post-balance and
/// asserts the delta matches the credited amount. A mismatch returns an
/// `Err` here, which propagates back to the chain and rolls the entire
/// transaction back — including all the parent's state writes (position
/// save, NFT mint, reserve update, etc.). On match, returns Ok and the
/// transaction commits normally.
///
/// Any unknown reply id is rejected rather than silently dropped:
/// nothing else in standard-pool currently dispatches `SubMsg::reply_*`,
/// so an unknown id signals either a forgotten dispatch site or an
/// upstream `pool-core` upgrade that introduced a new reply path
/// without wiring it here.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, env: Env, msg: cosmwasm_std::Reply) -> StdResult<Response> {
    match msg.id {
        DEPOSIT_VERIFY_REPLY_ID => handle_deposit_verify_reply(deps, env, msg)
            .map_err(|e| StdError::generic_err(e.to_string())),
        other => Err(StdError::generic_err(format!(
            "standard-pool reply: unknown reply id {}",
            other
        ))),
    }
}

// ---------------------------------------------------------------------------
// Migrate
// ---------------------------------------------------------------------------

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, msg: MigrateMsg) -> StdResult<Response> {
    // Reject downgrades. Mirrors the creator-pool migrate guard —
    // see that handler for the rationale. Tolerates a missing cw2 entry
    // (legacy pre-cw2 / test fixtures) by skipping the check; production
    // pools always set cw2 at instantiate time.
    if let Ok(stored_version) = cw2::get_contract_version(deps.storage) {
        let stored_semver: semver::Version = stored_version.version.parse().map_err(|e| {
            StdError::generic_err(format!(
                "stored contract version {} is not valid semver: {}",
                stored_version.version, e
            ))
        })?;
        let current_semver: semver::Version = CONTRACT_VERSION.parse().map_err(|e| {
            StdError::generic_err(format!(
                "current contract version {} is not valid semver: {}",
                CONTRACT_VERSION, e
            ))
        })?;
        if stored_semver > current_semver {
            return Err(StdError::generic_err(format!(
                "Migration would downgrade contract from {} to {}; refusing.",
                stored_semver, current_semver
            )));
        }
    }

    match msg {
        MigrateMsg::UpdateFees { new_fees } => {
            let max_lp_fee = Decimal::percent(10);
            if new_fees > max_lp_fee {
                return Err(StdError::generic_err("lp_fee must not exceed 10% (0.1)"));
            }
            let min_lp_fee = Decimal::permille(1); // 0.1%
            if new_fees < min_lp_fee {
                return Err(StdError::generic_err(
                    "lp_fee must be at least 0.1% (0.001)",
                ));
            }
            POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
                specs.lp_fee = new_fees;
                Ok(specs)
            })?;
        }
        MigrateMsg::UpdateVersion {} => {}
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("version", CONTRACT_VERSION))
}
