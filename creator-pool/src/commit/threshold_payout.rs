//! Threshold-crossing payout orchestration.
//!
//! Runs once per pool when a commit crosses the
//! `commit_amount_for_threshold_usd` target. Mints the four creator-token
//! splits (`creator_reward_amount`, `bluechip_reward_amount`,
//! `pool_seed_amount`, `commit_return_amount`), seeds the LP reserves
//! from `NATIVE_RAISED_FROM_COMMIT`, parks any creator excess (when
//! raised bluechip exceeds `max_bluechip_lock_per_pool`), schedules the
//! post-threshold distribution batch loop, and emits the factory's
//! `NotifyThresholdCrossed` SubMsg.
//!
//! The factory-notify SubMsg is held aside as `factory_notify` and
//! attached as a `reply_on_error` SubMsg on the calling Response so
//! a factory-side failure does NOT revert the pool's threshold-crossing
//! state — the pool's reply handler sets `PENDING_FACTORY_NOTIFY` and
//! the situation is retryable via `RetryFactoryNotify`.

use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Env, Order, StdError, StdResult, Storage, SubMsg,
    Uint128, WasmMsg,
};
use cw20::Cw20ExecuteMsg;

use crate::error::ContractError;
use crate::msg::CommitFeeInfo;
use crate::state::{
    CommitLimitInfo, CreatorExcessLiquidity, DistributionState, PoolFeeState, PoolInfo, PoolState,
    ThresholdPayoutAmounts, COMMIT_LEDGER, CREATOR_EXCESS_POSITION,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX, DISTRIBUTION_STATE,
    POOL_FEE_STATE, POOL_STATE, SECONDS_PER_DAY, THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS,
    THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS, THRESHOLD_PAYOUT_CREATOR_BASE_UNITS,
    THRESHOLD_PAYOUT_POOL_BASE_UNITS, THRESHOLD_PAYOUT_TOTAL_BASE_UNITS,
};
use pool_core::liquidity_helpers::integer_sqrt;

/// Validate that the four threshold-payout components match the canonical
/// per-pool split (325B + 25B + 350B + 500B = 1.2T base units) and sum
/// to the expected total. Called at pool instantiate so a malformed
/// `threshold_payout` binary fails before the pool is registered.
///
/// Both this validator AND `trigger_threshold_payout` reference the same
/// `THRESHOLD_PAYOUT_*_BASE_UNITS` constants — previously the values
/// lived inline in two places and were vulnerable to silent drift.
pub fn validate_pool_threshold_payments(
    params: &ThresholdPayoutAmounts,
) -> Result<(), ContractError> {
    if params.creator_reward_amount != Uint128::new(THRESHOLD_PAYOUT_CREATOR_BASE_UNITS) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!(
                "Creator amount must be {}",
                THRESHOLD_PAYOUT_CREATOR_BASE_UNITS
            ),
        });
    }
    if params.bluechip_reward_amount != Uint128::new(THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!(
                "BlueChip amount must be {}",
                THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS
            ),
        });
    }
    if params.pool_seed_amount != Uint128::new(THRESHOLD_PAYOUT_POOL_BASE_UNITS) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", THRESHOLD_PAYOUT_POOL_BASE_UNITS),
        });
    }
    if params.commit_return_amount != Uint128::new(THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!(
                "Commit amount must be {}",
                THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS
            ),
        });
    }

    let total = params
        .creator_reward_amount
        .checked_add(params.bluechip_reward_amount)?
        .checked_add(params.pool_seed_amount)?
        .checked_add(params.commit_return_amount)?;
    if total != Uint128::new(THRESHOLD_PAYOUT_TOTAL_BASE_UNITS) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!(
                "Total must equal {} (got {})",
                THRESHOLD_PAYOUT_TOTAL_BASE_UNITS, total
            ),
        });
    }

    Ok(())
}

/// Output of `trigger_threshold_payout`. The factory notification is
/// separated from the rest of the payout messages because we want it
/// delivered via `SubMsg::reply_on_error` — a failure there should NOT
/// revert the pool-side threshold-crossing state. The caller splices
/// `factory_notify` in as a SubMsg and `other_msgs` as plain
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
) -> Result<ThresholdPayoutMsgs, ContractError> {
    // No-double-mint invariant. The two crossing handlers
    // (`process_threshold_crossing_with_excess` and
    // `process_threshold_hit_exact`) gate on `IS_THRESHOLD_HIT == false`
    // before any state mutation and then set the flag BEFORE calling
    // here (so subsequent commits route to the post-threshold AMM-swap
    // path inside the same block). That means by the time we reach this
    // function the flag is already `true` on the canonical path —
    // duplicating the gate here would trip the happy path. The
    // invariant is therefore enforced by the two upstream entry gates
    // alone; this comment is a load-bearing pointer for anyone tracing
    // the no-double-mint argument later.

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

    // Backstop NFT-ownership accept. Under the canonical create flow
    // the factory's `finalize_pool` dispatches `AcceptNftOwnership {}`
    // to this pool in the same tx as the CW721 `TransferOwnership`, so
    // `nft_ownership_accepted` is already true by the time threshold
    // crosses and this branch is a no-op. Retained as defense-in-depth
    // for the test-fixture path (and any hypothetical future code path
    // that instantiates a pool directly) where the factory-side
    // dispatch may not have run; the deposit handler in pool-core
    // carries the same idempotent fallback.
    //
    // Idempotent: the `if !nft_ownership_accepted` gate makes a second
    // accept a no-op — important because the CW721 contract rejects a
    // duplicate `AcceptOwnership` with `NoPendingOwner` and that error
    // would revert the entire threshold-cross tx.
    if !pool_state.nft_ownership_accepted {
        other_msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.position_nft_address.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::<()>::UpdateOwnership(
                    pool_factory_interfaces::cw721_msgs::Action::AcceptOwnership,
                ),
            )?,
            funds: vec![],
        }));
        pool_state.nft_ownership_accepted = true;
    }

    // Runtime sanity check that the four payout components add up to the
    // canonical 1.2T total. Mirrors `validate_pool_threshold_payments`
    // (instantiate-time gate) so a corrupted `THRESHOLD_PAYOUT_AMOUNTS`
    // record from a buggy migration is caught here rather than producing
    // a silently-skewed mint.
    let total = payout
        .creator_reward_amount
        .checked_add(payout.bluechip_reward_amount)?
        .checked_add(payout.pool_seed_amount)?
        .checked_add(payout.commit_return_amount)?;

    if total != Uint128::new(THRESHOLD_PAYOUT_TOTAL_BASE_UNITS) {
        return Err(ContractError::ThresholdPayoutCorruption);
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
            distributed_so_far: cosmwasm_std::Uint128::zero(),
        };
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
    }

    // Audit fix: NATIVE_RAISED_FROM_COMMIT is now stored as net-of-fees
    // by every commit handler, so the seed amount is read out directly
    // with no recovery math. Previously the field stored gross and was
    // recovered via `gross * (1 - fee_rate)` floor here, which combined
    // with the per-commit fee floor to leave up to ~2 units stranded
    // per commit. The keeper bounty for distribution batches is paid
    // by the factory from its own reserve, not skimmed from LP funds —
    // see `factory::execute_pay_distribution_bounty`.
    let pools_bluechip_seed = crate::state::NATIVE_RAISED_FROM_COMMIT.load(storage)?;

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
                    .plus_seconds(commit_config.creator_excess_liquidity_lock_days * SECONDS_PER_DAY),
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
