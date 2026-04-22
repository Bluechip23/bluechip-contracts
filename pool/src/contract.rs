//! Pool contract entry points: instantiate, execute dispatch, swap, migrate.
//!
//! Commit logic lives in [`crate::commit`], admin operations in [`crate::admin`].

use crate::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw, execute_emergency_withdraw,
    execute_pause, execute_recover_stuck_states, execute_unpause,
    execute_update_config_from_factory,
};
use crate::asset::{PoolPairType, TokenInfo, TokenInfoPoolExt, TokenType};
use crate::commit::{commit, execute_continue_distribution};
use crate::error::ContractError;
use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, update_pool_fee_growth,
    validate_pool_threshold_payments,
};
use crate::liquidity::{
    execute_add_to_position, execute_collect_fees, execute_deposit_liquidity,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent,
};
use crate::liquidity_helpers::{execute_claim_creator_excess, execute_claim_creator_fees};
use crate::msg::{
    CommitPoolInstantiateMsg, Cw20HookMsg, ExecuteMsg, MigrateMsg, PoolInstantiateMsg,
};
use pool_factory_interfaces::StandardPoolInstantiateMsg;
use crate::query::query_check_commit;
use crate::state::{
    CommitLimitInfo, ExpectedFactory, OracleInfo, PoolAnalytics, PoolCtx, PoolDetails,
    PoolFeeState, PoolInfo, PoolKind, PoolSpecs, Position, ThresholdPayoutAmounts, COMMITFEEINFO,
    COMMIT_LIMIT_INFO, EXPECTED_FACTORY, IS_THRESHOLD_HIT, MINIMUM_LIQUIDITY,
    NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID, ORACLE_INFO, OWNER_POSITIONS, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_KIND, POOL_PAUSED, POOL_SPECS, POOL_STATE, REENTRANCY_LOCK,
    THRESHOLD_PAYOUT_AMOUNTS, USD_RAISED_FROM_COMMIT,
};
use crate::state::{
    PoolState, LIQUIDITY_POSITIONS, PENDING_FACTORY_NOTIFY, REPLY_ID_FACTORY_NOTIFY_INITIAL,
    REPLY_ID_FACTORY_NOTIFY_RETRY,
};
use crate::swap_helper::{assert_max_spread, compute_swap, update_price_accumulator};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo,
    Reply, Response, StdError, StdResult, SubMsg, SubMsgResult, Uint128, WasmMsg,
};
use cw2::set_contract_version;
use cw20::Cw20ReceiveMsg;

pub const DEFAULT_SLIPPAGE: &str = "0.005";

const CONTRACT_NAME: &str = "bluechip-contracts-pool";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Instantiate
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    match msg {
        PoolInstantiateMsg::Commit(commit_msg) => instantiate_commit_pool(deps, env, info, commit_msg),
        PoolInstantiateMsg::Standard(standard_msg) => {
            instantiate_standard_pool(deps, env, info, standard_msg)
        }
    }
}

/// Commit-pool instantiate path (original behavior). Sets up every
/// commit-phase storage item (threshold payout, commit fees, commit limit,
/// etc.) and writes `PoolKind::Commit`.
fn instantiate_commit_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: CommitPoolInstantiateMsg,
) -> Result<Response, ContractError> {
    let cfg = ExpectedFactory {
        expected_factory_address: msg.used_factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;
    if info.sender != cfg.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;
    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
    }

    // Enforce the expected pair shape: exactly one Bluechip entry and
    // exactly one CreatorToken entry whose address matches the factory-
    // minted token. This is defense-in-depth — the factory already
    // validates the shape and rewrites the sentinel — but the pool
    // refuses to stand up unless the invariants actually hold here,
    // so a buggy factory migration or a directly-instantiated pool
    // (e.g. via a raw Wasm instantiate) can't silently produce a pool
    // whose reserve accounting disagrees with its holdings.
    let mut bluechip_count = 0usize;
    let mut creator_match = false;
    let mut creator_count = 0usize;
    for t in msg.pool_token_info.iter() {
        match t {
            TokenType::Bluechip { denom } => {
                if denom.trim().is_empty() {
                    return Err(ContractError::Std(StdError::generic_err(
                        "Bluechip denom must be non-empty",
                    )));
                }
                bluechip_count += 1;
            }
            TokenType::CreatorToken { contract_addr } => {
                creator_count += 1;
                if contract_addr == &msg.token_address {
                    creator_match = true;
                }
            }
        }
    }
    if bluechip_count != 1 || creator_count != 1 {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "pool_token_info must contain exactly one Bluechip and one CreatorToken (got {} Bluechip, {} CreatorToken)",
            bluechip_count, creator_count
        ))));
    }
    if !creator_match {
        return Err(ContractError::Std(StdError::generic_err(
            "CreatorToken.contract_addr in pool_token_info must equal msg.token_address",
        )));
    }
    if (msg.commit_fee_info.commit_fee_bluechip + msg.commit_fee_info.commit_fee_creator)
        > Decimal::one()
    {
        return Err(ContractError::InvalidFee {});
    }

    let is_standard_pool = msg.is_standard_pool.unwrap_or(false);

    let threshold_payout_amounts = if is_standard_pool {
        ThresholdPayoutAmounts {
            creator_reward_amount: Uint128::zero(),
            bluechip_reward_amount: Uint128::zero(),
            pool_seed_amount: Uint128::zero(),
            commit_return_amount: Uint128::zero(),
        }
    } else if let Some(params_binary) = msg.threshold_payout {
        let params: ThresholdPayoutAmounts = from_json(params_binary)?;
        validate_pool_threshold_payments(&params)?;
        params
    } else {
        return Err(ContractError::InvalidThresholdParams {
            msg: "Your params could not be validated during pool instantiation.".to_string(),
        });
    };

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pool_info: PoolDetails {
            contract_addr: env.contract.address.clone(),
            asset_infos: msg.pool_token_info.clone(),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: msg.used_factory_addr.clone(),
        token_address: msg.token_address.clone(),
        position_nft_address: msg.position_nft_address.clone(),
    };

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
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: msg.commit_threshold_limit_usd,
        commit_amount_for_threshold: msg.commit_amount_for_threshold,
        max_bluechip_lock_per_pool: msg.max_bluechip_lock_per_pool,
        creator_excess_liquidity_lock_days: msg.creator_excess_liquidity_lock_days,
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.used_factory_addr.clone(),
    };

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

    USD_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    COMMITFEEINFO.save(deps.storage, &msg.commit_fee_info)?;
    NATIVE_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    IS_THRESHOLD_HIT.save(deps.storage, &is_standard_pool)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT_AMOUNTS.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_LIMIT_INFO.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    OWNER_POSITIONS.save(deps.storage, (&env.contract.address, "0"), &true)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    POOL_ANALYTICS.save(deps.storage, &PoolAnalytics::default())?;
    // This instantiate path is the commit-pool path (dispatched from
    // ExecuteMsg::Create in the factory).
    POOL_KIND.save(deps.storage, &PoolKind::Commit)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool_kind", "commit")
        .add_attribute("pool", env.contract.address.to_string()))
}

/// Standard-pool instantiate path (new in H14). Wraps two pre-existing
/// assets as a plain xyk pool. Skips every commit-phase storage item —
/// no threshold payout, no commit fees, no commit limit — and writes
/// `PoolKind::Standard`.
///
/// Commit 3 scope restriction: this path only supports pools where the
/// pair is exactly one `TokenType::Bluechip` + one `TokenType::CreatorToken`,
/// because the existing deposit/swap/liquidity code paths assume that
/// layout (asset 0 native, asset 1 CW20). Bluechip/Bluechip (e.g. the
/// ATOM/bluechip anchor) and CW20/CW20 pairs are deferred to Commit 4,
/// which generalizes the asset-handling code.
fn instantiate_standard_pool(
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

    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;
    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
    }

    // H14 Commit 3 restriction: existing deposit/swap/liquidity code
    // assumes asset 0 is a native Bluechip denom and asset 1 is a
    // CreatorToken CW20. Reject any other pair shape until Commit 4
    // generalizes the asset-handling path. The factory-side validator
    // allows any shape, so this is the enforcing boundary during the
    // staged rollout.
    let bluechip_count = msg
        .pool_token_info
        .iter()
        .filter(|t| matches!(t, TokenType::Bluechip { .. }))
        .count();
    let creator_count = msg
        .pool_token_info
        .iter()
        .filter(|t| matches!(t, TokenType::CreatorToken { .. }))
        .count();
    if bluechip_count != 1 || creator_count != 1 {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Standard pool (H14 Commit 3 scope): pair must be exactly one Bluechip + one CreatorToken (got {} Bluechip, {} CreatorToken). CW20/CW20 and Bluechip/Bluechip pair support lands in Commit 4.",
            bluechip_count, creator_count
        ))));
    }
    if !matches!(msg.pool_token_info[0], TokenType::Bluechip { .. }) {
        return Err(ContractError::Std(StdError::generic_err(
            "Standard pool (H14 Commit 3 scope): pool_token_info[0] must be the Bluechip side. Commit 4 will lift this ordering requirement.",
        )));
    }
    let creator_token_addr =
        if let TokenType::CreatorToken { contract_addr } = &msg.pool_token_info[1] {
            contract_addr.clone()
        } else {
            return Err(ContractError::Std(StdError::generic_err(
                "Standard pool (H14 Commit 3 scope): pool_token_info[1] must be the CreatorToken side.",
            )));
        };

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pool_info: PoolDetails {
            contract_addr: env.contract.address.clone(),
            asset_infos: msg.pool_token_info.clone(),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: msg.used_factory_addr.clone(),
        token_address: creator_token_addr,
        position_nft_address: msg.position_nft_address.clone(),
    };

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
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3),   // 0.3% LP fee (same default as commit pools)
        min_commit_interval: 13,        // seconds; used by swap rate limit
        usd_payment_tolerance_bps: 0,   // unused for standard pools
    };

    // Standard pools have no creator wallet (no creator token mint, no
    // commit-fee destinations). We save a zeroed COMMITFEEINFO with the
    // factory address as a placeholder for both wallet fields so the
    // storage item exists — downstream code that reads it on the swap
    // path checks is_zero fees and no-ops. The factory address is a
    // safe placeholder because: (a) it won't accidentally forward fees
    // somewhere sensitive, and (b) fees here are always zero anyway.
    let fee_info = crate::msg::CommitFeeInfo {
        bluechip_wallet_address: msg.used_factory_addr.clone(),
        creator_wallet_address: msg.used_factory_addr.clone(),
        commit_fee_bluechip: Decimal::zero(),
        commit_fee_creator: Decimal::zero(),
    };

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

    // Zero-valued legacy commit state. Populated so downstream code that
    // unconditionally loads these items doesn't have to may_load-with-
    // default on every swap. Standard pools never write to or read the
    // commit-phase semantics of these values — the commit/continue_
    // distribution/claim_* handlers reject for standard pools anyway.
    let zeroed_payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::zero(),
        bluechip_reward_amount: Uint128::zero(),
        pool_seed_amount: Uint128::zero(),
        commit_return_amount: Uint128::zero(),
    };
    let zeroed_commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: Uint128::zero(),
        commit_amount_for_threshold: Uint128::zero(),
        max_bluechip_lock_per_pool: Uint128::zero(),
        creator_excess_liquidity_lock_days: 0,
    };
    let oracle_info = OracleInfo {
        oracle_addr: msg.used_factory_addr.clone(),
    };

    USD_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    COMMITFEEINFO.save(deps.storage, &fee_info)?;
    NATIVE_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    // Standard pools are "threshold-hit" from birth — that's how the
    // existing IS_THRESHOLD_HIT gate in liquidity/swap handlers permits
    // deposit and swap immediately.
    IS_THRESHOLD_HIT.save(deps.storage, &true)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT_AMOUNTS.save(deps.storage, &zeroed_payout)?;
    COMMIT_LIMIT_INFO.save(deps.storage, &zeroed_commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    OWNER_POSITIONS.save(deps.storage, (&env.contract.address, "0"), &true)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    POOL_ANALYTICS.save(deps.storage, &PoolAnalytics::default())?;
    POOL_KIND.save(deps.storage, &PoolKind::Standard)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool_kind", "standard")
        .add_attribute("pool", env.contract.address.to_string()))
}

// ---------------------------------------------------------------------------
// Execute dispatch
// ---------------------------------------------------------------------------

/// Reject execute messages that only make sense on commit pools. Applied
/// at the dispatch layer — handlers themselves don't have to re-check.
/// Uses `load_pool_kind` so pre-H14 pools (no POOL_KIND storage key)
/// correctly classify as commit pools.
fn require_commit_pool(
    storage: &dyn cosmwasm_std::Storage,
    action: &'static str,
) -> Result<(), ContractError> {
    if crate::state::load_pool_kind(storage)? != PoolKind::Commit {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "{} is only available on commit pools; this is a standard pool",
            action
        ))));
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
        // --- Admin ---
        ExecuteMsg::UpdateConfigFromFactory { update } => {
            execute_update_config_from_factory(deps, env, info, update)
        }
        ExecuteMsg::Pause {} => execute_pause(deps, env, info),
        ExecuteMsg::Unpause {} => execute_unpause(deps, env, info),
        ExecuteMsg::EmergencyWithdraw {} => execute_emergency_withdraw(deps, env, info),
        ExecuteMsg::CancelEmergencyWithdraw {} => {
            execute_cancel_emergency_withdraw(deps, env, info)
        }
        ExecuteMsg::RecoverStuckStates { recovery_type } => {
            execute_recover_stuck_states(deps, env, info, recovery_type)
        }

        // --- Commit & Distribution (commit-pool only) ---
        ExecuteMsg::Commit {
            asset,
            transaction_deadline,
            belief_price,
            max_spread,
        } => {
            require_commit_pool(deps.storage, "Commit")?;
            // Block ALL commits while paused — pre-threshold AND post-threshold.
            // Previously only process_post_threshold_commit checked POOL_PAUSED,
            // so admin pauses failed to stop pre-threshold deposits, letting
            // users trap funds in the COMMIT_LEDGER of a paused pool.
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
            commit(
                deps,
                env,
                info,
                asset,
                transaction_deadline,
                belief_price,
                max_spread,
            )
        }
        ExecuteMsg::ContinueDistribution {} => {
            require_commit_pool(deps.storage, "ContinueDistribution")?;
            execute_continue_distribution(deps, env, info)
        }

        // --- Swap ---
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        } => {
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            offer_asset.confirm_sent_bluechip_token_balance(&info)?;
            let sender_addr = info.sender.clone();
            let to_addr: Option<Addr> = to
                .map(|to_str| deps.api.addr_validate(&to_str))
                .transpose()?;
            simple_swap(
                deps,
                env,
                info,
                sender_addr,
                offer_asset,
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        ExecuteMsg::Receive(cw20_msg) => execute_swap_cw20(deps, env, info, cw20_msg),

        // --- Liquidity ---
        // Pause checks are now applied to EVERY liquidity-touching path.
        // Previously only CollectFees honored POOL_PAUSED; deposits and
        // removes could run unchecked while the pool was paused (e.g. mid
        // emergency-withdraw window), which could either funnel fresh LP
        // capital into a pending drain or let LPs race the drain. The
        // drain-initiated path already flips POOL_PAUSED on, so a single
        // check blocks both admin-pause and emergency-pending states.
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
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
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
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
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
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
            // Defense-in-depth: currently a drained pool has
            // total_liquidity == 0 so the math inside remove_partial_liquidity
            // would error out on its own. That's a coincidence, not a
            // guarantee. If a future partial-drain or recovery path ever
            // leaves non-zero total_liquidity after EMERGENCY_DRAINED is set,
            // an explicit check here keeps users from pulling against
            // already-swept reserves with arbitrary math.
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
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
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount1,
            min_amount0,
            max_ratio_deviation_bps,
        } => {
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
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
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
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
        ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline } => {
            // Creator excess exists only when a commit-pool threshold is crossed
            // with more raised-bluechip than `max_bluechip_lock_per_pool`
            // absorbs. Standard pools have no commit phase and no excess
            // position, so there's nothing to claim.
            require_commit_pool(deps.storage, "ClaimCreatorExcessLiquidity")?;
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
            execute_claim_creator_excess(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::ClaimCreatorFees { transaction_deadline } => {
            // The creator fee pot is seeded by the fee_size_multiplier
            // clip on commit-pool LP fees. Standard pools have no creator
            // concept, so the pot is always empty and this handler is N/A.
            require_commit_pool(deps.storage, "ClaimCreatorFees")?;
            ensure_not_drained(deps.storage)?;
            if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
                return Err(ContractError::PoolPausedLowLiquidity {});
            }
            execute_claim_creator_fees(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::RetryFactoryNotify {} => {
            // Retries NotifyThresholdCrossed to the factory. Standard
            // pools never cross a threshold, so there's nothing to retry.
            require_commit_pool(deps.storage, "RetryFactoryNotify")?;
            execute_retry_factory_notify(deps, env, info)
        }
    }
}

/// Re-sends `NotifyThresholdCrossed` to the factory when the initial
/// notification (dispatched via `reply_on_error` during threshold-crossing
/// commit) failed. This entrypoint is callable by ANYONE; the factory's
/// POOL_THRESHOLD_MINTED idempotency check prevents a successful mint from
/// firing twice. Reply handling clears PENDING_FACTORY_NOTIFY on success.
///
/// Why permissionless: recovery-path tx. If a factory misconfiguration or
/// expand-economy stall caused the initial notification to fail, we want
/// anyone — a keeper, a committer, Bluechip ops — to be able to nudge the
/// system back to consistent state once the root cause is fixed. The worst
/// an abusive caller can do is waste their own gas on a factory reject.
pub fn execute_retry_factory_notify(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
) -> Result<Response, ContractError> {
    let pending = PENDING_FACTORY_NOTIFY
        .may_load(deps.storage)?
        .unwrap_or(false);
    if !pending {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending factory notification to retry",
        )));
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let notify = SubMsg::reply_always(
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.factory_addr.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::FactoryExecuteMsg::NotifyThresholdCrossed {
                    pool_id: pool_info.pool_id,
                },
            )?,
            funds: vec![],
        }),
        REPLY_ID_FACTORY_NOTIFY_RETRY,
    );

    Ok(Response::new()
        .add_submessage(notify)
        .add_attribute("action", "retry_factory_notify")
        .add_attribute("pool_id", pool_info.pool_id.to_string()))
}

/// SubMsg reply handler.
///
/// Two reply IDs come through here:
///   - REPLY_ID_FACTORY_NOTIFY_INITIAL (from the threshold-crossing commit).
///     Fires only on error (reply_on_error). We set PENDING_FACTORY_NOTIFY
///     and return Ok so the parent commit tx survives.
///   - REPLY_ID_FACTORY_NOTIFY_RETRY (from execute_retry_factory_notify).
///     Fires always (reply_always). On success we clear the pending flag;
///     on failure we keep it so another retry can be attempted.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> StdResult<Response> {
    match msg.id {
        REPLY_ID_FACTORY_NOTIFY_INITIAL => {
            let err = match msg.result {
                SubMsgResult::Err(e) => e,
                // reply_on_error shouldn't produce Ok here; treat as no-op.
                SubMsgResult::Ok(_) => return Ok(Response::new()),
            };
            PENDING_FACTORY_NOTIFY.save(deps.storage, &true)?;
            Ok(Response::new()
                .add_attribute("action", "factory_notify_deferred")
                .add_attribute("reason", err)
                .add_attribute("block_time", env.block.time.seconds().to_string()))
        }
        REPLY_ID_FACTORY_NOTIFY_RETRY => match msg.result {
            SubMsgResult::Ok(_) => {
                PENDING_FACTORY_NOTIFY.save(deps.storage, &false)?;
                Ok(Response::new()
                    .add_attribute("action", "factory_notify_retry_succeeded")
                    .add_attribute("block_time", env.block.time.seconds().to_string()))
            }
            SubMsgResult::Err(e) => {
                // Keep the pending flag set so another retry can be attempted
                // later. Don't revert — that would propagate the error back
                // to the caller and could trap gas in a retry loop.
                Ok(Response::new()
                    .add_attribute("action", "factory_notify_retry_failed")
                    .add_attribute("reason", e)
                    .add_attribute("block_time", env.block.time.seconds().to_string()))
            }
        },
        _ => Err(StdError::generic_err(format!(
            "Unknown reply id: {}",
            msg.id
        ))),
    }
}

// ---------------------------------------------------------------------------
// Swap (CW20 hook + core logic)
// ---------------------------------------------------------------------------

pub fn execute_swap_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    if !query_check_commit(deps.as_ref())? {
        return Err(ContractError::ShortOfThreshold {});
    }
    if cw20_msg.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }
    let contract_addr = info.sender.clone();
    match from_json(&cw20_msg.msg) {
        Ok(Cw20HookMsg::Swap {
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        }) => {
            let pool_info: PoolInfo = POOL_INFO.load(deps.storage)?;
            let authorized = pool_info.pool_info.asset_infos.iter().any(|t| {
                matches!(t, TokenType::CreatorToken { contract_addr } if contract_addr == info.sender)
            });
            if !authorized {
                return Err(ContractError::Unauthorized {});
            }
            let to_addr = to.map(|a| deps.api.addr_validate(&a)).transpose()?;
            let validated_sender = deps.api.addr_validate(&cw20_msg.sender)?;
            simple_swap(
                deps,
                env,
                info,
                validated_sender,
                TokenInfo {
                    info: TokenType::CreatorToken { contract_addr },
                    amount: cw20_msg.amount,
                },
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        Err(err) => Err(ContractError::Std(err)),
    }
}

#[allow(clippy::too_many_arguments)]
fn simple_swap(
    mut deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
    transaction_deadline: Option<cosmwasm_std::Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let reentrancy_guard = REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;

    let pool_specs = POOL_SPECS.load(deps.storage)?;

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        REENTRANCY_LOCK.save(deps.storage, &false)?;
        return Err(e);
    }

    let result = execute_simple_swap(
        &mut deps,
        env,
        _info,
        sender,
        offer_asset,
        belief_price,
        max_spread,
        to,
    );
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
}

#[allow(clippy::too_many_arguments)]
pub fn execute_simple_swap(
    deps: &mut DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
) -> Result<Response, ContractError> {
    let PoolCtx {
        info: pool_info,
        state: mut pool_state,
        fees: mut pool_fee_state,
        specs: pool_specs,
    } = PoolCtx::load(deps.storage)?;

    let (offer_index, offer_pool, ask_pool) =
        if offer_asset.info.equal(&pool_info.pool_info.asset_infos[0]) {
            (0usize, pool_state.reserve0, pool_state.reserve1)
        } else if offer_asset.info.equal(&pool_info.pool_info.asset_infos[1]) {
            (1usize, pool_state.reserve1, pool_state.reserve0)
        } else {
            return Err(ContractError::AssetMismatch {});
        };

    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    // Drain guard: reject swaps when either side is below MINIMUM_LIQUIDITY.
    // Don't try to persist POOL_PAUSED here — returning Err would revert the
    // save, so it's dead state. The reserve check alone is sufficient to
    // block every swap path; admins unlock the pool by restoring reserves or
    // by calling the factory's explicit UnpausePool route if POOL_PAUSED was
    // ever set by a successful admin action.
    if pool_state.reserve0 < MINIMUM_LIQUIDITY || pool_state.reserve1 < MINIMUM_LIQUIDITY {
        return Err(ContractError::InsufficientReserves {});
    }

    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, offer_asset.amount, pool_specs.lp_fee)?;

    // Reject dust swaps where the constant-product math floored
    // return_amt to zero. Without this, the trader's offer would be
    // absorbed into the pool while they receive nothing — effectively
    // donating to LPs. Better to surface the "offer too small" error
    // and let the caller bump their size or abandon.
    if return_amt.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    assert_max_spread(
        belief_price,
        max_spread,
        offer_asset.amount,
        return_amt.checked_add(commission_amt)?,
        spread_amt,
    )?;

    let offer_pool_post = offer_pool.checked_add(offer_asset.amount)?;
    let ask_pool_post = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

    if ask_pool_post < MINIMUM_LIQUIDITY {
        return Err(ContractError::InsufficientReserves {});
    }

    // TWAP: accumulate price using OLD reserves before updating
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

    if offer_index == 0 {
        pool_state.reserve0 = offer_pool_post;
        pool_state.reserve1 = ask_pool_post;
    } else {
        pool_state.reserve0 = ask_pool_post;
        pool_state.reserve1 = offer_pool_post;
    }

    update_pool_fee_growth(
        &mut pool_fee_state,
        &pool_state,
        offer_index,
        commission_amt,
    )?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    // Update analytics counters
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_swap_count += 1;
    if offer_index == 0 {
        analytics.total_volume_0 = analytics.total_volume_0.saturating_add(offer_asset.amount);
        analytics.total_volume_1 = analytics.total_volume_1.saturating_add(return_amt);
    } else {
        analytics.total_volume_1 = analytics.total_volume_1.saturating_add(offer_asset.amount);
        analytics.total_volume_0 = analytics.total_volume_0.saturating_add(return_amt);
    }
    analytics.last_trade_block = env.block.height;
    analytics.last_trade_timestamp = env.block.time.seconds();
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let ask_asset_info = if offer_index == 0 {
        pool_info.pool_info.asset_infos[1].clone()
    } else {
        pool_info.pool_info.asset_infos[0].clone()
    };

    // Lazy-evaluate sender.clone() so the clone is skipped when `to` is Some.
    let receiver = to.unwrap_or_else(|| sender.clone());
    let msgs = if !return_amt.is_zero() {
        vec![TokenInfo {
            info: ask_asset_info.clone(),
            amount: return_amt,
        }
        .into_msg(&deps.querier, receiver.clone())?]
    } else {
        vec![]
    };

    // Effective price: how much ask per unit of offer the trader received
    let effective_price = if !offer_asset.amount.is_zero() {
        Decimal::from_ratio(return_amt, offer_asset.amount).to_string()
    } else {
        "0".to_string()
    };

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "swap")
        .add_attribute("sender", sender)
        .add_attribute("receiver", receiver)
        .add_attribute("offer_asset", offer_asset.info.to_string())
        .add_attribute("ask_asset", ask_asset_info.to_string())
        .add_attribute("offer_amount", offer_asset.amount.to_string())
        .add_attribute("return_amount", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount", commission_amt.to_string())
        .add_attribute("effective_price", effective_price)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_fee_collected_0",
            pool_fee_state.total_fees_collected_0.to_string(),
        )
        .add_attribute(
            "total_fee_collected_1",
            pool_fee_state.total_fees_collected_1.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string())
        .add_attribute("total_swap_count", analytics.total_swap_count.to_string()))
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
