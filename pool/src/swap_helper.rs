#![allow(non_snake_case)]
use crate::contract::DEFAULT_SLIPPAGE;
use crate::error::ContractError;

use crate::generic_helpers::decimal2decimal256;

use crate::state::{PoolState, POOL_INFO};
use cosmwasm_std::{Decimal, Decimal256, Deps, Fraction, StdError, StdResult, Uint128, Uint256};
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};
use std::str::FromStr;

// calculates swap amounts using constant product formula (x * y = k)
pub fn compute_swap(
    //pool balance of offer amount
    offer_pool: Uint128,
    //pool balance of requested amount
    ask_pool: Uint128,
    //amount being offered
    offer_amount: Uint128,
    //pool fee rate
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let offer_amount: Uint256 = offer_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;
    // constant product
    let cp: Uint256 = offer_pool * ask_pool;

    let return_amount: Uint256 = (Decimal256::from_ratio(ask_pool, 1u8)
        - Decimal256::from_ratio(cp, offer_pool + offer_amount))
    .numerator()
        / Decimal256::one().denominator();

    // calculate spread(slippage) & commission
    let spread_amount: Uint256 = (offer_amount
        * Decimal256::from_ratio(ask_pool, offer_pool).numerator()
        / Decimal256::from_ratio(ask_pool, offer_pool).denominator())
        - return_amount;
    let commission_amount: Uint256 =
        return_amount * commission_rate.numerator() / commission_rate.denominator();
    //subtract commission from return amount
    let return_amount: Uint256 = return_amount - commission_amount;
    Ok((
        //amount trader recieves
        return_amount.try_into()?,
        //slippage
        spread_amount.try_into()?,
        //fee to liquidity holders
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
        &FactoryQueryMsg::ConvertBluechipToUsd {
            amount: bluechip_amount,
        },
    )?;

    Ok(response.amount)
}

pub fn get_bluechip_value(deps: Deps, usd_amount: Uint128) -> StdResult<Uint128> {
    let factory_address = POOL_INFO.load(deps.storage)?;

    let response: ConversionResponse = deps.querier.query_wasm_smart(
        factory_address.factory_addr,
        &FactoryQueryMsg::ConvertUsdToBluechip { amount: usd_amount },
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
   let ask_amount_before_commission = (Decimal256::from_ratio(ask_amount, 1u8) 
    / one_minus_commission).numerator() / Decimal256::one().denominator();
    
    // Use constant product formula: k = offer_pool * ask_pool
    // After swap: k = (offer_pool + offer_amount) * (ask_pool - ask_amount_before_commission)
    // Therefore: offer_pool * ask_pool = (offer_pool + offer_amount) * (ask_pool - ask_amount_before_commission)
    // Solving for offer_amount:
    // offer_amount = (offer_pool * ask_pool) / (ask_pool - ask_amount_before_commission) - offer_pool
    
    let cp: Uint256 = offer_pool * ask_pool;
    let new_ask_pool = ask_pool.checked_sub(ask_amount_before_commission)
        .map_err(|_| StdError::generic_err("Insufficient liquidity in pool"))?;
    
    let new_offer_pool = cp.checked_div(new_ask_pool)
        .map_err(|_| StdError::generic_err("Division error"))?;
    
    let offer_amount = new_offer_pool.checked_sub(offer_pool)
        .map_err(|_| StdError::generic_err("Invalid offer amount calculation"))?;
    
    // Calculate spread amount (price impact)
    // spread = offer_amount - (ask_amount_before_commission * offer_pool / ask_pool)
    let expected_offer_amount = ask_amount_before_commission * offer_pool / ask_pool;
    let spread_amount: Uint256 = offer_amount.saturating_sub(expected_offer_amount);
    
    // Calculate commission on the ask amount
    let commission_amount: Uint256 = ask_amount_before_commission * commission_rate.numerator() 
        / commission_rate.denominator();
    
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
        let expected_return = offer_amount * belief_price.inv().unwrap().numerator()
            / belief_price.inv().unwrap().denominator();
        let spread_amount = expected_return
            .checked_sub(return_amount)
            .unwrap_or_else(|_| Uint128::zero());

        if return_amount < expected_return
            && Decimal::from_ratio(spread_amount, expected_return) > max_spread
        {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    } else
    //use calculated spread amount from swap computation
    if Decimal::from_ratio(spread_amount, return_amount + spread_amount) > max_spread {
        return Err(ContractError::MaxSpreadAssertion {});
    }

    Ok(())
}
