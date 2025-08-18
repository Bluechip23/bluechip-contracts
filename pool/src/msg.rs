use cosmwasm_schema::{cw_serde, QueryResponses};

use crate::asset::{Asset, AssetInfo, PairInfo, };

use cosmwasm_std::{Addr, Binary, Decimal, Uint128};


/// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
/// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";

// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;

#[cw_serde]
pub enum ExecuteMsg {
    /// Receives a message of type [`Cw20ReceiveMsg`]
    /// Update the pair configuration
    UpdateConfig {
        params: Binary,
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
    DepositLiquidity {
        amount0: Uint128, // native amount (should be sent with the message)
    },
    AddToPosition {
        position_id: String,
        amount0: Uint128, // native amount (should be sent with the message)
    },
}

/// This structure describes the query messages available in the contract.
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    /// Returns information about a pair in an object of type [`super::asset::PairInfo`].
    #[returns(PairInfo)]
    Pair {},
    /// Returns contract configuration settings in a custom [`ConfigResponse`] structure.
    #[returns(ConfigResponse)]
    Config {},


    #[returns(FeeInfoResponse)]
    FeeInfo {},

}

#[cw_serde]
pub struct PoolInstantiateMsg {
    // tracks the pool and used for querys.
    pub pool_id: u64,
    /// the creator token and bluechip.The creator token will be Token and bluechip will be Native
    pub asset_infos: [AssetInfo; 2],
    /// CW20 contract code ID the pools use to copy into their logic. 
    pub token_code_id: u64,
     /// CW721 contract code ID the pools use to copy into their logic.
     pub position_nft_code_id: Addr,
    /// The factory contract address being used to create the creator pool
    pub factory_addr: Addr,
    //this will be fed into the factory's reply function. It is the threshold payout amounts.
    pub init_params: Option<Binary>,
    //the fee amount going to the creator (5%) and bluechip (1%)
    pub fee_info: FeeInfo,
    // address for the newly created creator token. Autopopulated by the factory reply function
    pub token_address: Addr,
    //the threshold limit for the contract. Once crossed, the pool mints and distributes new creator (CW20 token) and now behaves like a normal liquidity pool
    pub commit_limit_usd: Uint128,
    // the contract of the oracle being used to convert prices to and from dollars
    pub oracle_addr: Addr,
    // the symbol the contract will be looking for for commit messages. the bluechip token's symbol    
    pub oracle_symbol: String,
}


#[cw_serde]
pub struct PoolInitParams {
    // once the threshold is crossed, the amount distributed directly to the creator
    pub creator_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the BlueChip
    pub bluechip_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the newly formed creator pool
    pub pool_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the commiters before the threshold was crossed in proportion to the amount they commited.
    pub commit_amount: Uint128,
}

#[cw_serde]
pub struct FeeInfo {
    //addres bluechip fees from commits accumulate
    pub bluechip_address: Addr,
    //address creator fees from commits accumulate
    pub creator_address: Addr,
    // the amount bluechip earns per commit
    pub bluechip_fee: Decimal,
    // the amount the creator earns per commit
    pub creator_fee: Decimal,
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

/// This structure holds the parameters that are returned from a swap simulation response
#[cw_serde]
pub struct SimulationResponse {
    /// The amount of ask assets returned by the swap
    pub return_amount: Uint128,
    /// The spread used in the swap operation
    pub spread_amount: Uint128,
    /// The amount of fees charged by the transaction
    pub commission_amount: Uint128,
}

/// This structure holds the parameters that are returned from a reverse swap simulation response.
#[cw_serde]
pub struct ReverseSimulationResponse {
    /// The amount of offer assets returned by the reverse swap
    pub offer_amount: Uint128,
    /// The spread used in the swap operation
    pub spread_amount: Uint128,
    /// The amount of fees charged by the transaction
    pub commission_amount: Uint128,
}

/// This structure is used to return a cumulative prices query response.
#[cw_serde]
pub struct CumulativePricesResponse {
    /// The two assets in the pool to query
    pub assets: [Asset; 2],
    // The last value for the token0 cumulative price
    pub price0_cumulative_last: Uint128,
    /// The last value for the token1 cumulative price
    pub price1_cumulative_last: Uint128,
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

//everything below this is things you do not need to know right now. Just needs to be here for instantiation to work. 
#[cw_serde]
pub struct PoolStateResponse {
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128,
    pub reserve1: Uint128,
    pub total_liquidity: Uint128,
    pub block_time_last: u64,
}

#[cw_serde]
pub struct PoolFeeStateResponse {
    pub fee_growth_global_0: Decimal,
    pub fee_growth_global_1: Decimal,
    pub total_fees_collected_0: Uint128,
    pub total_fees_collected_1: Uint128,
}
