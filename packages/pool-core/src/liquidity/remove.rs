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
use crate::generic::{check_rate_limit, enforce_transaction_deadline, with_reentrancy_guard};
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees_with_clip, calculate_fee_size_multiplier,
    calculate_fees_owed_split_pair, check_ratio_deviation, check_slippage,
    sync_position_on_transfer, verify_position_ownership,
};
use crate::state::{
    maybe_auto_pause_on_low_liquidity, PoolSpecs, CREATOR_FEE_POT, LIQUIDITY_POSITIONS,
    POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE,
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

    // Removable principal: full position minus the locked slice (always
    // zero on non-first-deposit positions, so this is a no-op for them).
    // The locked slice stays in the pool's reserves and continues to be
    // "owned" by this Position for fee-accrual purposes — the depositor
    // can keep calling collect_fees on it indefinitely.
    let removable_liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_position.locked_liquidity)?;
    let user_share_0 =
        current_reserve0.multiply_ratio(removable_liquidity, pool_state.total_liquidity);
    let user_share_1 =
        current_reserve1.multiply_ratio(removable_liquidity, pool_state.total_liquidity);
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

    pool_state.total_liquidity = pool_state.total_liquidity.checked_sub(removable_liquidity)?;

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
    // Arm auto-pause if this remove dropped reserves below MIN.
    // Future swaps/removes will reject; the next deposit that restores
    // reserves above MIN auto-clears both flags.
    let auto_paused_now = maybe_auto_pause_on_low_liquidity(deps.storage, &pool_state)?;

    // H-NFT-1 audit fix: both exit cases keep the LIQUIDITY_POSITIONS
    // row alive. The prior behaviour deleted the row on a standard exit
    // (locked_liquidity == 0), leaving the user's CW721 NFT as a
    // tombstone — it still existed on-chain (no BurnNft is ever
    // dispatched) but every pool-side handler that loaded
    // LIQUIDITY_POSITIONS would fail with a "not found" error. The NFT
    // was tradeable on secondary markets despite being functionally
    // inert; a buyer thinking they were acquiring an LP position would
    // get a token id that AddToPosition / CollectFees / RemoveLiquidity
    // all reject. Mirrors Uniswap V3's empty-position model: the NFT
    // and its position row stay alive at zero, ready to be rehydrated
    // by a future AddToPosition call.
    //
    // Difference between the two branches: first-depositor positions
    // (locked_liquidity > 0) drop to exactly the locked floor
    // (MINIMUM_LIQUIDITY), preserving fee rights against the perma-
    // locked slice. Standard positions (locked_liquidity == 0) drop
    // to zero. In both cases the NFT remains usable — owner can
    // re-deposit, transfer, or just hold an "empty position" NFT.
    //
    // OWNER_POSITIONS index stays so frontends listing "your
    // positions" still surface empty NFTs for re-deposit. If the
    // owner transfers the NFT, sync_position_on_transfer updates
    // both position.owner and the OWNER_POSITIONS index to track
    // the new holder — same flow as a non-empty position.
    liquidity_position.liquidity = liquidity_position.locked_liquidity;
    liquidity_position.fee_size_multiplier =
        calculate_fee_size_multiplier(liquidity_position.liquidity);
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.unclaimed_fees_0 = Uint128::zero();
    liquidity_position.unclaimed_fees_1 = Uint128::zero();
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    // Update analytics
    let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
    analytics.total_lp_withdrawal_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let mut response = Response::new().add_attributes(vec![
        ("action", "remove_liquidity".to_string()),
        ("position_id", position_id),
        ("withdrawer", info.sender.to_string()),
        // Report the actual removed amount, not `liquidity_position.liquidity`
        // — on the first-depositor branch the latter has been overwritten
        // with `locked_liquidity` (MINIMUM_LIQUIDITY), which would mis-report
        // every first-depositor exit to indexers and frontends.
        ("liquidity_removed", removable_liquidity.to_string()),
        ("principal_0", user_share_0.to_string()),
        ("principal_1", user_share_1.to_string()),
        ("fees_0", fees_owed_0.to_string()),
        ("fees_1", fees_owed_1.to_string()),
        ("total_0", total_amount_0.to_string()),
        ("total_1", total_amount_1.to_string()),
        ("reserve0_after", pool_state.reserve0.to_string()),
        ("reserve1_after", pool_state.reserve1.to_string()),
        ("total_liquidity_after", pool_state.total_liquidity.to_string()),
        ("pool_contract", pool_state.pool_contract_address.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
        ("total_lp_withdrawal_count", analytics.total_lp_withdrawal_count.to_string()),
        ("auto_paused", auto_paused_now.to_string()),
    ]);
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
    // First reject "more than the position has at all" with the existing
    // error, so callers/tests that bump into the absolute ceiling keep
    // seeing `InsufficientLiquidity`. Then cap at the locked floor:
    // the locked slice (zero on every non-first-deposit Position) is
    // permanently bound and can never be withdrawn, but still earns fees
    // against the full position size — see `Position.locked_liquidity`.
    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }
    let removable_liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_position.locked_liquidity)?;
    if liquidity_to_remove > removable_liquidity {
        return Err(ContractError::LockedLiquidity {
            locked: liquidity_position.locked_liquidity,
        });
    }
    if liquidity_to_remove == removable_liquidity {
        // Dispatch to the lock-/rate-limit-free core handler. The OUTER
        // `execute_remove_*` wrappers already hold the reentrancy lock at
        // this point, so calling `execute_remove_all_liquidity` here would
        // self-reenter and erroneously trip ContractError::ReentrancyGuard.
        // `remove_all_liquidity` is the same body without the wrapper —
        // safe to call directly while holding the lock.
        let _ = transaction_deadline;
        return remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        );
    }
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    // Compute split fees for both the removed portion (LP payout) and
    // the preserved portion (rolled into the position's `unclaimed_fees`)
    // in a single helper call per token. The clipped slice of the
    // removed portion is routed to the creator pot below; the preserved
    // clip is intentionally dropped — it accrues through the standard
    // `fee_growth` snapshot on the next collect.
    let remaining_liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;
    let (fees_owed_0, clipped_0, preserved_fees_0) = calculate_fees_owed_split_pair(
        liquidity_to_remove,
        remaining_liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    )?;
    let (fees_owed_1, clipped_1, preserved_fees_1) = calculate_fees_owed_split_pair(
        liquidity_to_remove,
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
    // Arm auto-pause if this partial-remove dropped reserves below MIN.
    let auto_paused_now = maybe_auto_pause_on_low_liquidity(deps.storage, &pool_state)?;

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
    let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
    analytics.total_lp_withdrawal_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let mut response = Response::new().add_attributes(vec![
        ("action", "remove_partial_liquidity".to_string()),
        ("position_id", position_id),
        ("withdrawer", info.sender.to_string()),
        ("liquidity_removed", liquidity_to_remove.to_string()),
        ("remaining_liquidity", liquidity_position.liquidity.to_string()),
        ("principal_0", withdrawal_amount_0.to_string()),
        ("principal_1", withdrawal_amount_1.to_string()),
        ("fees_0", fees_owed_0.to_string()),
        ("fees_1", fees_owed_1.to_string()),
        ("preserved_fees_0", preserved_fees_0.to_string()),
        ("preserved_fees_1", preserved_fees_1.to_string()),
        ("total_0", total_amount_0.to_string()),
        ("total_1", total_amount_1.to_string()),
        ("reserve0_after", pool_state.reserve0.to_string()),
        ("reserve1_after", pool_state.reserve1.to_string()),
        ("total_liquidity_after", pool_state.total_liquidity.to_string()),
        ("pool_contract", pool_state.pool_contract_address.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
        ("total_lp_withdrawal_count", analytics.total_lp_withdrawal_count.to_string()),
        ("auto_paused", auto_paused_now.to_string()),
    ]);
    let transfer_msgs =
        build_fee_transfer_msgs(&pool_info, &info.sender, total_amount_0, total_amount_1)?;
    response = response.add_messages(transfer_msgs);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_all_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    with_reentrancy_guard(deps, move |mut deps| {
        let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
        let sender = info.sender.clone();
        check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
        remove_all_liquidity(
            &mut deps,
            env,
            info,
            position_id,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_partial_liquidity(
    deps: DepsMut,
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

    with_reentrancy_guard(deps, move |mut deps| {
        let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
        let sender = info.sender.clone();
        check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
        remove_partial_liquidity(
            &mut deps,
            env,
            info,
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        )
    })
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

    // Percent-based removal is computed against the REMOVABLE slice, not
    // the full position. This keeps `remove_by_percent(50)` followed by
    // `remove_by_percent(50)` ending at 25% of the original removable
    // share — the natural expectation. For a position with no lock
    // (locked_liquidity == 0), this is identical to the previous behavior.
    let removable = liquidity_position
        .liquidity
        .checked_sub(liquidity_position.locked_liquidity)?;
    let liquidity_to_remove = removable
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
