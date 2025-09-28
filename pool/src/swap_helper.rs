#![allow(non_snake_case)]
use crate::contract::{DEFAULT_SLIPPAGE, MAX_ALLOWED_SLIPPAGE};
use crate::error::ContractError;

use crate::generic_helpers::decimal2decimal256;

use crate::state::{PoolState, POOL_INFO};
use cosmwasm_std::{
    Decimal, Decimal256, Deps, Fraction, StdResult, Uint128, Uint256
};
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};
use std::str::FromStr;
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
        &FactoryQueryMsg::ConvertBluechipToUsd { amount: bluechip_amount },
    )?;
    
    Ok(response.amount)
}

pub fn get_bluechip_amount(deps: Deps, usd_amount: Uint128) -> StdResult<Uint128> {
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
    //current pool balance of token begin offered
    offer_pool: Uint128,
    //curren pool balance of requested token
    ask_pool: Uint128,
    ask_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    //reverse commission adjustment
    let one_minus_commission = Decimal256::one() - decimal2decimal256(commission_rate)?;
    let inv_one_minus_commission = Decimal256::one() / one_minus_commission;

    let ask_amount_256: Uint256 = ask_amount.into();
    // accounts for commission by adjusting the ask amount upward
    let offer_amount: Uint256 = Uint256::from(
        ask_pool.checked_sub(
            (ask_amount_256 * inv_one_minus_commission.numerator()
                / inv_one_minus_commission.denominator())
            .try_into()?,
        )?,
    );

    let spread_amount = (offer_amount * Decimal256::from_ratio(ask_pool, offer_pool).numerator()
        / Decimal256::from_ratio(ask_pool, offer_pool).denominator())
    .checked_sub(offer_amount)?
    .try_into()?;
    let commission_amount = offer_amount * decimal2decimal256(commission_rate)?.numerator()
        / decimal2decimal256(commission_rate)?.denominator();
    Ok((
        offer_amount.try_into()?,
        spread_amount,
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
    let max_allowed_spread = Decimal::from_str(MAX_ALLOWED_SLIPPAGE)?;

    let max_spread = max_spread.unwrap_or(default_spread);
    if belief_price == Some(Decimal::zero()) {
        return Err(ContractError::InvalidBeliefPrice {});
    }
    if max_spread.gt(&max_allowed_spread) {
        return Err(ContractError::AllowedSpreadAssertion {});
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
