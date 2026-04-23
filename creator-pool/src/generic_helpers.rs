//! Commit-phase-specific helpers. Shared primitives (`check_rate_limit`,
//! `enforce_transaction_deadline`, `update_pool_fee_growth`,
//! `decimal2decimal256`, `get_bank_transfer_to_msg`) live in
//! `pool_core::generic` and are re-exported below so every existing
//! `use crate::generic_helpers::X;` import resolves unchanged.
pub use pool_core::generic::*;

use crate::error::ContractError;
use crate::msg::CommitFeeInfo;
use crate::state::{
    CommitLimitInfo, Committing, PoolFeeState, PoolInfo, ThresholdPayoutAmounts, COMMIT_INFO,
    COMMIT_LEDGER, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
    DISTRIBUTION_STALL_TIMEOUT_SECONDS, MAX_DISTRIBUTIONS_PER_TX,
};
use crate::state::{
    CreatorExcessLiquidity, DistributionState, PoolState, CREATOR_EXCESS_POSITION,
    DISTRIBUTION_STATE, POOL_FEE_STATE, POOL_STATE,
};
use pool_core::liquidity_helpers::integer_sqrt;
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Env, Order, StdError, StdResult, Storage, SubMsg,
    Timestamp, Uint128, Uint256, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_storage_plus::Bound;



pub fn validate_pool_threshold_payments(
    params: &ThresholdPayoutAmounts,
) -> Result<(), ContractError> {
    const EXPECTED_CREATOR: u128 = 325_000_000_000;
    const EXPECTED_BLUECHIP: u128 = 25_000_000_000;
    const EXPECTED_POOL: u128 = 350_000_000_000;
    const EXPECTED_COMMIT: u128 = 500_000_000_000;
    const EXPECTED_TOTAL: u128 = 1_200_000_000_000;

    if params.creator_reward_amount != Uint128::new(EXPECTED_CREATOR) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Creator amount must be {}", EXPECTED_CREATOR),
        });
    }
    if params.bluechip_reward_amount != Uint128::new(EXPECTED_BLUECHIP) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("BlueChip amount must be {}", EXPECTED_BLUECHIP),
        });
    }
    if params.pool_seed_amount != Uint128::new(EXPECTED_POOL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", EXPECTED_POOL),
        });
    }
    if params.commit_return_amount != Uint128::new(EXPECTED_COMMIT) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Commit amount must be {}", EXPECTED_COMMIT),
        });
    }

    // Verify total
    let total = params
        .creator_reward_amount
        .checked_add(params.bluechip_reward_amount)?
        .checked_add(params.pool_seed_amount)?
        .checked_add(params.commit_return_amount)?;
    if total != Uint128::new(EXPECTED_TOTAL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Total must equal {} (got {})", EXPECTED_TOTAL, total),
        });
    }

    Ok(())
}

/// Output of `trigger_threshold_payout`. The factory notification is
/// separated from the rest of the payout messages because we want it
/// delivered via `SubMsg::reply_on_error` — a failure there should NOT
/// revert the pool-side threshold-crossing state (P4-H5). The caller
/// splices `factory_notify` in as a SubMsg and `other_msgs` as plain
/// CosmosMsgs on the returned Response.
#[derive(Debug)]
pub struct ThresholdPayoutMsgs {
    pub factory_notify: SubMsg,
    pub other_msgs: Vec<CosmosMsg>,
}

#[allow(clippy::too_many_arguments)]
pub fn trigger_threshold_payout(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    pool_state: &mut PoolState,
    pool_fee_state: &mut PoolFeeState,
    commit_config: &CommitLimitInfo,
    payout: &ThresholdPayoutAmounts,
    fee_info: &CommitFeeInfo,
    env: &Env,
) -> StdResult<ThresholdPayoutMsgs> {
    // Factory notification goes out as a `reply_on_error` SubMsg. If the
    // factory handler fails, the pool's `reply` entrypoint sets
    // PENDING_FACTORY_NOTIFY=true and swallows the error so the commit
    // tx overall still succeeds. See state::PENDING_FACTORY_NOTIFY.
    let factory_notify = SubMsg::reply_on_error(
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.factory_addr.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::FactoryExecuteMsg::NotifyThresholdCrossed {
                    pool_id: pool_info.pool_id,
                },
            )?,
            funds: vec![],
        }),
        crate::state::REPLY_ID_FACTORY_NOTIFY_INITIAL,
    );

    let mut other_msgs: Vec<CosmosMsg> = Vec::new();

    let total = payout
        .creator_reward_amount
        .checked_add(payout.bluechip_reward_amount)
        .map_err(StdError::overflow)?
        .checked_add(payout.pool_seed_amount)
        .map_err(StdError::overflow)?
        .checked_add(payout.commit_return_amount)
        .map_err(StdError::overflow)?;

    if total != Uint128::new(1_200_000_000_000) {
        return Err(StdError::generic_err(
            "Threshold payout corruption detected",
        ));
    }

    other_msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.creator_wallet_address,
        payout.creator_reward_amount,
    )?);

    other_msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.bluechip_wallet_address,
        payout.bluechip_reward_amount,
    )?);

    other_msgs.push(mint_tokens(
        &pool_info.token_address,
        &env.contract.address,
        payout.pool_seed_amount,
    )?);

    // Snapshot the committer count at threshold-crossing time. Post-threshold
    // commits never enter COMMIT_LEDGER (they swap directly), so this number
    // is the final size of the work queue. Saturating cast guards against the
    // (currently unreachable) case where threshold settings allow > u32::MAX
    // distinct committers.
    let committer_count_usize = COMMIT_LEDGER
        .keys(storage, None, None, Order::Ascending)
        .count();
    let committer_count = u32::try_from(committer_count_usize).unwrap_or(u32::MAX);

    if committer_count > 0 {
        let dist_state = DistributionState {
            is_distributing: true,
            total_to_distribute: payout.commit_return_amount,
            total_committed_usd: commit_config.commit_amount_for_threshold_usd,
            last_processed_key: None,
            // Real count, not u32::MAX. Termination is now driven by ledger
            // emptiness in process_distribution_batch (the source of truth),
            // and this field is informational/observability data showing
            // how much of the original queue is left.
            distributions_remaining: committer_count,
            estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
            max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
            last_successful_batch_size: None,
            consecutive_failures: 0,
            started_at: env.block.time,
            last_updated: env.block.time,
        };
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
    }

    let total_fee_rate = fee_info
        .commit_fee_bluechip
        .checked_add(fee_info.commit_fee_creator)
        .map_err(|_| StdError::generic_err("Fee rate overflow"))?;
    let total_bluechip_raised = crate::state::NATIVE_RAISED_FROM_COMMIT.load(storage)?;
    let one_minus_fee = Decimal::one()
        .checked_sub(total_fee_rate)
        .map_err(|_| StdError::generic_err("Fee rate >= 100%"))?;
    // Full post-fee bluechip raised seeds the pool. The keeper bounty for
    // distribution batches is paid by the factory from its own reserve, not
    // skimmed from LP funds — see factory::execute_pay_distribution_bounty.
    let pools_bluechip_seed = total_bluechip_raised
        .checked_mul_floor(one_minus_fee)
        .map_err(|_| StdError::generic_err("Fee deduction overflow"))?;

    if pools_bluechip_seed > commit_config.max_bluechip_lock_per_pool {
        let excess_bluechip = pools_bluechip_seed
            .checked_sub(commit_config.max_bluechip_lock_per_pool)
            .map_err(StdError::overflow)?;

        let excess_creator_tokens = payout
            .pool_seed_amount
            .multiply_ratio(excess_bluechip, pools_bluechip_seed);

        CREATOR_EXCESS_POSITION.save(
            storage,
            &CreatorExcessLiquidity {
                creator: fee_info.creator_wallet_address.clone(),
                bluechip_amount: excess_bluechip,
                token_amount: excess_creator_tokens,
                unlock_time: env
                    .block
                    .time
                    .plus_seconds(commit_config.creator_excess_liquidity_lock_days * 86400),
                excess_nft_id: None,
            },
        )?;

        pool_state.reserve0 = commit_config.max_bluechip_lock_per_pool;
        pool_state.reserve1 = payout
            .pool_seed_amount
            .checked_sub(excess_creator_tokens)
            .map_err(StdError::overflow)?;
    } else {
        pool_state.reserve0 = pools_bluechip_seed;
        pool_state.reserve1 = payout.pool_seed_amount;
    }
    // Virtual "unowned" seed liquidity prevents first-depositor share inflation.
    let seed_liquidity = integer_sqrt(pool_state.reserve0.checked_mul(pool_state.reserve1)?);
    pool_state.total_liquidity = seed_liquidity;

    pool_fee_state.fee_growth_global_0 = Decimal::zero();
    pool_fee_state.fee_growth_global_1 = Decimal::zero();
    pool_fee_state.total_fees_collected_0 = Uint128::zero();
    pool_fee_state.total_fees_collected_1 = Uint128::zero();

    POOL_STATE.save(storage, pool_state)?;
    POOL_FEE_STATE.save(storage, pool_fee_state)?;

    Ok(ThresholdPayoutMsgs {
        factory_notify,
        other_msgs,
    })
}

/// Process one batch of pending committer payouts.
///
/// Returns `(msgs, processed_count)` where `processed_count` is the number of
/// committers whose ledger entries were drained in this call. Callers use
/// this to decide whether the call did real work — important for the
/// distribution-keeper bounty, which should never pay out for an empty/no-op
/// batch (factory funds are finite, and keepers shouldn't farm empty calls).
///
/// Termination is driven by ledger emptiness, not by `distributions_remaining`.
/// The counter is informational; the ground truth is whether COMMIT_LEDGER has
/// any entries past the cursor. This means a single tx can both process the
/// final committer AND remove DISTRIBUTION_STATE, eliminating the previous
/// "one extra empty cleanup call" pattern that paid a free bounty.
pub fn process_distribution_batch(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    env: &Env,
) -> StdResult<(Vec<CosmosMsg>, u32)> {
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
        dist_state.consecutive_failures = 99; // Mark as failed
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
        return Err(StdError::generic_err(
            "Distribution timeout - requires manual recovery",
        ));
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
                // That's anomalous — bump the failure counter and bail after 5.
                dist_state.consecutive_failures += 1;

                if dist_state.consecutive_failures >= 5 {
                    dist_state.is_distributing = false;
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;

                    return Err(StdError::generic_err(
                        "Distribution failed too many times - manual recovery needed",
                    ));
                } else {
                    dist_state.last_updated = env.block.time;
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;
                }
            }
        }
        Err(e) => {
            dist_state.consecutive_failures += 1;

            if dist_state.consecutive_failures >= 5 {
                dist_state.is_distributing = false;
                DISTRIBUTION_STATE.save(storage, &dist_state)?;

                return Err(StdError::generic_err(format!(
                    "Distribution batch read failed: {}. Manual recovery needed after {} failures",
                    e, dist_state.consecutive_failures
                )));
            } else {
                DISTRIBUTION_STATE.save(storage, &dist_state)?;
                return Err(StdError::generic_err(format!(
                    "Batch processing failed (attempt {}): {}",
                    dist_state.consecutive_failures, e
                )));
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



pub fn mint_tokens(token_addr: &Addr, recipient: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    let mint_msg = Cw20ExecuteMsg::Mint {
        recipient: recipient.to_string(),
        amount,
    };
    let exec = WasmMsg::Execute {
        contract_addr: token_addr.to_string(),
        msg: to_json_binary(&mint_msg)?,
        funds: vec![],
    };

    Ok(exec.into())
}

pub fn update_commit_info(
    storage: &mut dyn Storage,
    sender: &Addr,
    pool_contract_address: Addr,
    bluechip_amount: Uint128,
    usd_amount: Uint128,
    timestamp: Timestamp,
) -> Result<(), ContractError> {
    COMMIT_INFO.update(
        storage,
        sender,
        |maybe_committing| -> Result<_, ContractError> {
            match maybe_committing {
                Some(mut committing) => {
                    committing.total_paid_bluechip = committing
                        .total_paid_bluechip
                        .checked_add(bluechip_amount)?;
                    committing.total_paid_usd =
                        committing.total_paid_usd.checked_add(usd_amount)?;
                    committing.last_payment_bluechip = bluechip_amount;
                    committing.last_payment_usd = usd_amount;
                    committing.last_committed = timestamp;
                    Ok(committing)
                }
                None => Ok(Committing {
                    pool_contract_address,
                    committer: sender.clone(),
                    total_paid_bluechip: bluechip_amount,
                    total_paid_usd: usd_amount,
                    last_committed: timestamp,
                    last_payment_bluechip: bluechip_amount,
                    last_payment_usd: usd_amount,
                }),
            }
        },
    )?;
    Ok(())
}
