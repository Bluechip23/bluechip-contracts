use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Uint128};

pub mod cw721_msgs;

#[cw_serde]
pub enum PoolQueryMsg {
    GetPoolState { pool_contract_address: String },
    GetAllPools {},
}
#[cw_serde]
#[derive(QueryResponses)]
pub enum FactoryQueryMsg {
    #[returns(BluechipPriceResponse)]
    GetBluechipUsdPrice {},

    #[returns(ConversionResponse)]
    ConvertBluechipToUsd { amount: Uint128 },

    #[returns(ConversionResponse)]
    ConvertUsdToBluechip { amount: Uint128 },
}

#[cw_serde]
pub struct BluechipPriceResponse {
    pub price: Uint128,
    pub timestamp: u64,
    pub is_cached: bool,
}

#[cw_serde]
pub struct ConversionResponse {
    pub amount: Uint128,
    pub rate_used: Uint128,
    pub timestamp: u64,
}

#[cw_serde]
pub struct PoolStateResponseForFactory {
    pub pool_contract_address: Addr,
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128,
    pub reserve1: Uint128,
    pub total_liquidity: Uint128,
    pub block_time_last: u64,
    pub price0_cumulative_last: Uint128,
    pub price1_cumulative_last: Uint128,
    pub assets: Vec<String>,
}

#[cw_serde]
pub struct AllPoolsResponse {
    pub pools: Vec<(String, PoolStateResponseForFactory)>,
}

#[cw_serde]
pub enum ExpandEconomyMsg {
    RequestExpansion { recipient: String, amount: Uint128 },
}

#[cw_serde]
pub enum ExpandEconomyExecuteMsg {
    ExpandEconomy(ExpandEconomyMsg),
}

pub fn calculate_mint_amount(
    seconds_elapsed: u64,
    pools_created: u64,
) -> cosmwasm_std::StdResult<cosmwasm_std::Uint128> {
    // Formula: 500 - (((5x^2 + x) / ((s/6) + 333x))

    let x = pools_created as u128;
    let s = seconds_elapsed as u128;

    let five_x_squared = 5u128
        .checked_mul(x)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in numerator"))?
        .checked_mul(x)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in numerator"))?;

    let numerator = five_x_squared
        .checked_add(x)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in numerator addition"))?;
    //number of bluechips minted by chain since first pool creation
    let s_div_6 = s / 6;
    let denominator = s_div_6
        .checked_add(
            333u128
                .checked_mul(x)
                .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in denominator"))?,
        )
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in denominator"))?;

    if denominator == 0 {
        return Ok(cosmwasm_std::Uint128::new(500_000_000));
    }

    let division_result = numerator
        .checked_mul(1_000_000)
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("Overflow in division result"))?
        / denominator;

    let base_amount = 500_000_000u128;

    if division_result >= base_amount {
        return Ok(cosmwasm_std::Uint128::zero());
    }

    Ok(cosmwasm_std::Uint128::new(base_amount - division_result))
}
