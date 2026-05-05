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
    entry_point, Addr, Decimal, DepsMut, Env, MessageInfo, Response, StdError, Storage, Uint128,
};
use cw2::set_contract_version;
use pool_core::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw, execute_emergency_withdraw_dispatch,
    execute_pause, execute_unpause, execute_update_config_from_factory,
};
use pool_core::asset::{PoolPairType, TokenInfoPoolExt, TokenType};
use pool_core::balance_verify::handle_deposit_verify_reply;
use pool_core::generic::unknown_reply_id_msg;
use pool_core::liquidity::{
    execute_add_to_position_with_verify, execute_collect_fees,
    execute_deposit_liquidity_with_verify, execute_remove_all_liquidity,
    execute_remove_partial_liquidity, execute_remove_partial_liquidity_by_percent,
};
use pool_core::msg::CommitFeeInfo;
use pool_core::state::{
    ExpectedFactory, OracleInfo, PoolAnalytics, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs,
    PoolState, Position, COMMITFEEINFO, DEFAULT_LP_FEE, DEFAULT_SWAP_RATE_LIMIT_SECS,
    DEPOSIT_VERIFY_REPLY_ID, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS, MAX_LP_FEE,
    MIN_LP_FEE, NEXT_POSITION_ID, ORACLE_INFO, OWNER_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE,
    POOL_INFO, POOL_KIND_STANDARD, POOL_SPECS, POOL_STATE,
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

    // Pair validation — each side must be a valid TokenType (the
    // empty-denom guard for `Native` and the `addr_validate` for
    // `CreatorToken` both live inside `TokenType::check` now) and the
    // two sides must differ. Defense-in-depth: the factory already
    // validates this, but rejecting again keeps the pool
    // self-defending against a buggy factory migration.
    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;
    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
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

    let liquidity_position = build_sentinel_position(&env);

    let pool_specs = PoolSpecs {
        lp_fee: DEFAULT_LP_FEE,
        min_commit_interval: DEFAULT_SWAP_RATE_LIMIT_SECS,
    };

    let fee_info = build_zero_fee_info(&msg.bluechip_wallet_address);
    let pool_state = build_initial_pool_state(&env);
    let pool_fee_state = build_zero_pool_fee_state();

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
        .add_attribute("pool_kind", POOL_KIND_STANDARD)
        .add_attribute("pool", env.contract.address.to_string()))
}

// ---------------------------------------------------------------------------
// Instantiate-only state builders. Extracted from the entry point so
// the dispatcher reads as a sequence of named saves; each builder owns
// the zero-init for one struct.
// ---------------------------------------------------------------------------

/// Sentinel position at id "0" — no actual liquidity, no lock. Saved
/// so iteration / pagination over `LIQUIDITY_POSITIONS` behaves the
/// same as creator-pool. The first real LP position lands at id "1"
/// because `NEXT_POSITION_ID` increments before use.
fn build_sentinel_position(env: &Env) -> Position {
    Position {
        liquidity: Uint128::zero(),
        owner: env.contract.address.clone(),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier: Decimal::one(),
        unclaimed_fees_0: Uint128::zero(),
        unclaimed_fees_1: Uint128::zero(),
        locked_liquidity: Uint128::zero(),
    }
}

/// Zero-valued fee placeholder. Two reasons we save it:
///   - `emergency_withdraw_core_drain` reads `bluechip_wallet_address`
///     as the drain recipient. It MUST be a wallet (not the factory
///     contract) — the factory has no withdrawal path, so funds sent
///     there are permanently locked.
///   - `query_fee_info` dereferences `COMMITFEEINFO` unconditionally.
/// The fee rates are zero on a standard pool, so the creator wallet
/// is never paid out in normal flow; we still store the factory's
/// configured bluechip wallet there as a safe placeholder.
fn build_zero_fee_info(bluechip_wallet: &Addr) -> CommitFeeInfo {
    CommitFeeInfo {
        bluechip_wallet_address: bluechip_wallet.clone(),
        creator_wallet_address: bluechip_wallet.clone(),
        commit_fee_bluechip: Decimal::zero(),
        commit_fee_creator: Decimal::zero(),
    }
}

/// `nft_ownership_accepted` starts false; the standard-pool
/// `AcceptNftOwnership` execute message (dispatched by the factory's
/// finalize chain) flips it true and dispatches the matching CW721
/// `AcceptOwnership` message back.
fn build_initial_pool_state(env: &Env) -> PoolState {
    PoolState {
        pool_contract_address: env.contract.address.clone(),
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(),
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        nft_ownership_accepted: false,
    }
}

/// Zero-init `PoolFeeState`. All four globals are `Decimal::zero()` and
/// all four accumulators are `Uint128::zero()`.
fn build_zero_pool_fee_state() -> PoolFeeState {
    PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    }
}

// ---------------------------------------------------------------------------
// Execute dispatch
// ---------------------------------------------------------------------------

/// Deposit-side gate: same semantics as creator-pool's
/// `check_pool_writable_for_deposit`. Rejects admin / emergency hard
/// pauses but accepts auto-pause-on-low-liquidity so deposits can
/// restore reserves. EmergencyPending also rejects — letting fresh
/// LP capital deposit into a soon-to-be-drained pool would funnel new
/// money straight to the emergency-drain recipient.
fn check_pool_writable_for_deposit(storage: &dyn Storage) -> Result<(), ContractError> {
    use pool_core::state::{pause_kind, PauseKind};
    ensure_not_drained(storage)?;
    match pause_kind(storage)? {
        PauseKind::None | PauseKind::AutoLowLiquidity => Ok(()),
        PauseKind::EmergencyPending | PauseKind::Hard => {
            Err(ContractError::PoolPausedLowLiquidity {})
        }
    }
}

/// LP-exit gate: permits `Remove*Liquidity` and `CollectFees` while
/// the pool is open OR while it's in the emergency-withdraw timelock
/// window (PauseKind::EmergencyPending). Auto-pause-on-low-liquidity
/// still rejects (the recovery path is "deposit to restore", not
/// "remove the last reserves"); explicit admin Hard pause still
/// rejects.
///
/// Allowing exits during EmergencyPending closes the LP-trap window
/// surfaced by the audit (HIGH-1): without this, LPs whose pool gets
/// emergency-withdrawn cannot withdraw their principal during the 24h
/// timelock and lose 100% on Phase-2 drain.
fn check_pool_writable_for_remove(storage: &dyn Storage) -> Result<(), ContractError> {
    use pool_core::state::{pause_kind, PauseKind};
    ensure_not_drained(storage)?;
    match pause_kind(storage)? {
        PauseKind::None | PauseKind::EmergencyPending => Ok(()),
        PauseKind::AutoLowLiquidity | PauseKind::Hard => {
            Err(ContractError::PoolPausedLowLiquidity {})
        }
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
            allow_high_max_spread,
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
                allow_high_max_spread,
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
            // Permitted during EmergencyPending so an LP about to remove
            // can sweep their share of fee_reserve before the drain.
            check_pool_writable_for_remove(deps.storage)?;
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
            // Permit removes during EmergencyPending so LPs can exit
            // during the 24h timelock rather than being confiscated on
            // Phase-2 drain. Hard pause + auto-pause + drained still
            // reject.
            check_pool_writable_for_remove(deps.storage)?;
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
            check_pool_writable_for_remove(deps.storage)?;
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
            check_pool_writable_for_remove(deps.storage)?;
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

/// Standard-pool emergency withdraw: no commit-only bookkeeping.
/// Routes directly through the shared pool-core two-phase dispatcher
/// with zero `accumulation_drain` amounts (no `CREATOR_EXCESS_POSITION`
/// to sweep, no `DISTRIBUTION_STATE` to halt).
fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    execute_emergency_withdraw_dispatch(deps, env, info, Uint128::zero(), Uint128::zero())
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
pub fn reply(
    deps: DepsMut,
    env: Env,
    msg: cosmwasm_std::Reply,
) -> Result<Response, ContractError> {
    match msg.id {
        // The verify handler returns `ContractError`; previously it was
        // `.to_string()`'d into a `StdError::generic_err`, erasing the
        // structured variant on the wire (operators monitoring for the
        // `BalanceMismatch` case — the exact one most worth alerting on
        // for fee-on-transfer / rebasing CW20 mismatches — could no
        // longer match it). With reply now returning the typed
        // `ContractError`, the variant is preserved end-to-end.
        DEPOSIT_VERIFY_REPLY_ID => handle_deposit_verify_reply(deps, env, msg),
        other => Err(ContractError::Std(StdError::generic_err(
            unknown_reply_id_msg(POOL_KIND_STANDARD, other),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Migrate
// ---------------------------------------------------------------------------

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, msg: MigrateMsg) -> Result<Response, ContractError> {
    // Reject downgrades. Mirrors the creator-pool migrate guard —
    // see that handler for the rationale. Tolerates a missing cw2 entry
    // (legacy pre-cw2 / test fixtures) by skipping the check; production
    // pools always set cw2 at instantiate time.
    if let Ok(stored_version) = cw2::get_contract_version(deps.storage) {
        let stored_semver: semver::Version = stored_version
            .version
            .parse()
            .map_err(|e: semver::Error| ContractError::StoredVersionInvalid {
                version: stored_version.version.clone(),
                msg: e.to_string(),
            })?;
        let current_semver: semver::Version = CONTRACT_VERSION
            .parse()
            .map_err(|e: semver::Error| ContractError::CurrentVersionInvalid {
                version: CONTRACT_VERSION.to_string(),
                msg: e.to_string(),
            })?;
        if stored_semver > current_semver {
            return Err(ContractError::DowngradeRefused {
                stored: stored_semver.to_string(),
                current: current_semver.to_string(),
            });
        }
    }

    match msg {
        MigrateMsg::UpdateFees { new_fees } => {
            if new_fees > MAX_LP_FEE || new_fees < MIN_LP_FEE {
                return Err(ContractError::LpFeeOutOfRange {
                    got: new_fees,
                    min: MIN_LP_FEE,
                    max: MAX_LP_FEE,
                });
            }
            POOL_SPECS.update(deps.storage, |mut specs| -> Result<_, ContractError> {
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
