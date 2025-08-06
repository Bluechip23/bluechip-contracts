use cosmwasm_schema::{cw_serde, QueryResponses};

use crate::asset::{Asset, AssetInfo, PairInfo};

use cosmwasm_std::{Addr, Binary, Decimal, Uint128};
use cw20::Cw20ReceiveMsg;

/// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
/// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";

// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;

/// This structure describes the parameters used for creating a contract.
#[cw_serde]
pub struct PairInstantiateMsg {
    /// Information about the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    /// The token contract code ID used for the tokens in the pool
    pub token_code_id: u64,
    /// The factory contract address
    pub factory_addr: Addr,
    /// Optional binary serialised parameters for custom pool types
    pub init_params: Option<Binary>,
    pub fee_info: FeeInfo,
    pub commit_limit: Uint128,
    pub token_address: Addr,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,     
    pub oracle_symbol: String,
    pub available_payment: Vec<Uint128>,
    pub available_payment_usd: Vec<Uint128>,
}

#[cw_serde]
pub struct PoolInitParams {
    pub creator_amount: Uint128,
    pub bluechip_amount: Uint128,
    pub pool_amount: Uint128,
    pub commit_amount: Uint128,
}

#[cw_serde]
pub struct FeeInfo {
    pub bluechip_address: Addr,
    pub creator_address: Addr,
    pub bluechip_fee: Decimal,
    pub creator_fee: Decimal,
}

/// This structure describes the execute messages available in the contract.
#[cw_serde]
pub enum ExecuteMsg {
    /// Receives a message of type [`Cw20ReceiveMsg`]
    Receive(Cw20ReceiveMsg),
    /// Swap performs a swap in the pool
    Swap {
        offer_asset: Asset,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
    },
    /// Update the pair configuration
    UpdateConfig {
        params: Binary,
    },

    Commit {
        asset: Asset,
    },
}

/// This structure describes a CW20 hook message.
#[cw_serde]
pub enum Cw20HookMsg {
    /// Swap a given amount of asset
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
    },
}

/// This structure describes the query messages available in the contract.
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    
    #[returns(PairInfo)]
    Pair {},
    /// Returns information about a pool in an object of type [`PoolResponse`].
    #[returns(PoolResponse)]
    Pool {},
    /// Returns contract configuration settings in a custom [`ConfigResponse`] structure.
    #[returns(ConfigResponse)]
    Config {},

}

/// This struct is used to return a query result with the total amount of LP tokens and the two assets in a specific pool.
#[cw_serde]
pub struct PoolResponse {
    /// The assets in the pool together with asset amounts
    pub assets: [Asset; 2],
}

/// This struct is used to return a query result with the general contract configuration.
#[cw_serde]
pub struct ConfigResponse {
    /// Last timestamp when the cumulative prices in the pool were updated
    pub block_time_last: u64,
    /// The pool's parameters
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct FeeInfoResponse {
    /// The two assets in the pool to query
    pub fee_info: FeeInfo,
}

/// This structure describes a migration message.
/// We currently take no arguments for migrations.
#[cw_serde]
pub struct MigrateMsg {}

