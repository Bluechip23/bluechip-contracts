//! Commit logic: pre-threshold funding, threshold crossing, post-threshold
//! swaps, and distribution batch processing.

use crate::admin::ensure_not_drained;
use crate::asset::TokenInfo;
use crate::error::ContractError;
use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, get_bank_transfer_to_msg,
    process_distribution_batch, trigger_threshold_payout, update_commit_info,
    update_pool_fee_growth,
};
use crate::state::{
    PoolState, COMMITFEEINFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO, DISTRIBUTION_STATE,
    IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT, NATIVE_RAISED_FROM_COMMIT, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE, REENTRANCY_LOCK,
    THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::swap_helper::{
    assert_max_spread, compute_swap, get_bluechip_value, get_usd_value_with_staleness_check,
    update_price_accumulator,
};
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo, Response, StdError,
    Timestamp, Uint128, WasmMsg,
};
use cw20::Cw20ExecuteMsg;

use crate::asset::{get_bluechip_denom, TokenType};

// Minimum commit value in USD (6 decimals). $1 = 1_000_000.
// Prevents dust commit griefing that bloats COMMIT_LEDGER and distribution.
pub const MIN_COMMIT_USD: Uint128 = Uint128::new(1_000_000);

pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    transaction_deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    ensure_not_drained(deps.storage)?;
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    // Reentrancy protection
    let reentrancy_guard = REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;

    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        REENTRANCY_LOCK.save(deps.storage, &false)?;
        return Err(e);
    }

    let result = execute_commit_logic(
        &mut deps,
        env,
        info,
        asset,
        belief_price,
        max_spread,
    );
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
}

fn execute_commit_logic(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let amount = asset.amount;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let threshold_payout = THRESHOLD_PAYOUT_AMOUNTS.load(deps.storage)?;
    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let sender = info.sender.clone();

    // Validate asset type
    if !asset.info.equal(&pool_info.pool_info.asset_infos[0])
        && !asset.info.equal(&pool_info.pool_info.asset_infos[1])
    {
        return Err(ContractError::AssetMismatch {});
    }
    if asset.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    let usd_value =
        get_usd_value_with_staleness_check(deps.as_ref(), asset.amount, env.block.time.seconds())?;
    if usd_value.is_zero() {
        return Err(ContractError::InvalidOraclePrice {});
    }
    if usd_value < MIN_COMMIT_USD {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Commit too small: ${} USD (minimum $1 USD)",
            usd_value
        ))));
    }

    let bluechip_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;

    match &asset.info {
        TokenType::Bluechip { denom } if denom == &bluechip_denom => {
            // Verify funds were actually sent
            let sent = info
                .funds
                .iter()
                .find(|c| c.denom == denom.as_str())
                .map(|c| c.amount)
                .unwrap_or_default();
            if sent < amount {
                return Err(ContractError::MismatchAmount {});
            }

            // Calculate fees
            let (commit_fee_bluechip_amt, commit_fee_creator_amt) =
                calculate_commit_fees(amount, &fee_info)?;
            let total_fees = commit_fee_bluechip_amt.checked_add(commit_fee_creator_amt)?;
            if total_fees >= amount {
                return Err(ContractError::InvalidFee {});
            }
            let amount_after_fees = amount.checked_sub(total_fees)?;
            if amount_after_fees.is_zero() {
                return Err(ContractError::InvalidFee {});
            }

            // Build fee transfer messages
            let mut messages = build_fee_messages(
                &fee_info,
                denom,
                commit_fee_bluechip_amt,
                commit_fee_creator_amt,
            )?;

            let threshold_already_hit = IS_THRESHOLD_HIT.load(deps.storage)?;

            if !threshold_already_hit {
                let current_usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
                let new_total = current_usd_raised.checked_add(usd_value)?;

                if new_total >= commit_config.commit_amount_for_threshold_usd {
                    LAST_THRESHOLD_ATTEMPT.save(deps.storage, &env.block.time)?;

                    let processing = THRESHOLD_PROCESSING
                        .may_load(deps.storage)?
                        .unwrap_or(false);
                    let can_process = if processing {
                        false
                    } else {
                        THRESHOLD_PROCESSING.save(deps.storage, &true)?;
                        true
                    };

                    if !can_process {
                        if IS_THRESHOLD_HIT.load(deps.storage)? {
                            return process_post_threshold_commit(
                                deps,
                                env,
                                sender,
                                asset,
                                amount_after_fees,
                                usd_value,
                                messages,
                                belief_price,
                                max_spread,
                            );
                        }
                        return process_pre_threshold_commit(
                            deps, env, sender, &asset, usd_value, messages,
                        );
                    }

                    // Calculate exact amounts for threshold crossing
                    let usd_to_threshold = commit_config
                        .commit_amount_for_threshold_usd
                        .checked_sub(current_usd_raised)
                        .unwrap_or(Uint128::zero());

                    if usd_value > usd_to_threshold && usd_to_threshold > Uint128::zero() {
                        // Split commit: part goes to threshold, excess becomes swap
                        process_threshold_crossing_with_excess(
                            deps,
                            env,
                            sender,
                            &asset,
                            amount,
                            amount_after_fees,
                            usd_value,
                            usd_to_threshold,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &pool_specs,
                            &pool_info,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            messages,
                            belief_price,
                            max_spread,
                        )
                    } else {
                        // Threshold hit exactly
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default().checked_add(usd_value)?)
                        })?;
                        let final_usd =
                            new_total.min(commit_config.commit_amount_for_threshold_usd);
                        USD_RAISED_FROM_COMMIT.save(deps.storage, &final_usd)?;
                        NATIVE_RAISED_FROM_COMMIT
                            .update::<_, ContractError>(deps.storage, |r| {
                                Ok(r.checked_add(asset.amount)?)
                            })?;
                        IS_THRESHOLD_HIT.save(deps.storage, &true)?;

                        messages.extend(trigger_threshold_payout(
                            deps.storage,
                            &pool_info,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            &env,
                        )?);
                        update_commit_info(
                            deps.storage,
                            &sender,
                            pool_state.pool_contract_address.clone(),
                            asset.amount,
                            usd_value,
                            env.block.time,
                        )?;
                        THRESHOLD_PROCESSING.save(deps.storage, &false)?;

                        // Update analytics
                        let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
                        analytics.total_commit_count += 1;
                        POOL_ANALYTICS.save(deps.storage, &analytics)?;

                        Ok(Response::new()
                            .add_messages(messages)
                            .add_attribute("action", "commit")
                            .add_attribute("phase", "threshold_hit_exact")
                            .add_attribute("committer", sender)
                            .add_attribute("commit_amount_bluechip", asset.amount.to_string())
                            .add_attribute("commit_amount_usd", usd_value.to_string())
                            .add_attribute("total_usd_raised_after", new_total.to_string())
                            .add_attribute(
                                "total_commit_count",
                                analytics.total_commit_count.to_string(),
                            )
                            .add_attribute(
                                "pool_contract",
                                pool_state.pool_contract_address.to_string(),
                            )
                            .add_attribute("block_height", env.block.height.to_string())
                            .add_attribute("block_time", env.block.time.seconds().to_string()))
                    }
                } else {
                    process_pre_threshold_commit(deps, env, sender, &asset, usd_value, messages)
                }
            } else {
                process_post_threshold_commit(
                    deps,
                    env,
                    sender,
                    asset,
                    amount_after_fees,
                    usd_value,
                    messages,
                    belief_price,
                    max_spread,
                )
            }
        }
        _ => Err(ContractError::AssetMismatch {}),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

use crate::msg::CommitFeeInfo;
use cosmwasm_std::Fraction;

/// Calculate both fee portions for a commit. Returns (bluechip_fee, creator_fee).
fn calculate_commit_fees(
    amount: Uint128,
    fee_info: &CommitFeeInfo,
) -> Result<(Uint128, Uint128), ContractError> {
    let bluechip_fee = amount
        .checked_mul(fee_info.commit_fee_bluechip.numerator())?
        .checked_div(fee_info.commit_fee_bluechip.denominator())
        .map_err(|_| ContractError::DivideByZero)?;
    let creator_fee = amount
        .checked_mul(fee_info.commit_fee_creator.numerator())?
        .checked_div(fee_info.commit_fee_creator.denominator())
        .map_err(|_| ContractError::DivideByZero)?;
    Ok((bluechip_fee, creator_fee))
}

/// Build bank-send messages for the two fee recipients.
fn build_fee_messages(
    fee_info: &CommitFeeInfo,
    denom: &str,
    bluechip_fee: Uint128,
    creator_fee: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut messages = Vec::new();
    if !bluechip_fee.is_zero() {
        messages.push(get_bank_transfer_to_msg(
            &fee_info.bluechip_wallet_address,
            denom,
            bluechip_fee,
        )?);
    }
    if !creator_fee.is_zero() {
        messages.push(get_bank_transfer_to_msg(
            &fee_info.creator_wallet_address,
            denom,
            creator_fee,
        )?);
    }
    Ok(messages)
}

fn process_pre_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &TokenInfo,
    usd_value: Uint128,
    messages: Vec<CosmosMsg>,
) -> Result<Response, ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;

    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default().checked_add(usd_value)?)
    })?;
    USD_RAISED_FROM_COMMIT
        .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(usd_value)?))?;
    NATIVE_RAISED_FROM_COMMIT
        .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(asset.amount)?))?;

    update_commit_info(
        deps.storage,
        &sender,
        pool_state.pool_contract_address.clone(),
        asset.amount,
        usd_value,
        env.block.time,
    )?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_commit_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let total_usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
    let total_bluechip_raised = NATIVE_RAISED_FROM_COMMIT.load(deps.storage)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "funding")
        .add_attribute("committer", sender)
        .add_attribute("commit_amount_bluechip", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string())
        .add_attribute("total_usd_raised_after", total_usd_raised.to_string())
        .add_attribute(
            "total_bluechip_raised_after",
            total_bluechip_raised.to_string(),
        )
        .add_attribute(
            "total_commit_count",
            analytics.total_commit_count.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

#[allow(clippy::too_many_arguments)]
fn process_post_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: TokenInfo,
    swap_amount: Uint128,
    usd_value: Uint128,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    let offer_pool = pool_state.reserve0;
    let ask_pool = pool_state.reserve1;

    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, swap_amount, pool_specs.lp_fee)?;

    // Dust-swap guard: mirror simple_swap's zero-return rejection so a
    // post-threshold commit that would consume the user's bluechip
    // without yielding any creator tokens fails loudly.
    if return_amt.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    assert_max_spread(
        belief_price,
        max_spread,
        swap_amount,
        return_amt,
        spread_amt,
    )?;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

    pool_state.reserve0 = offer_pool.checked_add(swap_amount)?;
    pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

    update_pool_fee_growth(&mut pool_fee_state, &pool_state, 0, commission_amt)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;

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

    update_commit_info(
        deps.storage,
        &sender,
        pool_state.pool_contract_address.clone(),
        asset.amount,
        usd_value,
        env.block.time,
    )?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_commit_count += 1;
    analytics.total_swap_count += 1;
    analytics.total_volume_0 = analytics.total_volume_0.saturating_add(swap_amount);
    analytics.total_volume_1 = analytics.total_volume_1.saturating_add(return_amt);
    analytics.last_trade_block = env.block.height;
    analytics.last_trade_timestamp = env.block.time.seconds();
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    // Effective price: creator tokens received per bluechip spent
    let effective_price = if !swap_amount.is_zero() {
        Decimal::from_ratio(return_amt, swap_amount).to_string()
    } else {
        "0".to_string()
    };

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "active")
        .add_attribute("committer", sender)
        .add_attribute("commit_amount_bluechip", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string())
        .add_attribute("swap_amount_bluechip", swap_amount.to_string())
        .add_attribute("tokens_received", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount", commission_amt.to_string())
        .add_attribute("effective_price", effective_price)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_commit_count",
            analytics.total_commit_count.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

#[allow(clippy::too_many_arguments)]
fn process_threshold_crossing_with_excess(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &TokenInfo,
    amount: Uint128,
    amount_after_fees: Uint128,
    usd_value: Uint128,
    usd_to_threshold: Uint128,
    pool_state: &mut PoolState,
    pool_fee_state: &mut crate::state::PoolFeeState,
    pool_specs: &crate::state::PoolSpecs,
    pool_info: &crate::state::PoolInfo,
    commit_config: &crate::state::CommitLimitInfo,
    threshold_payout: &crate::state::ThresholdPayoutAmounts,
    fee_info: &CommitFeeInfo,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let bluechip_to_threshold = get_bluechip_value(deps.as_ref(), usd_to_threshold)?;
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

    messages.extend(trigger_threshold_payout(
        deps.storage,
        pool_info,
        pool_state,
        pool_fee_state,
        commit_config,
        threshold_payout,
        fee_info,
        &env,
    )?);

    update_commit_info(
        deps.storage,
        &sender,
        pool_state.pool_contract_address.clone(),
        bluechip_to_threshold,
        usd_to_threshold,
        env.block.time,
    )?;

    // Process the excess as a swap, capped at 20% of pool reserves to prevent
    // a single whale from dominating the first trade on a freshly seeded pool.
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

        // Cap the excess swap at 20% of the freshly seeded bluechip reserve.
        // Any remainder is refunded to the sender — they can swap it in
        // subsequent transactions where other participants can also trade.
        let max_excess_swap = offer_pool.multiply_ratio(20u128, 100u128);
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
            // The path-aware default (25%) instead of the pool-wide
            // DEFAULT_SLIPPAGE (0.5%) reflects the structural reality of
            // this swap: the excess can be up to 20% of the freshly-seeded
            // pool's reserves, which by x*y=k math produces an inherent
            // spread of ~15–20% even under honest conditions. A 0.5% cap
            // would revert virtually every real threshold crossing with
            // non-trivial excess. 25% gives a small buffer over the 20%
            // design ceiling: anything worse than that indicates either
            // a bug, a pathological pool seed, or that the excess cap
            // wasn't applied — all cases the caller should know about.
            //
            // Users who explicitly set a tighter `max_spread` get that
            // stricter bound honored; callers who forgot to specify one
            // get 25% instead of no protection at all.
            let effective_max_spread = max_spread.or(Some(Decimal::percent(25)));
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
            let bluechip_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;
            messages.push(get_bank_transfer_to_msg(
                &sender,
                &bluechip_denom,
                refunded_excess,
            )?);
        }

        update_commit_info(
            deps.storage,
            &sender,
            pool_state.pool_contract_address.clone(),
            bluechip_excess.checked_sub(refunded_excess)?,
            usd_excess,
            env.block.time,
        )?;
    }

    THRESHOLD_PROCESSING.save(deps.storage, &false)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
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
    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "threshold_crossing")
        .add_attribute("committer", sender)
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
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_commit_count",
            analytics.total_commit_count.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Distribution
// ---------------------------------------------------------------------------

pub fn execute_continue_distribution(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Defense in depth: emergency_withdraw flips is_distributing=false on
    // drain (admin.rs::execute_emergency_withdraw), so the load+check below
    // already rejects calls on drained pools — but reading EMERGENCY_DRAINED
    // up front fails early with the canonical error and avoids the keeper
    // ever issuing a tx against a drained pool.
    ensure_not_drained(deps.storage)?;

    let dist_state = DISTRIBUTION_STATE.load(deps.storage)?;
    if !dist_state.is_distributing {
        return Err(ContractError::NothingToRecover {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;

    let (mut msgs, processed_count) =
        process_distribution_batch(deps.storage, &pool_info, &env)?;

    // Bounty paid by the factory from its own reserve, not pool LP funds.
    // Only emit the PayDistributionBounty message when this call actually
    // processed at least one committer. An empty/no-op call (cursor past
    // end, stale-state cleanup) must not earn a bounty: it would let a
    // keeper farm the factory reserve for zero work, and the factory's
    // bounty cap doesn't gate frequency the way the oracle cooldown does.
    if processed_count > 0 {
        // Factory rejects unregistered pools, which reverts this whole tx —
        // desired behavior since only legitimate pools should pay bounties.
        msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.factory_addr.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::FactoryExecuteMsg::PayDistributionBounty {
                    recipient: info.sender.to_string(),
                },
            )?,
            funds: vec![],
        }));
    }

    // process_distribution_batch may have either removed the state
    // entirely (genuine completion) or flipped is_distributing=false
    // (recovery path after repeated failures). Treat both as "stop
    // calling this pool" from the keeper's perspective.
    let (remaining_after, is_complete) = match DISTRIBUTION_STATE.may_load(deps.storage)? {
        None => (0u32, true),
        Some(d) => (d.distributions_remaining, !d.is_distributing),
    };

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "continue_distribution")
        .add_attribute("caller", info.sender.to_string())
        .add_attribute("processed_count", processed_count.to_string())
        .add_attribute("bounty_paid", (processed_count > 0).to_string())
        .add_attribute(
            "remaining_before",
            dist_state.distributions_remaining.to_string(),
        )
        .add_attribute("remaining_after", remaining_after.to_string())
        .add_attribute("distribution_complete", is_complete.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}
