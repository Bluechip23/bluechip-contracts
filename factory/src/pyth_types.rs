//! Pyth on-chain message-shape mirrors.
//!
//! `PythPriceRetrievalResponse`, `PriceFeed`, `PriceFeedResponse`, and
//! `PythQueryMsg` mirror types defined in the upstream Pyth Cosmos
//! contracts. They are re-implemented here (rather than imported) to
//! avoid pulling the full Pyth SDK into the factory's dependency
//! graph. If the on-chain Pyth contract ever bumps its schema, these
//! definitions must be re-checked against the new wire format —
//! `query_pyth_atom_usd_price` deserialization will fail at runtime
//! with a generic StdError if any field is missing or renamed.
//!
//! Source of truth: <https://github.com/pyth-network/pyth-crosschain>
//! (last verified against pyth-sdk-cw v1.x — update this comment when
//! revalidating against a newer release).
//!
//! Mirrors mockoracle::msg::PriceResponse. Used by the
//! `#[cfg(feature = "mock")]` oracle-update path, which reads the
//! current bluechip price directly from the configured mock oracle
//! instead of deriving it from pool TWAPs.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;
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
