#![allow(non_snake_case)]
use crate::contract::{DEFAULT_SLIPPAGE, MAX_ALLOWED_SLIPPAGE};
use crate::error::ContractError;

use crate::generic_helpers::decimal2decimal256;

use crate::oracle::{OracleData, PriceResponse, PythQueryMsg};
use crate::state::PoolState;
use crate::state::{MAX_ORACLE_AGE, POOL_STATE};
use cosmwasm_std::{
    Addr, Decimal, Decimal256, Deps, Fraction, QuerierWrapper, StdError, StdResult, Uint128,
    Uint256,
};
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

pub fn native_to_usd(
    cached_price: Uint128,
    native_amount: Uint128,
    expo: i32, // micro-native
) -> StdResult<Uint128> {
    // validate expected exponent - oracle prices should have 8 decimal places
    if expo != -8 {
        return Err(StdError::generic_err(format!(
            "Unexpected price exponent: {}. Expected: -8",
            expo
        )));
    }
    // dividing by 100M adjusts for the -8 exponent (8 decimal places)
    let usd_micro_u256 = (Uint256::from(native_amount) * Uint256::from(cached_price))
        / Uint256::from(100_000_000u128);

    let usd_micro = Uint128::try_from(usd_micro_u256)?;
    //returns USD amount in micro-USD (6 decimals)
    Ok(usd_micro)
}

//usd to native using cahched price and handles decimal precision
pub fn usd_to_native(
    usd_amount: Uint128,
    // micro-USD (6 decimals)
    cached_price: Uint128,
) -> StdResult<Uint128> {
    if cached_price.is_zero() {
        return Err(StdError::generic_err("Invalid zero price"));
    }
    //100 multiplier adjusts for decimal precision differences
    let native_micro_u256 =
        (Uint256::from(usd_amount) * Uint256::from(100u128)) / Uint256::from(cached_price);
    Uint128::try_from(native_micro_u256).map_err(|_| StdError::generic_err("Overflow"))
}

pub fn get_and_validate_oracle_price(
    querier: &QuerierWrapper,
    oracle_addr: &Addr,
    symbol: &str,
    current_time: u64,
) -> StdResult<OracleData> {
    let resp: PriceResponse = querier
        .query_wasm_smart(
            oracle_addr.clone(),
            &PythQueryMsg::GetPrice {
                price_id: symbol.into(),
            },
        )
        .map_err(|e| StdError::generic_err(format!("Oracle query failed: {}", e)))?;

    // Staleness check
    let zero: Uint128 = Uint128::zero();
    if resp.price <= zero {
        return Err(StdError::generic_err(
            "Invalid zero or negative price from oracle",
        ));
    }

    if current_time.saturating_sub(resp.publish_time) > MAX_ORACLE_AGE {
        return Err(StdError::generic_err("Oracle price too stale"));
    }
    Ok(OracleData {
        price: resp.price,
        expo: resp.expo,
    })
}

pub fn validate_oracle_price_against_twap(
    deps: &Deps,
    oracle_price: Uint128,
    current_time: u64,
) -> Result<(), ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;

    // Calculate TWAP over last hour (or since last update)
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);

    if time_elapsed > 3600 && !pool_state.reserve0.is_zero() {
        // Calculate average price from accumulator
        let twap_price = pool_state
            .price0_cumulative_last
            .checked_div(Uint128::from(time_elapsed))
            .map_err(|_| ContractError::DivideByZero {})?;

        // Check deviation (allow 20% max)
        let deviation = if oracle_price > twap_price {
            Decimal::from_ratio(oracle_price - twap_price, twap_price)
        } else {
            Decimal::from_ratio(twap_price - oracle_price, twap_price)
        };

        if deviation > Decimal::percent(20) {
            return Err(ContractError::OraclePriceDeviation {
                oracle: oracle_price,
                twap: twap_price,
            });
        }
    }
    Ok(())
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
