//! Threshold-crossing commit handler. Fires when a single commit carries
//! the pool over its `commit_amount_for_threshold_usd` target.
//!
//! Responsibilities (in order):
//!   1. Split the incoming commit into a threshold portion (up to the
//!      remaining target) and an excess portion.
//!   2. Credit the threshold portion to `COMMIT_LEDGER` +
//!      `USD_RAISED_FROM_COMMIT` / `NATIVE_RAISED_FROM_COMMIT`, flip
//!      `IS_THRESHOLD_HIT`, and run the payout (seeds LP reserves, sends
//!      creator tokens, schedules the factory notify SubMsg).
//!   3. Swap the excess through the freshly-seeded pool, capped at 3% of
//!      the new bluechip reserve to prevent the crosser from capturing a
//!      structural MEV trade. Whatever is above the cap is refunded.
//!   4. Update commit analytics and clear `THRESHOLD_PROCESSING` so the
//!      next commit can proceed.
//!
//! The factory-notify message is attached as a SubMsg (not a plain
//! CosmosMsg) so a failure on the factory side is recoverable via
//! `RetryFactoryNotify` rather than reverting the whole crossing tx.

use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, Response, Uint128, WasmMsg,
};
use cw20::Cw20ExecuteMsg;

use crate::asset::{get_native_denom, TokenInfo};
use crate::error::ContractError;
use crate::generic_helpers::{
    get_bank_transfer_to_msg, trigger_threshold_payout, update_commit_info, update_pool_fee_growth,
};
use crate::msg::CommitFeeInfo;
use crate::state::{
    CommitLimitInfo, PoolFeeState, PoolInfo, PoolSpecs, PoolState, ThresholdPayoutAmounts,
    COMMIT_LEDGER, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, POOL_ANALYTICS, POOL_FEE_STATE,
    POOL_STATE, POST_THRESHOLD_COOLDOWN_BLOCKS, POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK,
    THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::swap_helper::{
    assert_max_spread, compute_swap, update_price_accumulator, usd_to_bluechip_at_rate,
};

use super::commit_base_attributes;

#[allow(clippy::too_many_arguments)]
pub(super) fn process_threshold_crossing_with_excess(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &TokenInfo,
    amount: Uint128,
    amount_after_fees: Uint128,
    usd_value: Uint128,
    usd_to_threshold: Uint128,
    oracle_rate: Uint128,
    pool_state: &mut PoolState,
    pool_fee_state: &mut PoolFeeState,
    pool_specs: &PoolSpecs,
    pool_info: &PoolInfo,
    commit_config: &CommitLimitInfo,
    threshold_payout: &ThresholdPayoutAmounts,
    fee_info: &CommitFeeInfo,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    // Reuse the rate captured at commit() entry rather than re-querying the
    // oracle (P4-M6). usd_to_bluechip_at_rate is the inverse of the
    // bluechip_to_usd math used to produce usd_value, so thresholding is
    // arithmetically consistent with the valuation.
    let bluechip_to_threshold = usd_to_bluechip_at_rate(usd_to_threshold, oracle_rate)?;
    let bluechip_excess = asset.amount.checked_sub(bluechip_to_threshold)?;

    let threshold_portion_after_fees = if amount.is_zero() {
        Uint128::zero()
    } else {
        amount_after_fees.multiply_ratio(bluechip_to_threshold, amount)
    };
    let effective_bluechip_excess = amount_after_fees.checked_sub(threshold_portion_after_fees)?;
    let usd_excess = usd_value.checked_sub(usd_to_threshold)?;

    // Update commit ledger with only the threshold portion
    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default().checked_add(usd_to_threshold)?)
    })?;
    USD_RAISED_FROM_COMMIT.save(deps.storage, &commit_config.commit_amount_for_threshold_usd)?;
    NATIVE_RAISED_FROM_COMMIT
        .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(bluechip_to_threshold)?))?;

    IS_THRESHOLD_HIT.save(deps.storage, &true)?;
    // Arm the post-threshold cooldown. The crosser's own bounded excess
    // swap (capped at 3% of seeded reserve below) executes in this same
    // tx — the gate sits on simple_swap / execute_swap_cw20 /
    // process_post_threshold_commit, none of which run inside the
    // crossing tx. So the crosser's privileged excess is unaffected,
    // while every follower trade in this block plus the next
    // POST_THRESHOLD_COOLDOWN_BLOCKS blocks is rejected with
    // PostThresholdCooldownActive.
    POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK.save(
        deps.storage,
        &(env.block.height + POST_THRESHOLD_COOLDOWN_BLOCKS + 1),
    )?;

    // Hold factory_notify aside; it becomes a SubMsg on the final Response
    // so a factory-side failure is recoverable via RetryFactoryNotify
    // rather than reverting the whole threshold crossing (P4-H5).
    let payout_msgs = trigger_threshold_payout(
        deps.storage,
        pool_info,
        pool_state,
        pool_fee_state,
        commit_config,
        threshold_payout,
        fee_info,
        &env,
    )?;
    messages.extend(payout_msgs.other_msgs);
    let factory_notify = payout_msgs.factory_notify;

    update_commit_info(
        deps.storage,
        &sender,
        &pool_state.pool_contract_address,
        bluechip_to_threshold,
        usd_to_threshold,
        env.block.time,
    )?;

    // Process the excess as a swap, capped at 3% of pool reserves to keep
    // the threshold-crosser from capturing a disproportionate share of the
    // freshly-seeded pool on a single atomic tx. The cap was previously 20%,
    // which turned every threshold crossing into a guaranteed MEV bonanza
    // (~20% of all newly-minted creator tokens at seed price, front-run by
    // anyone with gas). Dropping to 3% removes the structural free trade
    // while still letting a modest overshoot settle in the same tx rather
    // than requiring a full refund + manual re-swap.
    let mut return_amt = Uint128::zero();
    let mut spread_amt = Uint128::zero();
    let mut commission_amt = Uint128::zero();
    let mut refunded_excess = Uint128::zero();
    let mut capped_excess = Uint128::zero();

    if effective_bluechip_excess > Uint128::zero() {
        // `trigger_threshold_payout` above mutated pool_state/pool_fee_state
        // in place AND saved them to storage, so the caller-provided refs
        // already reflect the post-seed pool. No reload needed; we modify
        // the refs directly through the swap and save once at the end.
        let offer_pool = pool_state.reserve0;
        let ask_pool = pool_state.reserve1;

        // Cap the excess swap at 3% of the freshly seeded bluechip reserve.
        // Any remainder is refunded to the sender — they can swap it in
        // subsequent transactions where other participants can also trade.
        let max_excess_swap = offer_pool.multiply_ratio(3u128, 100u128);
        capped_excess = effective_bluechip_excess.min(max_excess_swap);
        refunded_excess = effective_bluechip_excess.checked_sub(capped_excess)?;

        if !ask_pool.is_zero() && !offer_pool.is_zero() && !capped_excess.is_zero() {
            let (ret, sp, comm) =
                compute_swap(offer_pool, ask_pool, capped_excess, pool_specs.lp_fee)?;
            return_amt = ret;
            spread_amt = sp;
            commission_amt = comm;
        }

        if !capped_excess.is_zero() {
            // Unconditional slippage protection on the threshold-crossing
            // excess swap. Previously gated on max_spread.is_some(), which
            // meant callers who omitted max_spread skipped the check
            // entirely.
            //
            // With the excess cap now at 3% of the freshly-seeded bluechip
            // reserve, the maximum honest x*y=k spread on this swap is
            // ~3% as well. 5% gives a small buffer for rounding / fee
            // interaction without leaving the previous 25% gaping hole
            // that let front-runners sandwich the crossing tx.
            //
            // Users who explicitly set a tighter `max_spread` get that
            // stricter bound honored; callers who forgot to specify one
            // get 5% instead of no protection at all.
            let effective_max_spread = max_spread.or(Some(Decimal::percent(5)));
            assert_max_spread(
                belief_price,
                effective_max_spread,
                capped_excess,
                return_amt.checked_add(commission_amt)?,
                spread_amt,
            )?;
        }

        update_price_accumulator(pool_state, env.block.time.seconds())?;

        pool_state.reserve0 = offer_pool.checked_add(capped_excess)?;
        pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

        update_pool_fee_growth(pool_fee_state, pool_state, 0, commission_amt)?;
        POOL_FEE_STATE.save(deps.storage, pool_fee_state)?;
        POOL_STATE.save(deps.storage, pool_state)?;

        if !return_amt.is_zero() {
            messages.push(
                WasmMsg::Execute {
                    contract_addr: pool_info.token_address.to_string(),
                    msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                        recipient: sender.to_string(),
                        amount: return_amt,
                    })?,
                    funds: vec![],
                }
                .into(),
            );
        }

        // Refund the capped portion back to the sender
        if !refunded_excess.is_zero() {
            let bluechip_denom = get_native_denom(&pool_info.pool_info.asset_infos)?;
            messages.push(get_bank_transfer_to_msg(
                &sender,
                &bluechip_denom,
                refunded_excess,
            )?);
        }

        update_commit_info(
            deps.storage,
            &sender,
            &pool_state.pool_contract_address,
            bluechip_excess.checked_sub(refunded_excess)?,
            usd_excess,
            env.block.time,
        )?;
    }

    THRESHOLD_PROCESSING.save(deps.storage, &false)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
    analytics.total_commit_count += 1;
    if !capped_excess.is_zero() && !return_amt.is_zero() {
        analytics.total_swap_count += 1;
        analytics.total_volume_0 = analytics.total_volume_0.saturating_add(capped_excess);
        analytics.total_volume_1 = analytics.total_volume_1.saturating_add(return_amt);
        analytics.last_trade_block = env.block.height;
        analytics.last_trade_timestamp = env.block.time.seconds();
    }
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    // `pool_state` (outer &mut ref) already reflects the committed on-chain
    // state after trigger_threshold_payout + the optional excess-swap block
    // above. Previous code reloaded here; the reload was redundant and
    // cost an extra storage read per threshold-crossing tx.
    let base = commit_base_attributes(
        "threshold_crossing",
        &sender,
        &pool_state.pool_contract_address,
        analytics.total_commit_count,
        &env,
    );
    Ok(Response::new()
        .add_submessage(factory_notify)
        .add_messages(messages)
        .add_attributes(base)
        .add_attribute("total_amount_bluechip", asset.amount.to_string())
        .add_attribute(
            "threshold_amount_bluechip",
            bluechip_to_threshold.to_string(),
        )
        .add_attribute("swap_amount_bluechip", capped_excess.to_string())
        .add_attribute(
            "swap_amount_bluechip_pre_cap",
            effective_bluechip_excess.to_string(),
        )
        .add_attribute("threshold_amount_usd", usd_to_threshold.to_string())
        .add_attribute("swap_amount_usd", usd_excess.to_string())
        .add_attribute("bluechip_excess_spread", spread_amt.to_string())
        .add_attribute("bluechip_excess_returned", return_amt.to_string())
        .add_attribute("bluechip_excess_commission", commission_amt.to_string())
        .add_attribute("bluechip_excess_refunded", refunded_excess.to_string())
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string()))
}
