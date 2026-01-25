#![allow(non_snake_case)]
use crate::contract::DEFAULT_SLIPPAGE;
use crate::error::ContractError;

use crate::generic_helpers::decimal2decimal256;

use crate::state::{PoolState, POOL_INFO};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Decimal, Decimal256, Deps, Fraction, StdError, StdResult, Uint128, Uint256};
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};
use std::str::FromStr;

// Wrapper for factory queries to match the factory's QueryMsg structure
#[cw_serde]
enum FactoryQueryWrapper {
    InternalBlueChipOracleQuery(FactoryQueryMsg),
}

// calculates swap amounts using constant product formula (x * y = k)
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

    // constant product - use checked math
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

    // calculate spread - use checked math
    let price_ratio = Decimal256::from_ratio(ask_pool, offer_pool);
    // calculate spread - saturate to zero if actual > ideal (can happen with rounding)
    let ideal_return = offer_amount
        .checked_mul(price_ratio.numerator())
        .map_err(|e| StdError::generic_err(format!("Overflow calculating spread: {}", e)))?
        .checked_div(price_ratio.denominator())
        .map_err(|e| StdError::generic_err(format!("Division error calculating spread: {}", e)))?;

    // If return_amount > ideal (favorable rounding), spread is zero
    let spread_amount: Uint256 = if ideal_return > return_amount {
        ideal_return - return_amount
    } else {
        Uint256::zero()
    };

    // calculate commission - use checked math
    let commission_amount: Uint256 = return_amount
        .checked_mul(commission_rate.numerator())
        .map_err(|e| StdError::generic_err(format!("Overflow calculating commission: {}", e)))?
        .checked_div(commission_rate.denominator())
        .map_err(|e| {
            StdError::generic_err(format!("Division error calculating commission: {}", e))
        })?;

    // subtract commission from return amount - use checked math
    let final_return_amount: Uint256 = return_amount
        .checked_sub(commission_amount)
        .map_err(|e| StdError::generic_err(format!("Underflow subtracting commission: {}", e)))?;

    Ok((
        final_return_amount.try_into()?,
        spread_amount.try_into()?,
        commission_amount.try_into()?,
    ))
}
// Update price accumulator with time-weighted average
pub fn update_price_accumulator(
    pool_state: &mut PoolState,
    current_time: u64,
) -> Result<(), ContractError> {
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);
    // update if time has passed and both reserves exist
    if time_elapsed > 0 && !pool_state.reserve0.is_zero() && !pool_state.reserve1.is_zero() {
        // Calculate price0 * time_elapsed directly
        let price0_increment = pool_state
            .reserve1
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve0)
            .map_err(|_| ContractError::DivideByZero)?;
        // Calculate price1 * time_elapsed directly
        let price1_increment = pool_state
            .reserve0
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve1)
            .map_err(|_| ContractError::DivideByZero)?;
        //add to cumulative accumulators
        pool_state.price0_cumulative_last = pool_state
            .price0_cumulative_last
            .checked_add(price0_increment)?;
        pool_state.price1_cumulative_last = pool_state
            .price1_cumulative_last
            .checked_add(price1_increment)?;
        //update time for next check
        pool_state.block_time_last = current_time;
    }

    Ok(())
}

pub fn get_usd_value(deps: Deps, bluechip_amount: Uint128) -> StdResult<Uint128> {
    let factory_address = POOL_INFO.load(deps.storage)?;

    let response: ConversionResponse = deps.querier.query_wasm_smart(
        factory_address.factory_addr,
        &FactoryQueryWrapper::InternalBlueChipOracleQuery(FactoryQueryMsg::ConvertBluechipToUsd {
            amount: bluechip_amount,
        }),
    )?;

    Ok(response.amount)
}

pub fn get_bluechip_value(deps: Deps, usd_amount: Uint128) -> StdResult<Uint128> {
    let factory_address = POOL_INFO.load(deps.storage)?;

    let response: ConversionResponse = deps.querier.query_wasm_smart(
        factory_address.factory_addr,
        &FactoryQueryWrapper::InternalBlueChipOracleQuery(FactoryQueryMsg::ConvertUsdToBluechip {
            amount: usd_amount,
        }),
    )?;

    Ok(response.amount)
}
//used in reverse query to find price for a desired amount of an unowned token in a token pair
//compuets a required offer amount for a desired ask amount
pub fn compute_offer_amount(
    //current pool balance of token being offered
    offer_pool: Uint128,
    //current pool balance of requested token
    ask_pool: Uint128,
    ask_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let ask_amount: Uint256 = ask_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;

    // Calculate ask_amount before commission is applied
    // ask_amount_with_commission = ask_amount / (1 - commission _rate)
    let one_minus_commission = Decimal256::one() - commission_rate;
    let ask_amount_before_commission =
        (Decimal256::from_ratio(ask_amount, 1u8) / one_minus_commission).numerator()
            / Decimal256::one().denominator();

    let cp: Uint256 = offer_pool * ask_pool;
    let new_ask_pool = ask_pool
        .checked_sub(ask_amount_before_commission)
        .map_err(|_| StdError::generic_err("Insufficient liquidity in pool"))?;

    let new_offer_pool = cp
        .checked_div(new_ask_pool)
        .map_err(|_| StdError::generic_err("Division error"))?;

    let offer_amount = new_offer_pool
        .checked_sub(offer_pool)
        .map_err(|_| StdError::generic_err("Invalid offer amount calculation"))?;

    // Calculate spread amount (price impact)
    // spread = offer_amount - (ask_amount_before_commission * offer_pool / ask_pool)
    let expected_offer_amount = ask_amount_before_commission * offer_pool / ask_pool;
    let spread_amount: Uint256 = offer_amount.saturating_sub(expected_offer_amount);

    // Calculate commission on the ask amount
    let commission_amount: Uint256 =
        ask_amount_before_commission * commission_rate.numerator() / commission_rate.denominator();

    Ok((
        //amount trader must offer
        offer_amount.try_into()?,
        //slippage
        spread_amount.try_into()?,
        //fee to liquidity holders
        commission_amount.try_into()?,
    ))
}
//use either belief price or calculated spread amount
pub fn assert_max_spread(
    //expeccted exchange rate
    belief_price: Option<Decimal>,
    //max slippage the trader allows
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
        //compare against traders expected price
        let inverse = belief_price.inv().ok_or_else(|| {
            ContractError::Std(StdError::generic_err("Invalid belief price: zero"))
        })?;

        let expected_return = offer_amount * inverse.numerator() / inverse.denominator();
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
        let total_amount = return_amount + spread_amount;
        if total_amount.is_zero() {
            return Err(ContractError::MaxSpreadAssertion {});
        }
        //use calculated spread amount from swap computation
        if Decimal::from_ratio(spread_amount, total_amount) > max_spread {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    }

    Ok(())
}
