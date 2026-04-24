//! LP-position exit handlers: full removal, partial absolute removal,
//! and partial percentage removal. All three share the same settle-fees
//! -> compute-payout -> debit-reserves -> transfer-out sequence; the
//! two core bodies live in `remove_all_liquidity` and
//! `remove_partial_liquidity`, with thin rate-limited `execute_*`
//! wrappers for external callers.
//!
//! `execute_remove_partial_liquidity_by_percent` is a convenience shim
//! that computes `liquidity_to_remove = position.liquidity * pct / 100`
//! and delegates, short-circuiting to full removal for pct >= 100 so
//! the position NFT is burned cleanly on "give me all of it".

use cosmwasm_std::{DepsMut, Env, MessageInfo, Response, StdError, Timestamp, Uint128};

use crate::error::ContractError;
use crate::generic::{check_rate_limit, enforce_transaction_deadline};
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees_with_clip, calculate_fee_size_multiplier,
    calculate_fees_owed_split, check_ratio_deviation, check_slippage, sync_position_on_transfer,
    verify_position_ownership,
};
use crate::state::{
    PoolSpecs, CREATOR_FEE_POT, LIQUIDITY_POSITIONS, OWNER_POSITIONS, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE,
};
use crate::swap::update_price_accumulator;

pub fn remove_all_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;

    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    if pool_state.total_liquidity.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Pool total liquidity is zero",
        )));
    }
    let user_share_0 =
        current_reserve0.multiply_ratio(liquidity_position.liquidity, pool_state.total_liquidity);
    let user_share_1 =
        current_reserve1.multiply_ratio(liquidity_position.liquidity, pool_state.total_liquidity);
    check_slippage(user_share_0, min_amount0, "bluechip")?;
    check_slippage(user_share_1, min_amount1, "cw20")?;
    check_ratio_deviation(
        user_share_0,
        user_share_1,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )?;
    let ((fees_owed_0, fees_owed_1), _, (clipped_0, clipped_1)) =
        calc_capped_fees_with_clip(&liquidity_position, &pool_fee_state)?;

    let total_amount_0 = user_share_0.checked_add(fees_owed_0)?;
    let total_amount_1 = user_share_1.checked_add(fees_owed_1)?;

    let liquidity_to_subtract = liquidity_position.liquidity;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_subtract)?;

    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(user_share_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(user_share_1)?;
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

    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.remove(deps.storage, &position_id);
    OWNER_POSITIONS.remove(deps.storage, (&info.sender, &position_id));
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_lp_withdrawal_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let mut response = Response::new()
        .add_attribute("action", "remove_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("withdrawer", info.sender.to_string())
        .add_attribute(
            "liquidity_removed",
            liquidity_position.liquidity.to_string(),
        )
        .add_attribute("principal_0", user_share_0)
        .add_attribute("principal_1", user_share_1)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1)
        .add_attribute("total_0", total_amount_0)
        .add_attribute("total_1", total_amount_1)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_liquidity_after",
            pool_state.total_liquidity.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string())
        .add_attribute(
            "total_lp_withdrawal_count",
            analytics.total_lp_withdrawal_count.to_string(),
        );
    let transfer_msgs =
        build_fee_transfer_msgs(&pool_info, &info.sender, total_amount_0, total_amount_1)?;
    response = response.add_messages(transfer_msgs);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub fn remove_partial_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;

    if liquidity_to_remove.is_zero() {
        return Err(ContractError::InvalidAmount {});
    }
    if liquidity_to_remove == liquidity_position.liquidity {
        return execute_remove_all_liquidity(
            deps.branch(),
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        );
    }
    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    // Only calculate fees on the portion being removed. Split returns
    // `(adjusted, clipped)`; the clipped slice is routed to the creator
    // pot below so it isn't orphaned in fee_reserve.
    let (fees_owed_0, clipped_0) = calculate_fees_owed_split(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    )?;

    let (fees_owed_1, clipped_1) = calculate_fees_owed_split(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    )?;

    // Preserve fees on the remaining portion so resetting the snapshot
    // below doesn't discard them. Only the adjusted (LP-facing) portion
    // is preserved; the clipped slice of the remaining liquidity will
    // accrue through the standard fee_growth snapshot on the next
    // collect and route to the pot at that time.
    let remaining_liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;
    let (preserved_fees_0, _preserved_clip_0) = calculate_fees_owed_split(
        remaining_liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    )?;
    let (preserved_fees_1, _preserved_clip_1) = calculate_fees_owed_split(
        remaining_liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    )?;

    if pool_state.total_liquidity.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Pool total liquidity is zero",
        )));
    }
    let withdrawal_amount_0 =
        current_reserve0.multiply_ratio(liquidity_to_remove, pool_state.total_liquidity);

    let withdrawal_amount_1 =
        current_reserve1.multiply_ratio(liquidity_to_remove, pool_state.total_liquidity);

    let fees_owed_0 = fees_owed_0.min(pool_fee_state.fee_reserve_0);
    let fees_owed_1 = fees_owed_1.min(pool_fee_state.fee_reserve_1);
    // Cap the clip slice against whatever fee_reserve is left after the
    // LP portion so the two debits can't exceed the actual reserve.
    let clipped_0 = clipped_0.min(pool_fee_state.fee_reserve_0.saturating_sub(fees_owed_0));
    let clipped_1 = clipped_1.min(pool_fee_state.fee_reserve_1.saturating_sub(fees_owed_1));

    check_slippage(withdrawal_amount_0, min_amount0, "bluechip")?;
    check_slippage(withdrawal_amount_1, min_amount1, "cw20")?;
    check_ratio_deviation(
        withdrawal_amount_0,
        withdrawal_amount_1,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )?;
    let total_amount_0 = withdrawal_amount_0.checked_add(fees_owed_0)?;
    let total_amount_1 = withdrawal_amount_1.checked_add(fees_owed_1)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(withdrawal_amount_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(withdrawal_amount_1)?;
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

    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_remove)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;

    liquidity_position.unclaimed_fees_0 = liquidity_position
        .unclaimed_fees_0
        .checked_add(preserved_fees_0)?;
    liquidity_position.unclaimed_fees_1 = liquidity_position
        .unclaimed_fees_1
        .checked_add(preserved_fees_1)?;

    liquidity_position.liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;

    liquidity_position.fee_size_multiplier =
        calculate_fee_size_multiplier(liquidity_position.liquidity);

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_lp_withdrawal_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let mut response = Response::new()
        .add_attribute("action", "remove_partial_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("withdrawer", info.sender.to_string())
        .add_attribute("liquidity_removed", liquidity_to_remove.to_string())
        .add_attribute(
            "remaining_liquidity",
            liquidity_position.liquidity.to_string(),
        )
        .add_attribute("principal_0", withdrawal_amount_0)
        .add_attribute("principal_1", withdrawal_amount_1)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1)
        .add_attribute("preserved_fees_0", preserved_fees_0)
        .add_attribute("preserved_fees_1", preserved_fees_1)
        .add_attribute("total_0", total_amount_0)
        .add_attribute("total_1", total_amount_1)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_liquidity_after",
            pool_state.total_liquidity.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string())
        .add_attribute(
            "total_lp_withdrawal_count",
            analytics.total_lp_withdrawal_count.to_string(),
        );
    let transfer_msgs =
        build_fee_transfer_msgs(&pool_info, &info.sender, total_amount_0, total_amount_1)?;
    response = response.add_messages(transfer_msgs);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_all_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();
    check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
    remove_all_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_partial_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
    remove_partial_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        liquidity_to_remove,
        transaction_deadline,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_partial_liquidity_by_percent(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    percentage: u64,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    if percentage == 0 {
        return Err(ContractError::InvalidPercent {});
    }

    if percentage >= 100 {
        return execute_remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        );
    }

    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    let liquidity_to_remove = liquidity_position
        .liquidity
        .checked_mul(Uint128::from(percentage))?
        .checked_div(Uint128::from(100u128))
        .map_err(|_| ContractError::DivideByZero)?;
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
