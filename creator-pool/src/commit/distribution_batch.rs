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

use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Env, Order, StdResult, Storage, SubMsg, Uint128, Uint256,
    WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_storage_plus::Bound;

use crate::error::ContractError;
use crate::state::{
    DistributionState, PendingMint, PoolInfo, COMMITFEEINFO, COMMIT_LEDGER,
    DISTRIBUTION_STALL_TIMEOUT_SECONDS, DISTRIBUTION_STATE,
    MAX_CONSECUTIVE_DISTRIBUTION_FAILURES, MAX_DISTRIBUTIONS_PER_TX, NEXT_DIST_MINT_REPLY_ID,
    PENDING_MINT_REPLIES, REPLY_ID_DISTRIBUTION_MINT_BASE,
};

/// Build a `SubMsg::reply_always` that mints `amount` of the pool's
/// CW20 to `recipient`, wrapped so a per-mint failure is captured by
/// the contract's reply handler and folded into `FAILED_MINTS` rather
/// than reverting the entire batch tx.
///
/// Used by both `process_distribution_batch` (for bulk distribution +
/// dust settlement) and `execute_claim_failed_distribution` (for
/// user-initiated retries from the failed-mint accumulator).
///
/// Allocates a fresh reply id from `NEXT_DIST_MINT_REPLY_ID`, stashes a
/// `PendingMint { user, amount }` under that id, and emits the SubMsg
/// pointing at the same id. The reply handler in `contract.rs` reads +
/// removes the stash, then either acks success or moves the amount into
/// `FAILED_MINTS` for the user to claim later.
///
/// `user` is the canonical committer address for accounting purposes —
/// even when the SubMsg is wired to mint to a different `recipient`
/// (e.g., on `ClaimFailedDistribution` where the user redirects to a
/// fresh wallet), `FAILED_MINTS` should always be re-credited under the
/// original committer if the redirect also fails.
pub(crate) fn build_distribution_mint_submsg(
    storage: &mut dyn Storage,
    token_addr: &Addr,
    recipient: &Addr,
    user_for_accounting: &Addr,
    amount: Uint128,
) -> StdResult<SubMsg> {
    let counter = NEXT_DIST_MINT_REPLY_ID
        .may_load(storage)?
        .unwrap_or_default();
    let next = counter
        .checked_add(1)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err(
            "NEXT_DIST_MINT_REPLY_ID counter overflow",
        ))?;
    NEXT_DIST_MINT_REPLY_ID.save(storage, &next)?;

    let reply_id = REPLY_ID_DISTRIBUTION_MINT_BASE
        .checked_add(counter)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err(
            "distribution mint reply id space exhausted",
        ))?;

    PENDING_MINT_REPLIES.save(
        storage,
        reply_id,
        &PendingMint {
            user: user_for_accounting.clone(),
            amount,
        },
    )?;

    let mint_msg = Cw20ExecuteMsg::Mint {
        recipient: recipient.to_string(),
        amount,
    };
    let wasm = WasmMsg::Execute {
        contract_addr: token_addr.to_string(),
        msg: to_json_binary(&mint_msg)?,
        funds: vec![],
    };

    Ok(SubMsg::reply_always(CosmosMsg::Wasm(wasm), reply_id))
}

/// Process one batch of pending committer payouts.
///
/// Returns `(submsgs, processed_count)` where `processed_count` is the number
/// of committers whose ledger entries were drained in this call. Callers use
/// this to decide whether the call did real work — important for the
/// distribution-keeper bounty, which should never pay out for an empty/no-op
/// batch (factory funds are finite, and keepers shouldn't farm empty calls).
///
/// Each per-user mint is wrapped in a `SubMsg::reply_always` (see
/// `build_distribution_mint_submsg`). A failing mint is captured by the
/// contract's reply handler and folded into `FAILED_MINTS`, leaving the
/// rest of the batch's storage writes (cursor advance, ledger removal,
/// other successful mints) intact. This is the H6 fix: pre-isolation,
/// any single failing recipient would have reverted the entire batch.
pub fn process_distribution_batch(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    env: &Env,
) -> Result<(Vec<SubMsg>, u32), ContractError> {
    let mut submsgs: Vec<SubMsg> = Vec::new();
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
        // 1. The QueryMsg::DistributionState query, which exposes a
        // computed `is_stalled` flag against this same threshold.
        // 2. The error text below in failed-tx receipts.
        // 3. `recover_distribution` (admin.rs) which gates on
        // `time_since_update >= STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS`
        // independently of any marker.
        // 4. Permissionless `SelfRecoverDistribution` after
        // `PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS` so the
        // admin path is not the only liveness escape.
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
            let mut batch_distributed = Uint128::zero();
            for (payer, usd_paid) in batch.iter() {
                let reward = calculate_committer_reward(
                    *usd_paid,
                    dist_state.total_to_distribute,
                    dist_state.total_committed_usd,
                )?;

                if !reward.is_zero() {
                    submsgs.push(build_distribution_mint_submsg(
                        storage,
                        &pool_info.token_address,
                        payer,
                        payer,
                        reward,
                    )?);
                    batch_distributed = batch_distributed.checked_add(reward)?;
                }
                COMMIT_LEDGER.remove(storage, payer);
                last_processed = Some(payer.clone());
                processed_count += 1;
            }
            let new_distributed_so_far = dist_state
                .distributed_so_far
                .checked_add(batch_distributed)?;

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
                // Final batch (or stale-state cleanup with empty ledger).
                // Settle the per-user floor-division dust deterministically
                // by minting the residual to the creator wallet so the
                // pool's `total_to_distribute` is fully accounted for and
                // no portion of the threshold-payout schedule is left
                // uncirculated. Reasoning: each per-user reward is
                // `floor(usd_paid * total_to_distribute / total_committed_usd)`,
                // so the sum can be up to (N - 1) base units short. The
                // creator is the natural recipient — they have the most
                // reputational exposure if the protocol ever leaves
                // committer rewards unsettled, and they cannot manipulate
                // the residual without manipulating the per-user floors
                // in a way that already harms their committers (and is
                // therefore self-defeating). On legacy in-progress
                // distributions started before `distributed_so_far`
                // existed (`#[serde(default)]` → zero) the residual would
                // equal the full `total_to_distribute`, which would
                // double-mint; gate the settlement on `distributed_so_far
                // > 0` so legacy distributions complete with the
                // pre-upgrade dust-burn behavior intact.
                if !new_distributed_so_far.is_zero()
                    && new_distributed_so_far < dist_state.total_to_distribute
                {
                    let residual = dist_state
                        .total_to_distribute
                        .checked_sub(new_distributed_so_far)?;
                    let fee_info = COMMITFEEINFO.load(storage)?;
                    let creator = fee_info.creator_wallet_address.clone();
                    submsgs.push(build_distribution_mint_submsg(
                        storage,
                        &pool_info.token_address,
                        &creator,
                        &creator,
                        residual,
                    )?);
                }
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
                    distributed_so_far: new_distributed_so_far,
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
            // Genuine `COMMIT_LEDGER.range(...)` failure — only reachable
            // on storage corruption (deserialization error). The previous
            // code path bumped `consecutive_failures` and tried to
            // persist the new state before returning Err, but CosmWasm
            // reverts every storage write in a handler that returns
            // `Err`, so neither save persisted — the documented
            // "stop after MAX_CONSECUTIVE_DISTRIBUTION_FAILURES"
            // never fired through this branch in practice. Removed the
            // dead increment + save and surface the corruption as a
            // typed error so operators investigate root cause directly
            // rather than relying on a give-up gate that wouldn't
            // accumulate. The "ledger has more entries but range
            // returned zero rows" branch above (line ~263) is the only
            // path where the failure counter actually accumulates,
            // because that path returns Ok and its save survives.
            return Err(ContractError::DistributionBatchFailed {
                attempt: 0,
                reason: format!(
                    "ledger range failed (storage corruption?): {}; investigate \
                     COMMIT_LEDGER state — auto-recovery via \
                     consecutive_failures counter does not apply on this path",
                    e
                ),
            });
        }
    }

    Ok((submsgs, processed_count))
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
