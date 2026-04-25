//! LP-fee collection handler.
//!
//! An LP-position owner calls `collect_fees` to sweep the fees accrued
//! against their position since the last collection. This routes:
//!   - owed-and-uncapped fees   → LP's wallet (per asset side)
//!   - clipped-by-multiplier    → `CREATOR_FEE_POT` (per asset side)
//!
//! The fee-size-multiplier clipping is the mechanism that caps how much
//! of a single deposit's accrued fees the LP can take home if the
//! position's `fee_size_multiplier` has been reduced; see
//! `liquidity_helpers::calc_capped_fees_with_clip` for the math.

use cosmwasm_std::{DepsMut, Env, MessageInfo, Response, Uint128};

use crate::error::ContractError;
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees_with_clip, sync_position_on_transfer,
    verify_position_ownership,
};
use crate::state::{
    CREATOR_FEE_POT, LIQUIDITY_POSITIONS, POOL_FEE_STATE, POOL_INFO, POOL_STATE, REENTRANCY_LOCK,
};
use crate::swap::update_price_accumulator;

pub fn execute_collect_fees(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    // Reentrancy guard, same shared lock as every other state-mutating
    // entry point. CollectFees emits CW20 Transfer messages; a hostile
    // creator token could otherwise re-enter the pool before the response
    // commits and double-collect fees against a stale fee_growth checkpoint.
    if REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;
    let result = execute_collect_fees_inner(deps.branch(), env, info, position_id);
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
}

fn execute_collect_fees_inner(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
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
    let ((fees_owed_0, fees_owed_1), _, (clipped_0, clipped_1)) =
        calc_capped_fees_with_clip(&liquidity_position, &pool_fee_state)?;

    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.unclaimed_fees_0 = Uint128::zero();
    liquidity_position.unclaimed_fees_1 = Uint128::zero();

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    // Debit both the LP payout and the creator-pot slice from fee_reserve
    // in a single pass, then credit CREATOR_FEE_POT. Keeps the reserve
    // invariant (reserve == owed_to_someone) tight.
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

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    let fee_msgs = build_fee_transfer_msgs(&pool_info, &info.sender, fees_owed_0, fees_owed_1)?;

    Ok(Response::new()
        .add_messages(fee_msgs)
        .add_attributes(vec![
            ("action", "collect_fees".to_string()),
            ("position_id", position_id),
            ("collector", info.sender.to_string()),
            ("fees_0", fees_owed_0.to_string()),
            ("fees_1", fees_owed_1.to_string()),
            ("clipped_to_creator_pot_0", clipped_0.to_string()),
            ("clipped_to_creator_pot_1", clipped_1.to_string()),
            ("fee_reserve_0_after", pool_fee_state.fee_reserve_0.to_string()),
            ("fee_reserve_1_after", pool_fee_state.fee_reserve_1.to_string()),
            ("pool_contract", pool_state.pool_contract_address.to_string()),
            ("block_height", env.block.height.to_string()),
            ("block_time", env.block.time.seconds().to_string()),
        ]))
}
