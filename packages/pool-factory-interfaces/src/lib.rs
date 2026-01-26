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
