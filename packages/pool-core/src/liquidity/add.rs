//! `add_to_position`: top up an existing LP position with additional
//! liquidity. Fees accrued on the position since the last collection
//! are settled first (treated as a collect+reset) so the new share is
//! valued against a clean baseline.
//!
//! Two-layer API:
//! - `execute_add_to_position` — public entry point, does reentrancy /
//! rate-limit checks and delegates.
//! - `add_to_position`         — core handler, exposed `pub` so
//! downstream crates (and future helpers) can call it without the
//! rate-limit layer if they've already handled rate limiting.

use cosmwasm_std::{
    Addr, CosmosMsg, DepsMut, Env, MessageInfo, Response, Timestamp, Uint128,
};

use crate::error::ContractError;
use crate::generic::{check_rate_limit, enforce_transaction_deadline, with_reentrancy_guard};
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees_with_clip, effective_fee_size_multiplier,
    enforce_standard_pool_min_position, sync_position_on_transfer, verify_position_ownership,
};
use crate::state::{
    PoolSpecs, CREATOR_FEE_POT, LIQUIDITY_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE, POOL_SPECS,
    POOL_STATE,
};
use crate::swap::update_price_accumulator;

use super::deposit::{finalize_deposit_response, prepare_deposit, snapshot_pool_cw20_balances};

#[allow(clippy::too_many_arguments)]
pub fn add_to_position(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    position_id: String,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    add_to_position_internal(
        deps,
        env,
        info,
        user,
        position_id,
        amount0,
        amount1,
        min_amount0,
        min_amount1,
        transaction_deadline,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn add_to_position_internal(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    position_id: String,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
    verify_balances: bool,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let prep = prepare_deposit(
        deps.as_ref(),
        &info,
        amount0,
        amount1,
        min_amount0,
        min_amount1,
    )?;

    // Standard-pool dust-floor on the produced LP units. Mirrors the
    // check inside `execute_deposit_liquidity_inner`. No-op on creator
    // pools. Applied here rather than only on initial deposit because a
    // tiny add-to-position on a standard pool would otherwise still
    // produce a low-fee_size_multiplier in the pre-fix world; with the
    // multiplier now pinned at 1.0 on standard pools, the floor is the
    // only remaining dust-griefing deterrent and must apply at every
    // liquidity-in entry point.
    enforce_standard_pool_min_position(deps.storage, prep.liquidity)?;

    // Same pre-snapshot pattern as `execute_deposit_liquidity_inner`.
    // Skipped when verify_balances=false (creator-pool path) — saves the
    // two CW20 balance queries per add-to-position call.
    let pre_snapshot = if verify_balances {
        Some(snapshot_pool_cw20_balances(
            deps.as_ref(),
            &prep.pool_info.pool_info.contract_addr,
            &prep.pool_info.pool_info.asset_infos,
        )?)
    } else {
        None
    };

    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    verify_position_ownership(
        deps.as_ref(),
        &prep.pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;
    // Collect pending fees before adding new liquidity to reset accounting.
    let ((fees_owed_0, fees_owed_1), _, (clipped_0, clipped_1)) =
        calc_capped_fees_with_clip(&liquidity_position, &pool_fee_state)?;

    let mut messages: Vec<CosmosMsg> = prep.collect_msgs.clone();

    liquidity_position.liquidity = liquidity_position.liquidity.checked_add(prep.liquidity)?;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_size_multiplier =
        effective_fee_size_multiplier(deps.storage, liquidity_position.liquidity)?;
    liquidity_position.unclaimed_fees_0 = Uint128::zero();
    liquidity_position.unclaimed_fees_1 = Uint128::zero();

    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(prep.liquidity)?;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_fee_state.fee_reserve_0 = pool_fee_state
        .fee_reserve_0
        .checked_sub(fees_owed_0)?
        .checked_sub(clipped_0)?;
    pool_fee_state.fee_reserve_1 = pool_fee_state
        .fee_reserve_1
        .checked_sub(fees_owed_1)?
        .checked_sub(clipped_1)?;

    let mut pot = CREATOR_FEE_POT
        .may_load(deps.storage)?
        .unwrap_or_default();
    pot.amount_0 = pot.amount_0.checked_add(clipped_0)?;
    pot.amount_1 = pot.amount_1.checked_add(clipped_1)?;
    CREATOR_FEE_POT.save(deps.storage, &pot)?;

    pool_state.reserve0 = pool_state.reserve0.checked_add(prep.actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(prep.actual_amount1)?;

    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
    analytics.total_lp_deposit_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let attrs = vec![
        ("action", "add_to_position".to_string()),
        ("position_id", position_id),
        ("depositor", user.to_string()),
        ("additional_liquidity", prep.liquidity.to_string()),
        ("total_liquidity", liquidity_position.liquidity.to_string()),
        ("amount0_requested", amount0.to_string()),
        ("amount1_requested", amount1.to_string()),
        ("actual_amount0_added", prep.actual_amount0.to_string()),
        ("actual_amount1_added", prep.actual_amount1.to_string()),
        ("refunded_amount0", prep.refund_amount0.to_string()),
        ("refunded_amount1", prep.refund_amount1.to_string()),
        ("fees_collected_0", fees_owed_0.to_string()),
        ("fees_collected_1", fees_owed_1.to_string()),
        ("reserve0_after", pool_state.reserve0.to_string()),
        ("reserve1_after", pool_state.reserve1.to_string()),
        ("total_liquidity_after", pool_state.total_liquidity.to_string()),
        ("pool_contract", pool_state.pool_contract_address.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
        ("total_lp_deposit_count", analytics.total_lp_deposit_count.to_string()),
    ];
    let fee_msgs = build_fee_transfer_msgs(&prep.pool_info, &user, fees_owed_0, fees_owed_1)?;
    messages.extend(fee_msgs);

    finalize_deposit_response(
        deps.storage,
        &prep.pool_info,
        &prep.pool_info.pool_info.asset_infos,
        prep.actual_amount0,
        prep.actual_amount1,
        pre_snapshot,
        messages,
        attrs,
    )
}

/// Public entry point — used by creator-pool. No CW20 balance
/// verification: the freshly-minted cw20-base token is trusted.
#[allow(clippy::too_many_arguments)]
pub fn execute_add_to_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    execute_add_to_position_dispatch(
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
        false,
    )
}

/// Variant used by standard-pool. Same SubMsg-based balance
/// verification as `execute_deposit_liquidity_with_verify`; reverts the
/// transaction when an arbitrary CW20 charged transfer fees or rebased.
#[allow(clippy::too_many_arguments)]
pub fn execute_add_to_position_with_verify(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    execute_add_to_position_dispatch(
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
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_add_to_position_dispatch(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
    verify_balances: bool,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    // Shared reentrancy guard. Same lock as commit/swap/deposit so a
    // hostile CW20's transfer hook can't reach this handler from any
    // other path, and vice versa.
    with_reentrancy_guard(deps, move |mut deps| {
        let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
        check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
        add_to_position_internal(
            &mut deps,
            env,
            info,
            sender,
            position_id,
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
            verify_balances,
        )
    })
}
