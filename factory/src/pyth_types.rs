use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;

// Mirrors mockoracle::msg::PriceResponse. Used by the #[cfg(feature = "mock")]
// oracle-update path, which reads the current bluechip price directly from
// the configured mock oracle instead of deriving it from pool TWAPs.
#[cw_serde]
pub struct PriceResponse {
    pub price: Uint128,
    pub publish_time: u64,
    pub expo: i32,
    pub conf: Uint128,
}

#[cw_serde]
pub struct PythPriceRetrievalResponse {
    pub price: i64,
    pub conf: u64,
    pub expo: i32,
    pub publish_time: i64,
}

#[cw_serde]
pub struct PriceFeed {
    pub id: String,
    pub price: PythPriceRetrievalResponse,
    pub ema_price: PythPriceRetrievalResponse,
}

#[cw_serde]
pub struct PriceFeedResponse {
    pub price_feed: Option<PriceFeed>,
    pub price: Option<PythPriceRetrievalResponse>,
}

#[cw_serde]
pub enum PythQueryMsg {
    PythConversionPriceFeed { id: String },
    GetPrice { price_id: String },
}
