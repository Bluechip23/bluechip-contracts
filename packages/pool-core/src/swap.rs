//! Pair-shape-agnostic constant-product AMM math.
//!
//! Shared by simple swap, post-threshold commit, and the deposit /
//! remove-liquidity paths on both pool kinds. Everything here is
//! either pure (no storage) or mutates a caller-provided `PoolState`
//! reference.
//!
//! Oracle-backed USD conversion helpers — which query the factory's
//! internal oracle and are only needed by the commit flow — stay in
//! `creator-pool::swap_helper`.

use crate::error::ContractError;
use crate::generic::decimal2decimal256;
use crate::state::PoolState;
use cosmwasm_std::{Decimal, Decimal256, Fraction, StdError, StdResult, Uint128, Uint256};
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
