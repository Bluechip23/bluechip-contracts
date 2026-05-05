//! Inner batch processor for the post-threshold distribution loop.
//!
//! `process_distribution_batch` drains up to one keeper-batch worth of
//! committers from `COMMIT_LEDGER`, mints each their pro-rata share of
//! `total_to_distribute`, and either advances the `DISTRIBUTION_STATE`
//! cursor or removes it entirely when the ledger is exhausted.
//!
//! Termination is driven by ledger emptiness, not by
//! `distributions_remaining`. The counter is informational; the ground
//! truth is whether `COMMIT_LEDGER` has any entries past the cursor.
//! This means a single tx can both process the final committer AND
//! remove `DISTRIBUTION_STATE`, eliminating the previous "one extra
//! empty cleanup call" pattern that paid a free bounty.
//!
//! See also `commit/distribution.rs` for the
//! `execute_continue_distribution` entry point that routes incoming
//! keeper calls through this helper, and `commit/threshold_payout.rs`
//! for the threshold-crossing handler that initialises
//! `DISTRIBUTION_STATE` in the first place.

use cosmwasm_std::{Addr, CosmosMsg, Env, Order, StdResult, Storage, Uint128, Uint256};
use cw_storage_plus::Bound;

use crate::error::ContractError;
use crate::state::{
    DistributionState, PoolInfo, COMMIT_LEDGER, DISTRIBUTION_STALL_TIMEOUT_SECONDS,
    DISTRIBUTION_STATE, MAX_CONSECUTIVE_DISTRIBUTION_FAILURES, MAX_DISTRIBUTIONS_PER_TX,
};

use super::threshold_payout::mint_tokens;

/// Process one batch of pending committer payouts.
///
/// Returns `(msgs, processed_count)` where `processed_count` is the number of
/// committers whose ledger entries were drained in this call. Callers use
/// this to decide whether the call did real work — important for the
/// distribution-keeper bounty, which should never pay out for an empty/no-op
/// batch (factory funds are finite, and keepers shouldn't farm empty calls).
pub fn process_distribution_batch(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    env: &Env,
) -> Result<(Vec<CosmosMsg>, u32), ContractError> {
    let mut msgs = Vec::new();
    let mut dist_state = match DISTRIBUTION_STATE.may_load(storage)? {
        Some(state) => state,
        None => return Ok((vec![], 0)), // No distribution in progress
    };
    let time_since_update = env
        .block
        .time
        .seconds()
        .saturating_sub(dist_state.last_updated.seconds());
    if time_since_update > DISTRIBUTION_STALL_TIMEOUT_SECONDS {
        // Surfaces the stall as a tx error to the keeper. We do NOT write
        // a marker into DISTRIBUTION_STATE here — CosmWasm reverts every
        // staged storage write when a handler returns Err, so any save
        // immediately before this return would be discarded along with
        // the tx. Operators detect stalls via:
        //   1. The QueryMsg::DistributionState query, which exposes a
        //      computed `is_stalled` flag against this same threshold.
        //   2. The error text below in failed-tx receipts.
        //   3. `recover_distribution` (admin.rs) which gates on
        //      `time_since_update >= STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS`
        //      independently of any marker.
        return Err(ContractError::DistributionTimeout);
    }
    let start_after = dist_state.last_processed_key.as_ref().map(Bound::exclusive);

    let effective_batch_size = calculate_effective_batch_size(&dist_state);
    let mut processed_count = 0u32;
    let mut last_processed: Option<Addr> = None;
    let batch_result: StdResult<Vec<(Addr, Uint128)>> = {
        COMMIT_LEDGER
            .range(storage, start_after, None, Order::Ascending)
            .take(effective_batch_size as usize)
            .collect::<StdResult<Vec<_>>>()
    };

    match batch_result {
        Ok(batch) => {
            for (payer, usd_paid) in batch.iter() {
                let reward = calculate_committer_reward(
                    *usd_paid,
                    dist_state.total_to_distribute,
                    dist_state.total_committed_usd,
                )?;

                if !reward.is_zero() {
                    msgs.push(mint_tokens(&pool_info.token_address, payer, reward)?);
                }
                COMMIT_LEDGER.remove(storage, payer);
                last_processed = Some(payer.clone());
                processed_count += 1;
            }

            // Use the actual ledger as the source of truth for termination.
            // The cursor must start *after* whatever we last touched: either
            // the last entry we processed in this call, or — if we processed
            // nothing — the saved cursor from prior calls.
            let recheck_start = last_processed
                .as_ref()
                .or(dist_state.last_processed_key.as_ref())
                .map(Bound::exclusive);
            let ledger_has_more = COMMIT_LEDGER
                .range(storage, recheck_start, None, Order::Ascending)
                .take(1)
                .next()
                .is_some();

            if !ledger_has_more {
                // Nothing left to distribute. Clean up regardless of whether
                // we processed any entries this call (covers both "natural
                // completion" and "stale state with empty ledger" paths).
                DISTRIBUTION_STATE.remove(storage);
            } else if processed_count > 0 {
                // Progress made; persist the new cursor for the next call.
                let new_remaining = dist_state
                    .distributions_remaining
                    .saturating_sub(processed_count);
                let updated_state = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: last_processed,
                    distributions_remaining: new_remaining,
                    estimated_gas_per_distribution: dist_state.estimated_gas_per_distribution,
                    max_gas_per_tx: dist_state.max_gas_per_tx,
                    last_successful_batch_size: Some(processed_count),
                    consecutive_failures: 0,
                    started_at: dist_state.started_at,
                    last_updated: env.block.time,
                };
                DISTRIBUTION_STATE.save(storage, &updated_state)?;
            } else {
                // Ledger has more entries but our `take(N)` returned zero.
                // That's anomalous — bump the failure counter and bail at
                // MAX_CONSECUTIVE_DISTRIBUTION_FAILURES.
                dist_state.consecutive_failures += 1;

                if dist_state.consecutive_failures >= MAX_CONSECUTIVE_DISTRIBUTION_FAILURES {
                    dist_state.is_distributing = false;
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;

                    return Err(ContractError::DistributionFailedTooManyTimes {
                        attempts: dist_state.consecutive_failures,
                        cap: MAX_CONSECUTIVE_DISTRIBUTION_FAILURES,
                        reason: "ledger has entries but range yielded zero rows".to_string(),
                    });
                } else {
                    dist_state.last_updated = env.block.time;
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;
                }
            }
        }
        Err(e) => {
            dist_state.consecutive_failures += 1;

            if dist_state.consecutive_failures >= MAX_CONSECUTIVE_DISTRIBUTION_FAILURES {
                dist_state.is_distributing = false;
                DISTRIBUTION_STATE.save(storage, &dist_state)?;

                return Err(ContractError::DistributionFailedTooManyTimes {
                    attempts: dist_state.consecutive_failures,
                    cap: MAX_CONSECUTIVE_DISTRIBUTION_FAILURES,
                    reason: e.to_string(),
                });
            } else {
                DISTRIBUTION_STATE.save(storage, &dist_state)?;
                return Err(ContractError::DistributionBatchFailed {
                    attempt: dist_state.consecutive_failures,
                    reason: e.to_string(),
                });
            }
        }
    }

    Ok((msgs, processed_count))
}

pub fn calculate_effective_batch_size(dist_state: &DistributionState) -> u32 {
    let base_batch_size = if dist_state.estimated_gas_per_distribution == 0 {
        1u32
    } else {
        let raw = dist_state.max_gas_per_tx / dist_state.estimated_gas_per_distribution;
        if raw > u32::MAX as u64 {
            u32::MAX
        } else {
            raw as u32
        }
    };

    base_batch_size.min(MAX_DISTRIBUTIONS_PER_TX).max(1)
}

fn calculate_committer_reward(
    usd_paid: Uint128,
    total_to_distribute: Uint128,
    total_committed_usd: Uint128,
) -> StdResult<Uint128> {
    if total_committed_usd.is_zero() {
        return Ok(Uint128::zero());
    }

    let reward = Uint128::try_from(
        Uint256::from(usd_paid)
            .checked_mul(Uint256::from(total_to_distribute))?
            .checked_div(Uint256::from(total_committed_usd))?,
    )?;

    Ok(reward)
}
