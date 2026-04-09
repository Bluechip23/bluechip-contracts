use cosmwasm_schema::cw_serde;

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
