use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;

#[cw_serde]
pub enum PythQueryMsg {
    GetPrice { price_id: String },
}

#[cw_serde]
pub struct PriceResponse {
    // The price value (e.g., 125_000_000 for $1.25 with 8 decimals)
    pub price: Uint128, 
    // Unix timestamp when price was last updated
    pub publish_time: u64, 
    // decimals
    pub expo: i32,   
    pub conf: Uint128,
}

pub struct OracleData {
    pub price: Uint128,
    pub expo: i32,
}