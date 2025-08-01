use cosmwasm_schema::{cw_serde, QueryResponses};

use crate::asset::{Asset, AssetInfo, PairInfo, PaymentInfoResponse, USDTierInfoWithTolerance};
use crate::state::Subscription;
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
pub struct PoolInstantiateMsg {
    pub pool_id: u64,
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
    pub commit_limit_usd: Uint128,
    pub position_nft_address: Addr,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
    pub available_payment_usd: Vec<Uint128>,
    pub available_payment: Vec<Uint128>,
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
    SimpleSwap {
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
        amount: Uint128,
    },
    DepositLiquidity {
        amount0: Uint128,
        amount1: Uint128,
    },
    /// Collect fees owed to a given position
    CollectFees {
        position_id: String,
    },
    AddToPosition {
        position_id: String,
        amount0: Uint128, // native token amount
        amount1: Uint128, // cw20 token amount
    },
    RemovePartialLiquidity {
        position_id: String,
        liquidity_to_remove: Decimal,
    },
    RemovePartialLiquidityByPercent {
        position_id: String,
        percentage: u64, // 1-99
    },
    RemoveLiquidity {
        position_id: String,
    },
    /// Withdraw (and eventually burn) part or all of the liquidity
    WithdrawPosition {
        position_id: String,
        liquidity: Uint128,
    },

    ReplaceAllPaymentTiers {
        new_payment_tiers: Vec<Uint128>,
    },
    AddPaymentTiers {
        tiers_to_add: Vec<Uint128>,
    },

    /// Remove specific payment tiers - only callable by creator
    RemovePaymentTiers {
        tiers_to_remove: Vec<Uint128>,
    },
    ReplaceAllUsdPaymentTiers {
        new_payment_tiers_usd: Vec<Uint128>,
    },
    AddUsdPaymentTiers {
        tiers_to_add_usd: Vec<Uint128>,
    },

    /// Remove specific payment tiers - only callable by creator
    RemoveUsdPaymentTiers {
        tiers_to_remove_usd: Vec<Uint128>,
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
    #[returns(USDTierInfoWithTolerance)]
    CreatorTierInfo {},

    #[returns(PaymentInfoResponse)]
    PaymentInfo {},
    /// Returns information about a pair in an object of type [`super::asset::PairInfo`].
    #[returns(PairInfo)]
    Pair {},
    /// Returns information about a pool in an object of type [`PoolResponse`].
    #[returns(PoolResponse)]
    Pool {},
    /// Returns contract configuration settings in a custom [`ConfigResponse`] structure.
    #[returns(ConfigResponse)]
    Config {},
    /// Returns information about a swap simulation in a [`SimulationResponse`] object.
    #[returns(SimulationResponse)]
    Simulation { offer_asset: Asset },
    /// Returns information about cumulative prices in a [`CumulativePricesResponse`] object.
    #[returns(ReverseSimulationResponse)]
    ReverseSimulation { ask_asset: Asset },
    /// Returns information about the cumulative prices in a [`CumulativePricesResponse`] object
    #[returns(CumulativePricesResponse)]
    CumulativePrices {},

    #[returns(FeeInfoResponse)]
    FeeInfo {},

    #[returns(CommitStatus)]
    IsFullyCommited {},

    #[returns(Option<Subscription>)]
    SubscriptionInfo { wallet: String },

    #[returns(bool)]
    IsSubscribed { wallet: String },
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
pub struct SubscriptionResponse {}
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

/// This structure holds stableswap pool parameters.
#[cw_serde]
pub struct StablePoolParams {
    /// The current stableswap pool amplification
    pub amp: u64,
}

/// This structure stores a stableswap pool's configuration.
#[cw_serde]
pub struct StablePoolConfig {
    /// The stableswap pool amplification
    pub amp: Decimal,
}

/// This enum stores the options available to start and stop changing a stableswap pool's amplification.
#[cw_serde]
pub enum StablePoolUpdateParams {
    StartChangingAmp { next_amp: u64, next_amp_time: u64 },
    StopChangingAmp {},
}
#[cw_serde]
pub enum CommitStatus {
    InProgress { raised: Uint128, target: Uint128 },
    FullyCommitted,
}
