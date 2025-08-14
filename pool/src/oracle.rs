use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;

#[cw_serde]
pub enum PythQueryMsg {
    /// e.g. price_id = "SEI_USD"
    GetPrice { price_id: String },
}

#[cw_serde]
pub struct PriceResponse {
    pub price: i64, // The price value (e.g., 125_000_000 for $1.25 with 8 decimals)
    pub publish_time: u64, // Unix timestamp when price was last updated
    pub expo: i32,   // Price decimals (usually 8 for Pyth)
    pub conf: Uint128,
}

pub struct OracleData {
    pub price: Uint128,
    pub expo: i32,
}