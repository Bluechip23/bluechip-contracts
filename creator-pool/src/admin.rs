//! Creator-pool admin handlers.
//!
//! Shared handlers — pause, unpause, cancel-emergency-withdraw,
//! update-config-from-factory, ensure_not_drained — live in
//! `pool_core::admin` and are re-exported below so existing
//! `use crate::admin::X;` imports resolve unchanged.
//!
//! The creator-pool crate keeps:
//!   - `execute_emergency_withdraw` — a wrapper around pool-core's
//!     two-phase initiate/core_drain that adds the commit-only
//!     pre-threshold rejection, CREATOR_EXCESS_POSITION sweep, and
//!     DISTRIBUTION_STATE halt.
//!   - `execute_recover_stuck_states` + private recovery helpers —
//!     all three failure modes (stuck threshold, stalled distribution,
//!     jammed reentrancy guard) only ever occur inside the commit
//!     flow, so standard-pool doesn't need them.

pub use pool_core::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw,
    execute_emergency_withdraw_dispatch, execute_pause, execute_unpause,
    execute_update_config_from_factory, CoreDrainResult,
};

use crate::error::ContractError;
use crate::state::{
    DistributionState, RecoveryType, COMMIT_LEDGER, CREATOR_EXCESS_POSITION,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX, DISTRIBUTION_STATE,
    EXPECTED_FACTORY, IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT,
    MAX_CONSECUTIVE_DISTRIBUTION_FAILURES, PENDING_EMERGENCY_WITHDRAW, POOL_INFO, REENTRANCY_LOCK,
    STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS, STUCK_THRESHOLD_RECOVERY_WINDOW_SECONDS,
    THRESHOLD_PROCESSING,
};
use cosmwasm_std::{
    DepsMut, Env, MessageInfo, Order, Response, StdResult, Storage, Timestamp, Uint128,
};

// ---------------------------------------------------------------------------
// Emergency Withdraw — creator-pool wrapper
// ---------------------------------------------------------------------------

/// Wraps pool-core's two-phase emergency withdraw with commit-only
/// bookkeeping:
///   - Pre-threshold rejection (committed funds are untracked in
///     reserves; draining would strand them).
///   - CREATOR_EXCESS_POSITION sweep on Phase 2 — fold its amounts into
///     `accumulation_drain_{0,1}` so pool-core's single audit record
///     captures the grand total and the two transfer messages carry it.
///   - DISTRIBUTION_STATE halt on Phase 2 so future
///     ContinueDistribution calls reject cleanly.
///
/// Phase 1/2 dispatch matches pre-split behavior: if
/// `PENDING_EMERGENCY_WITHDRAW` is unset we run Phase 1 (pause + set
/// timelock); otherwise Phase 2 (drain after the timelock has elapsed).
pub fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Duplicate auth + drained checks here so the pre-threshold error
    // below doesn't mask unauthorized access. Pool-core's initiate /
    // core_drain do their own checks too — cheap loads, worth it to
    // preserve the pre-split error ordering.
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    ensure_not_drained(deps.storage)?;

    // Disable emergency withdraw pre-threshold. Pre-threshold, reserve0/1
    // are both zero because pool seeding only happens inside
    // `trigger_threshold_payout`. A drain here would sweep nothing out
    // of state-tracked reserves, BUT it would mark the pool
    // EMERGENCY_DRAINED and permanently lock all future actions — while
    // the pool's actual bank balance (committed bluechip minus fees) sits
    // stranded forever. The correct recovery path for a pre-threshold
    // pool is a future cancel/refund flow; until that exists, refuse to
    // run emergency withdraw at all before the threshold has crossed.
    if !IS_THRESHOLD_HIT
        .may_load(deps.storage)?
        .unwrap_or(false)
    {
        return Err(ContractError::EmergencyWithdrawPreThreshold);
    }

    // Phase 1 (no timelock armed yet) is a pure pause + arm — no
    // commit-only bookkeeping should run yet. The original wrapper
    // returned early here before sweeping `CREATOR_EXCESS_POSITION` /
    // halting `DISTRIBUTION_STATE`; preserve that ordering so the
    // Phase 1 transaction does not delete user-owned state during
    // the timelock window.
    let pending_armed = PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_some();

    // Phase 2 bookkeeping: capture creator excess + halt distribution
    // state BEFORE handing off to the shared dispatcher. CosmWasm tx
    // atomicity rolls these saves back along with the rest of the tx
    // if anything inside the dispatcher errors, so half-drained state
    // is structurally unreachable.
    let mut deps = deps;
    let (acc_0, acc_1) = if pending_armed {
        let excess = CREATOR_EXCESS_POSITION.may_load(deps.storage)?;
        let amounts = excess
            .as_ref()
            .map(|e| (e.bluechip_amount, e.token_amount))
            .unwrap_or((Uint128::zero(), Uint128::zero()));
        if excess.is_some() {
            CREATOR_EXCESS_POSITION.remove(deps.storage);
        }
        // The pool no longer holds a bounty reserve; distribution
        // bounties are paid by the factory. Halt any in-flight
        // distribution so future ContinueDistribution calls reject
        // cleanly.
        if let Ok(mut dist_state) = DISTRIBUTION_STATE.load(deps.storage) {
            dist_state.is_distributing = false;
            dist_state.distributions_remaining = 0;
            DISTRIBUTION_STATE.save(deps.storage, &dist_state)?;
        }
        amounts
    } else {
        (Uint128::zero(), Uint128::zero())
    };

    execute_emergency_withdraw_dispatch(deps.branch(), env, info, acc_0, acc_1)
}

// ---------------------------------------------------------------------------
// Stuck-state recovery (factory-only; commit-phase only)
// ---------------------------------------------------------------------------

pub fn execute_recover_stuck_states(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recovery_type: RecoveryType,
) -> Result<Response, ContractError> {
    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;
    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    // Reject recovery on a drained pool. None of the three recovery
    // branches (StuckThreshold, StuckDistribution, StuckReentrancyGuard)
    // produce any fund-flow on a drained pool — every downstream consumer
    // (commit, ContinueDistribution, swap) re-checks `ensure_not_drained`
    // and fails closed — but `recover_distribution` re-saves
    // DISTRIBUTION_STATE with `is_distributing = true`, leaving operators
    // who poll `QueryMsg::DistributionState` after a drain seeing
    // `is_distributing: true, is_stalled: false` while every keeper call
    // fails. Failing here surfaces the post-drain state cleanly and
    // keeps the recovery path's invariants tight ("only meaningful on a
    // live pool").
    ensure_not_drained(deps.storage)?;

    let mut attributes = vec![("action", "recover_stuck_states".to_string())];
    let mut recovered_items = vec![];

    match recovery_type {
        RecoveryType::StuckThreshold => {
            recover_threshold(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckDistribution => {
            recover_distribution(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckReentrancyGuard => {
            recover_reentrancy_guard(deps.storage, &mut recovered_items)?;
        }
        RecoveryType::Both => {
            let _ = recover_threshold(deps.storage, &env, &mut recovered_items);
            let _ = recover_distribution(deps.storage, &env, &mut recovered_items);
            let _ = recover_reentrancy_guard(deps.storage, &mut recovered_items);
        }
    }

    if recovered_items.is_empty() {
        return Err(ContractError::NothingToRecover {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    attributes.push(("recovered", recovered_items.join(",")));
    attributes.push((
        "pool_contract",
        pool_info.pool_info.contract_addr.to_string(),
    ));
    attributes.push(("recovered_by", info.sender.to_string()));
    attributes.push(("block_height", env.block.height.to_string()));
    attributes.push(("block_time", env.block.time.seconds().to_string()));
    Ok(Response::new().add_attributes(attributes))
}

fn recover_threshold(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    let last_threshold_time = LAST_THRESHOLD_ATTEMPT
        .may_load(storage)?
        .unwrap_or(Timestamp::from_seconds(0));

    if env.block.time.seconds()
        >= last_threshold_time.seconds() + STUCK_THRESHOLD_RECOVERY_WINDOW_SECONDS
    {
        let was_stuck = THRESHOLD_PROCESSING.may_load(storage)?.unwrap_or(false);
        if was_stuck {
            THRESHOLD_PROCESSING.save(storage, &false)?;
            recovered.push("threshold".to_string());
        }
    }
    Ok(())
}

fn recover_distribution(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    if let Some(dist_state) = DISTRIBUTION_STATE.may_load(storage)? {
        let time_since_update = env
            .block
            .time
            .seconds()
            .saturating_sub(dist_state.last_updated.seconds());

        if time_since_update >= STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS
            || dist_state.consecutive_failures >= MAX_CONSECUTIVE_DISTRIBUTION_FAILURES
        {
            let remaining_committers = COMMIT_LEDGER
                .keys(storage, None, None, Order::Ascending)
                .count() as u32;

            if remaining_committers == 0 {
                DISTRIBUTION_STATE.remove(storage);
            } else {
                let restarted = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: None,
                    distributions_remaining: remaining_committers,
                    estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
                    max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
                    last_successful_batch_size: None,
                    consecutive_failures: 0,
                    started_at: env.block.time,
                    last_updated: env.block.time,
                };
                DISTRIBUTION_STATE.save(storage, &restarted)?;
            }

            recovered.push(format!(
                "distribution_restarted_{}_remaining",
                remaining_committers
            ));
        }
    }
    Ok(())
}

fn recover_reentrancy_guard(
    storage: &mut dyn Storage,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    let guard = REENTRANCY_LOCK.may_load(storage)?.unwrap_or(false);
    if guard {
        REENTRANCY_LOCK.save(storage, &false)?;
        recovered.push("reentrancy_guard".to_string());
    }
    Ok(())
}
