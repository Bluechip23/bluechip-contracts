//! Standard-pool entry points: instantiate, execute dispatch, migrate.
//!
//! Query dispatch lives in `crate::query`. No `reply` entry point —
//! position-NFT ownership is accepted lazily on the first deposit via
//! `pool_state.nft_ownership_accepted` (Option X from the 4b-ii design
//! review).

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
use pool_core::liquidity::{
    execute_add_to_position, execute_collect_fees, execute_deposit_liquidity,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent,
};
use pool_core::msg::CommitFeeInfo;
use pool_core::state::{
    ExpectedFactory, OracleInfo, PoolAnalytics, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs,
    PoolState, Position, COMMITFEEINFO, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID, ORACLE_INFO, OWNER_POSITIONS, PENDING_EMERGENCY_WITHDRAW, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE,
};
use pool_core::swap::{execute_swap_cw20, simple_swap};
use pool_factory_interfaces::StandardPoolInstantiateMsg;

const CONTRACT_NAME: &str = "bluechip-contracts-standard-pool";
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

    // `PoolInfo.token_address` is a legacy commit-pool field. Standard
    // pools populate it with the first CreatorToken side if any, else
    // the factory address as a harmless placeholder. Shared liquidity
    // and swap code dispatches per-TokenType on `asset_infos[i]` and
    // doesn't read this field.
    let token_address_placeholder = msg
        .pool_token_info
        .iter()
        .find_map(|t| match t {
            TokenType::CreatorToken { contract_addr } => Some(contract_addr.clone()),
            _ => None,
        })
        .unwrap_or_else(|| msg.used_factory_addr.clone());

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

    // Zero-valued placeholder. Two reasons we save it:
    //   - emergency_withdraw_core_drain reads `bluechip_wallet_address`
    //     as the drain recipient.
    //   - query_fee_info dereferences COMMITFEEINFO unconditionally.
    // Factory address is a safe default for both wallet fields — fees
    // are always zero on a standard pool, so no live flow depends on
    // these values.
    let fee_info = CommitFeeInfo {
        bluechip_wallet_address: msg.used_factory_addr.clone(),
        creator_wallet_address: msg.used_factory_addr.clone(),
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
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            check_pool_writable(deps.storage)?;
            let sender = info.sender.clone();
            execute_deposit_liquidity(
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
            check_pool_writable(deps.storage)?;
            let sender = info.sender.clone();
            execute_add_to_position(
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
// Migrate
// ---------------------------------------------------------------------------

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, msg: MigrateMsg) -> StdResult<Response> {
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
