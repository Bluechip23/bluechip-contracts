//! Pool contract entry points: instantiate, execute dispatch, swap, migrate.
//!
//! Commit logic lives in [`crate::commit`], admin operations in [`crate::admin`].

use crate::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw, execute_emergency_withdraw,
    execute_pause, execute_recover_stuck_states, execute_unpause,
    execute_update_config_from_factory,
};
use crate::asset::{PoolPairType, TokenInfoPoolExt, TokenType};
use crate::commit::{commit, execute_continue_distribution};
use crate::error::ContractError;
use crate::generic_helpers::validate_pool_threshold_payments;
use crate::liquidity::{
    execute_add_to_position, execute_collect_fees, execute_deposit_liquidity,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent,
};
use crate::liquidity_helpers::{execute_claim_creator_excess, execute_claim_creator_fees};
use crate::msg::{ExecuteMsg, MigrateMsg, PoolInstantiateMsg};
use crate::query::query_check_commit;
use crate::state::{
    CommitLimitInfo, ExpectedFactory, OracleInfo, PoolAnalytics, PoolDetails, PoolFeeState,
    PoolInfo, PoolSpecs, Position, ThresholdPayoutAmounts, COMMITFEEINFO, COMMIT_LIMIT_INFO,
    EXPECTED_FACTORY, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID, ORACLE_INFO,
    OWNER_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS,
    POOL_STATE, THRESHOLD_PAYOUT_AMOUNTS, USD_RAISED_FROM_COMMIT,
};
use crate::state::{
    PoolState, LIQUIDITY_POSITIONS, PENDING_FACTORY_NOTIFY, REPLY_ID_FACTORY_NOTIFY_INITIAL,
    REPLY_ID_FACTORY_NOTIFY_RETRY,
};
// Swap orchestration moved to pool_core::swap; re-exported via swap_helper.
use crate::swap_helper::{execute_swap_cw20, simple_swap};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo,
    Reply, Response, StdError, StdResult, Storage, SubMsg, SubMsgResult, Uint128, WasmMsg,
};
use cw2::set_contract_version;

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

    // Enforce strict pair shape AND ordering: index 0 = Bluechip
    // (Native), index 1 = CreatorToken matching `msg.token_address`.
    // Every downstream piece of commit/swap/threshold-payout code
    // hard-codes `reserve0 == bluechip, reserve1 == creator-token`,
    // so a reversed pair would silently produce wrong-direction swaps.
    // Defense-in-depth — the factory's validate_pool_token_info enforces
    // the same invariant — but rejecting again here means a buggy
    // factory migration or a directly-instantiated pool (e.g. via a raw
    // Wasm instantiate bypassing the factory entirely) can't silently
    // produce a pool whose reserve accounting disagrees with its
    // holdings.
    match (&msg.pool_token_info[0], &msg.pool_token_info[1]) {
        (TokenType::Native { denom }, TokenType::CreatorToken { contract_addr }) => {
            if denom.trim().is_empty() {
                return Err(ContractError::Std(StdError::generic_err(
                    "Bluechip denom must be non-empty",
                )));
            }
            if contract_addr != &msg.token_address {
                return Err(ContractError::Std(StdError::generic_err(
                    "CreatorToken.contract_addr in pool_token_info must equal msg.token_address",
                )));
            }
        }
        _ => {
            return Err(ContractError::Std(StdError::generic_err(
                "pool_token_info must be [Bluechip(Native), CreatorToken] — \
                 order matters: bluechip at index 0, creator-token at index 1.",
            )));
        }
    }
    if (msg.commit_fee_info.commit_fee_bluechip + msg.commit_fee_info.commit_fee_creator)
        > Decimal::one()
    {
        return Err(ContractError::InvalidFee {});
    }

    let threshold_payout_amounts = if let Some(params_binary) = msg.threshold_payout {
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
        // Sentinel position at id "0" — no actual liquidity, no lock.
        locked_liquidity: Uint128::zero(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3), // 0.3% LP fee
        min_commit_interval: 13,      // seconds
    };

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: msg.commit_threshold_limit_usd,
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
    // Creator pools start pre-threshold — swap / liquidity entry points
    // gate on this until `process_threshold_crossing_with_excess` flips
    // it to `true` during the threshold-crossing commit.
    IS_THRESHOLD_HIT.save(deps.storage, &false)?;
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

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool_kind", "commit")
        .add_attribute("pool", env.contract.address.to_string()))
}


// ---------------------------------------------------------------------------
// Execute dispatch
// ---------------------------------------------------------------------------

/// Pause-only gate. Used by `Commit` (pre-threshold path) where the pool
/// has no reserves yet, so the drain check from `check_pool_writable`
/// doesn't apply. Identical inlined check was repeated 9× across the
/// dispatch arms before this extraction.
fn check_pool_not_paused(storage: &dyn Storage) -> Result<(), ContractError> {
    if POOL_PAUSED.may_load(storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    Ok(())
}

/// Liquidity-write gate: every deposit / add / remove / collect / claim
/// path must fail closed when an emergency drain has been initiated OR
/// when admin has paused the pool. Combines the two checks that were
/// previously copy-pasted into 8 dispatch arms.
fn check_pool_writable(storage: &dyn Storage) -> Result<(), ContractError> {
    ensure_not_drained(storage)?;
    check_pool_not_paused(storage)
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
            // Block ALL commits while paused — pre-threshold AND post-threshold.
            // Previously only process_post_threshold_commit checked POOL_PAUSED,
            // so admin pauses failed to stop pre-threshold deposits, letting
            // users trap funds in the COMMIT_LEDGER of a paused pool.
            check_pool_not_paused(deps.storage)?;
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
            offer_asset.confirm_sent_native_balance(&info)?;
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
            check_pool_writable(deps.storage)?;
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
            check_pool_writable(deps.storage)?;
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
            // Defense-in-depth: currently a drained pool has
            // total_liquidity == 0 so the math inside remove_partial_liquidity
            // would error out on its own. That's a coincidence, not a
            // guarantee. If a future partial-drain or recovery path ever
            // leaves non-zero total_liquidity after EMERGENCY_DRAINED is set,
            // an explicit check here keeps users from pulling against
            // already-swept reserves with arbitrary math.
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
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount1,
            min_amount0,
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
        ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline } => {
            // Creator excess exists only when a commit-pool threshold is crossed
            // with more raised-bluechip than `max_bluechip_lock_per_pool`
            // absorbs. Standard pools have no commit phase and no excess
            // position, so there's nothing to claim.
            check_pool_writable(deps.storage)?;
            execute_claim_creator_excess(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::ClaimCreatorFees { transaction_deadline } => {
            // The creator fee pot is seeded by the fee_size_multiplier
            // clip on commit-pool LP fees. Standard pools have no creator
            // concept, so the pot is always empty and this handler is N/A.
            check_pool_writable(deps.storage)?;
            execute_claim_creator_fees(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::RetryFactoryNotify {} => {
            // Retries NotifyThresholdCrossed to the factory. Standard
            // pools never cross a threshold, so there's nothing to retry.
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
