use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;

#[cw_serde]
pub enum ExecuteMsg {
    SetPrice { price_id: String, price: Uint128 },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum PythQueryMsg {
    #[returns(PriceResponse)]
    GetPrice { price_id: String },
}

#[cw_serde]
pub struct InstantiateMsg {}

#[cw_serde]
pub struct PriceResponse {
    pub price: Uint128,
    pub publish_time: u64,
    pub expo: i32,
    pub conf: Uint128, // 8 decimals (e.g. 1.23 USD = 123_000_000)
}