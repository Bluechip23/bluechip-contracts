//! Pair-shape-agnostic swap.
//!
//! Two layers:
//!   - Pure AMM math: `compute_swap`, `compute_offer_amount`,
//!     `assert_max_spread`, `update_price_accumulator`. No storage; may
//!     mutate a caller-provided `PoolState` ref.
//!   - Swap orchestration: `execute_swap_cw20` (CW20 `Receive` hook),
//!     `simple_swap` (reentrancy + rate-limit wrapper), and
//!     `execute_simple_swap` (the actual swap handler). All
//!     shape-agnostic — no commit-phase logic; `query_check_commit` is
//!     the only gate and it's `true` on standard pools by default.
//!
//! Oracle-backed USD conversion helpers — which query the factory's
//! internal oracle and are only needed by the commit flow — stay in
//! `creator-pool::swap_helper`.

use crate::asset::{TokenInfo, TokenInfoPoolExt, TokenType};
use crate::error::ContractError;
use crate::generic::{check_rate_limit, decimal2decimal256, enforce_transaction_deadline,
    update_pool_fee_growth};
use crate::msg::Cw20HookMsg;
use crate::state::{
    PoolCtx, PoolInfo, PoolState, IS_THRESHOLD_HIT, MINIMUM_LIQUIDITY, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE, REENTRANCY_LOCK,
};
use cosmwasm_std::{
    from_json, Addr, Decimal, Decimal256, DepsMut, Env, Fraction, MessageInfo, Response, StdError,
    StdResult, Uint128, Uint256,
};
use cw20::Cw20ReceiveMsg;
use std::str::FromStr;

pub const DEFAULT_SLIPPAGE: &str = "0.005";

/// Constant product swap (x * y = k). Returns (return_amount, spread, commission).
pub fn compute_swap(
    offer_pool: Uint128,
    ask_pool: Uint128,
    offer_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let offer_amount: Uint256 = offer_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;

    let cp: Uint256 = offer_pool.checked_mul(ask_pool).map_err(|e| {
        StdError::generic_err(format!("Overflow calculating constant product: {}", e))
    })?;

    let return_amount: Uint256 = (Decimal256::from_ratio(ask_pool, 1u8)
        - Decimal256::from_ratio(
            cp,
            offer_pool.checked_add(offer_amount).map_err(|e| {
                StdError::generic_err(format!("Overflow in pool calculation: {}", e))
            })?,
        ))
    .numerator()
        / Decimal256::one().denominator();

    let price_ratio = Decimal256::from_ratio(ask_pool, offer_pool);
    let ideal_return = offer_amount
        .checked_mul(price_ratio.numerator())
        .map_err(|e| StdError::generic_err(format!("Overflow calculating spread: {}", e)))?
        .checked_div(price_ratio.denominator())
        .map_err(|e| StdError::generic_err(format!("Division error calculating spread: {}", e)))?;

    let spread_amount: Uint256 = if ideal_return > return_amount {
        ideal_return - return_amount
    } else {
        Uint256::zero()
    };

    let commission_amount: Uint256 = return_amount
        .checked_mul(commission_rate.numerator())
        .map_err(|e| StdError::generic_err(format!("Overflow calculating commission: {}", e)))?
        .checked_div(commission_rate.denominator())
        .map_err(|e| {
            StdError::generic_err(format!("Division error calculating commission: {}", e))
        })?;

    let final_return_amount: Uint256 = return_amount
        .checked_sub(commission_amount)
        .map_err(|e| StdError::generic_err(format!("Underflow subtracting commission: {}", e)))?;

    Ok((
        final_return_amount.try_into()?,
        spread_amount.try_into()?,
        commission_amount.try_into()?,
    ))
}

pub fn update_price_accumulator(
    pool_state: &mut PoolState,
    current_time: u64,
) -> Result<(), ContractError> {
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);
    if time_elapsed > 0 && !pool_state.reserve0.is_zero() && !pool_state.reserve1.is_zero() {
        let price0_increment = pool_state
            .reserve1
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve0)
            .map_err(|_| ContractError::DivideByZero)?;
        let price1_increment = pool_state
            .reserve0
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve1)
            .map_err(|_| ContractError::DivideByZero)?;
        pool_state.price0_cumulative_last = pool_state
            .price0_cumulative_last
            .saturating_add(price0_increment);
        pool_state.price1_cumulative_last = pool_state
            .price1_cumulative_last
            .saturating_add(price1_increment);
        pool_state.block_time_last = current_time;
    }

    Ok(())
}

/// Reverse swap: computes the required offer amount for a desired ask amount.
pub fn compute_offer_amount(
    offer_pool: Uint128,
    ask_pool: Uint128,
    ask_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let ask_amount: Uint256 = ask_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;

    let one_minus_commission = Decimal256::one()
        .checked_sub(commission_rate)
        .map_err(|_| StdError::generic_err("Commission rate >= 100%"))?;
    let ask_amount_before_commission =
        (Decimal256::from_ratio(ask_amount, 1u8) / one_minus_commission).numerator()
            / Decimal256::one().denominator();

    let cp: Uint256 = offer_pool
        .checked_mul(ask_pool)
        .map_err(|_| StdError::generic_err("Constant product overflow"))?;
    let new_ask_pool = ask_pool
        .checked_sub(ask_amount_before_commission)
        .map_err(|_| StdError::generic_err("Insufficient liquidity in pool"))?;

    let new_offer_pool = cp
        .checked_div(new_ask_pool)
        .map_err(|_| StdError::generic_err("Division error"))?;

    let offer_amount = new_offer_pool
        .checked_sub(offer_pool)
        .map_err(|_| StdError::generic_err("Invalid offer amount calculation"))?;

    let expected_offer_amount = ask_amount_before_commission
        .checked_mul(offer_pool)
        .map_err(|_| StdError::generic_err("Expected offer amount overflow"))?
        .checked_div(ask_pool)
        .map_err(|_| StdError::generic_err("Expected offer amount division error"))?;
    let spread_amount: Uint256 = offer_amount.saturating_sub(expected_offer_amount);

    let commission_amount: Uint256 = ask_amount_before_commission
        .checked_mul(commission_rate.numerator())
        .map_err(|_| StdError::generic_err("Commission calculation overflow"))?
        .checked_div(commission_rate.denominator())
        .map_err(|_| StdError::generic_err("Commission calculation division error"))?;

    Ok((
        offer_amount.try_into()?,
        spread_amount.try_into()?,
        commission_amount.try_into()?,
    ))
}

pub fn assert_max_spread(
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    offer_amount: Uint128,
    return_amount: Uint128,
    spread_amount: Uint128,
) -> Result<(), ContractError> {
    let default_spread = Decimal::from_str(DEFAULT_SLIPPAGE)?;

    let max_spread = max_spread.unwrap_or(default_spread);
    if belief_price == Some(Decimal::zero()) {
        return Err(ContractError::InvalidBeliefPrice {});
    }

    if let Some(belief_price) = belief_price {
        let inverse = belief_price.inv().ok_or_else(|| {
            ContractError::Std(StdError::generic_err("Invalid belief price: zero"))
        })?;

        let expected_return = offer_amount
            .checked_mul(inverse.numerator())
            .map_err(|_| ContractError::Std(StdError::generic_err("Expected return overflow")))?
            .checked_div(inverse.denominator())
            .map_err(|_| {
                ContractError::Std(StdError::generic_err("Expected return division error"))
            })?;
        let spread_amount = expected_return
            .checked_sub(return_amount)
            .unwrap_or_else(|_| Uint128::zero());

        if expected_return.is_zero() {
            return Err(ContractError::MaxSpreadAssertion {});
        }

        if return_amount < expected_return
            && Decimal::from_ratio(spread_amount, expected_return) > max_spread
        {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    } else {
        let total_amount = return_amount
            .checked_add(spread_amount)
            .map_err(|_| ContractError::Std(StdError::generic_err("Spread total overflow")))?;
        if total_amount.is_zero() {
            return Err(ContractError::MaxSpreadAssertion {});
        }
        if Decimal::from_ratio(spread_amount, total_amount) > max_spread {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Swap orchestration (CW20 hook + reentrancy/rate-limit wrapper + handler)
// ---------------------------------------------------------------------------

pub fn execute_swap_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    // Gate: standard pools set IS_THRESHOLD_HIT=true at instantiate so this
    // is a no-op for them; creator pools set it at threshold-crossing time.
    if !IS_THRESHOLD_HIT.load(deps.storage)? {
        return Err(ContractError::ShortOfThreshold {});
    }
    if cw20_msg.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }
    let contract_addr = info.sender.clone();
    match from_json(&cw20_msg.msg) {
        Ok(Cw20HookMsg::Swap {
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        }) => {
            let pool_info: PoolInfo = POOL_INFO.load(deps.storage)?;
            let authorized = pool_info.pool_info.asset_infos.iter().any(|t| {
                matches!(t, TokenType::CreatorToken { contract_addr } if contract_addr == info.sender)
            });
            if !authorized {
                return Err(ContractError::Unauthorized {});
            }
            let to_addr = to.map(|a| deps.api.addr_validate(&a)).transpose()?;
            let validated_sender = deps.api.addr_validate(&cw20_msg.sender)?;
            simple_swap(
                deps,
                env,
                info,
                validated_sender,
                TokenInfo {
                    info: TokenType::CreatorToken { contract_addr },
                    amount: cw20_msg.amount,
                },
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        Err(err) => Err(ContractError::Std(err)),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn simple_swap(
    mut deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
    transaction_deadline: Option<cosmwasm_std::Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let reentrancy_guard = REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;

    let pool_specs = POOL_SPECS.load(deps.storage)?;

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        REENTRANCY_LOCK.save(deps.storage, &false)?;
        return Err(e);
    }

    let result = execute_simple_swap(
        &mut deps,
        env,
        _info,
        sender,
        offer_asset,
        belief_price,
        max_spread,
        to,
    );
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
}

#[allow(clippy::too_many_arguments)]
pub fn execute_simple_swap(
    deps: &mut DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
) -> Result<Response, ContractError> {
    let PoolCtx {
        info: pool_info,
        state: mut pool_state,
        fees: mut pool_fee_state,
        specs: pool_specs,
    } = PoolCtx::load(deps.storage)?;

    let (offer_index, offer_pool, ask_pool) =
        if offer_asset.info.equal(&pool_info.pool_info.asset_infos[0]) {
            (0usize, pool_state.reserve0, pool_state.reserve1)
        } else if offer_asset.info.equal(&pool_info.pool_info.asset_infos[1]) {
            (1usize, pool_state.reserve1, pool_state.reserve0)
        } else {
            return Err(ContractError::AssetMismatch {});
        };

    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    // Drain guard: reject swaps when either side is below MINIMUM_LIQUIDITY.
    // Don't try to persist POOL_PAUSED here — returning Err would revert the
    // save, so it's dead state. The reserve check alone is sufficient to
    // block every swap path; admins unlock the pool by restoring reserves or
    // by calling the factory's explicit UnpausePool route if POOL_PAUSED was
    // ever set by a successful admin action.
    if pool_state.reserve0 < MINIMUM_LIQUIDITY || pool_state.reserve1 < MINIMUM_LIQUIDITY {
        return Err(ContractError::InsufficientReserves {});
    }

    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, offer_asset.amount, pool_specs.lp_fee)?;

    // Reject dust swaps where the constant-product math floored
    // return_amt to zero. Without this, the trader's offer would be
    // absorbed into the pool while they receive nothing — effectively
    // donating to LPs. Better to surface the "offer too small" error
    // and let the caller bump their size or abandon.
    if return_amt.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    assert_max_spread(
        belief_price,
        max_spread,
        offer_asset.amount,
        return_amt.checked_add(commission_amt)?,
        spread_amt,
    )?;

    let offer_pool_post = offer_pool.checked_add(offer_asset.amount)?;
    let ask_pool_post = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

    if ask_pool_post < MINIMUM_LIQUIDITY {
        return Err(ContractError::InsufficientReserves {});
    }

    // TWAP: accumulate price using OLD reserves before updating
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

    if offer_index == 0 {
        pool_state.reserve0 = offer_pool_post;
        pool_state.reserve1 = ask_pool_post;
    } else {
        pool_state.reserve0 = ask_pool_post;
        pool_state.reserve1 = offer_pool_post;
    }

    update_pool_fee_growth(
        &mut pool_fee_state,
        &pool_state,
        offer_index,
        commission_amt,
    )?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    // Update analytics counters
    let mut analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    analytics.total_swap_count += 1;
    if offer_index == 0 {
        analytics.total_volume_0 = analytics.total_volume_0.saturating_add(offer_asset.amount);
        analytics.total_volume_1 = analytics.total_volume_1.saturating_add(return_amt);
    } else {
        analytics.total_volume_1 = analytics.total_volume_1.saturating_add(offer_asset.amount);
        analytics.total_volume_0 = analytics.total_volume_0.saturating_add(return_amt);
    }
    analytics.last_trade_block = env.block.height;
    analytics.last_trade_timestamp = env.block.time.seconds();
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    let ask_asset_info = if offer_index == 0 {
        pool_info.pool_info.asset_infos[1].clone()
    } else {
        pool_info.pool_info.asset_infos[0].clone()
    };

    // Lazy-evaluate sender.clone() so the clone is skipped when `to` is Some.
    let receiver = to.unwrap_or_else(|| sender.clone());
    let msgs = if !return_amt.is_zero() {
        vec![TokenInfo {
            info: ask_asset_info.clone(),
            amount: return_amt,
        }
        .into_msg(&deps.querier, receiver.clone())?]
    } else {
        vec![]
    };

    // Effective price: how much ask per unit of offer the trader received
    let effective_price = if !offer_asset.amount.is_zero() {
        Decimal::from_ratio(return_amt, offer_asset.amount).to_string()
    } else {
        "0".to_string()
    };

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "swap")
        .add_attribute("sender", sender)
        .add_attribute("receiver", receiver)
        .add_attribute("offer_asset", offer_asset.info.to_string())
        .add_attribute("ask_asset", ask_asset_info.to_string())
        .add_attribute("offer_amount", offer_asset.amount.to_string())
        .add_attribute("return_amount", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount", commission_amt.to_string())
        .add_attribute("effective_price", effective_price)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string())
        .add_attribute(
            "total_fee_collected_0",
            pool_fee_state.total_fees_collected_0.to_string(),
        )
        .add_attribute(
            "total_fee_collected_1",
            pool_fee_state.total_fees_collected_1.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_state.pool_contract_address.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string())
        .add_attribute("total_swap_count", analytics.total_swap_count.to_string()))
}
