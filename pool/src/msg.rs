use crate::asset::{PoolDetails, TokenInfo, TokenType};
use crate::state::Commiting;
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Binary, Decimal, Timestamp, Uint128};
use cw20::Cw20ReceiveMsg;

#[cw_serde]
pub enum ExecuteMsg {
    Receive(Cw20ReceiveMsg),
    SimpleSwap {
        offer_asset: TokenInfo,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
    UpdateConfigFromFactory {
        update: PoolConfigUpdate,
    },
    ContinueDistribution {},
    Commit {
        asset: TokenInfo,
        amount: Uint128,
        transaction_deadline: Option<Timestamp>,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
    },
    DepositLiquidity {
        amount0: Uint128,
        amount1: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    CollectFees {
        position_id: String,
    },
    AddToPosition {
        position_id: String,
        amount0: Uint128, // bluechip token amount
        amount1: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    RemovePartialLiquidity {
        position_id: String,
        liquidity_to_remove: Uint128,
        transaction_deadline: Option<Timestamp>, // Specific amount of liquidity to remove
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
    },
    RemovePartialLiquidityByPercent {
        position_id: String,
        percentage: u64,
        transaction_deadline: Option<Timestamp>,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
    },
    RemoveAllLiquidity {
        position_id: String,
        transaction_deadline: Option<Timestamp>,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
    },
}

#[cw_serde]
pub enum MigrateMsg {
    UpdateFees { new_fees: Decimal },
    UpdateVersion {}, // Simple version update
}

#[cw_serde]
pub enum Cw20HookMsg {
    // Swap a given amount of asset
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
    DepositLiquidity {
        amount0: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    AddToPosition {
        position_id: String,
        amount0: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
}

// This structure describes the query messages available in the contract.
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(PoolDetails)]
    Pair {},
    #[returns(ConfigResponse)]
    Config {},
    #[returns(SimulationResponse)]
    Simulation { offer_asset: TokenInfo },
    #[returns(ReverseSimulationResponse)]
    ReverseSimulation { ask_asset: TokenInfo },
    #[returns(CumulativePricesResponse)]
    CumulativePrices {},

    #[returns(FeeInfoResponse)]
    FeeInfo {},

    #[returns(CommitStatus)]
    IsFullyCommited {},

    #[returns(Option<Commiting>)]
    CommitingInfo { wallet: String },

    #[returns(PoolCommitResponse)]
    PoolCommits {
        pool_contract_address: Addr,
        min_payment_usd: Option<Uint128>,
        after_timestamp: Option<u64>, // Unix timestamp
        start_after: Option<String>,  // For pagination
        limit: Option<u32>,
    },

    #[returns(PoolStateResponse)]
    PoolState {},

    #[returns(PoolFeeStateResponse)]
    FeeState {},

    #[returns(PositionResponse)]
    Position { position_id: String },

    #[returns(PositionsResponse)]
    Positions {
        start_after: Option<String>,
        limit: Option<u32>,
    },

    #[returns(PositionsResponse)]
    PositionsByOwner {
        owner: String,
        start_after: Option<String>,
        limit: Option<u32>,
    },
    #[returns(LastCommitedResponse)]
    LastCommited { wallet: String },

    #[returns(PoolInfoResponse)]
    PoolInfo {},
}

#[cw_serde]
pub struct PoolInstantiateMsg {
    pub pool_id: u64,
    // Information about the two assets in the pool
    pub pool_token_info: [TokenType; 2],
    // The token contract code ID used for the tokens in the pool
    pub cw20_token_contract_id: u64,
    // The factory contract address
    pub used_factory_addr: Addr,
    // gets set in reply function - amounts that go to each payout party
    pub threshold_payout: Option<Binary>,
    pub commit_fee_info: CommitFeeInfo,
    pub commit_threshold_limit_usd: Uint128,
    pub commit_amount_for_threshold: Uint128,
    pub position_nft_address: Addr,
    pub token_address: Addr,
}

#[cw_serde]
pub struct PoolCommitResponse {
    //numbe of total commits
    pub total_count: u32,
    //lists of wallets commited
    pub commiters: Vec<CommiterInfo>,
}

#[cw_serde]
pub struct PoolConfigUpdate {
    pub commit_fee_info: Option<CommitFeeInfo>,
    pub commit_limit_usd: Option<Uint128>,
    pub pyth_contract_addr_for_conversions: Option<String>,
    pub pyth_atom_usd_price_feed_id: Option<String>,
    pub commit_amount_for_threshold: Option<Uint128>,
    pub threshold_payout: Option<Binary>,
    pub cw20_token_contract_id: Option<u64>,
    pub cw721_nft_contract_id: Option<u64>,
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: u64,
    pub usd_payment_tolerance_bps: u16,
}

#[cw_serde]
pub struct CommiterInfo {
    pub wallet: String,
    //last payment in bluechip amount
    pub last_payment_bluechip: Uint128,
    //last payment converted to USD
    pub last_payment_usd: Uint128,
    pub last_commited: Timestamp,
    pub total_paid_usd: Uint128,
}
#[cw_serde]
pub struct CommitFeeInfo {
    //BlueChip wallet
    pub bluechip_wallet_address: Addr,
    //pool creatpr wallet
    pub creator_wallet_address: Addr,
    //amount of commit that goes to BlueChip
    pub commit_fee_bluechip: Decimal,
    //amount of commit taht goes to pool creator
    pub commit_fee_creator: Decimal,
}

#[cw_serde]
pub struct PoolResponse {
    // The assets in the pool together with asset amounts
    pub assets: [TokenInfo; 2],
}

#[cw_serde]
pub struct ConfigResponse {
    // Last timestamp when the cumulative prices in the pool were updated
    pub block_time_last: u64,
    // The pool's parameters
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct LastCommitedResponse {
    //has wallet sent a commit transaction
    pub has_commited: bool,
    //last time commiting
    pub last_commited: Option<Timestamp>,
    //last payment in bluechip
    pub last_payment_bluechip: Option<Uint128>,
    //last payment converted to usd
    pub last_payment_usd: Option<Uint128>,
}

#[cw_serde]
pub struct SimulationResponse {
    //amount of ask assets returned by the swap
    pub return_amount: Uint128,
    // spread used in the swap operation
    pub spread_amount: Uint128,
    //amount of fees charged by the transaction
    pub commission_amount: Uint128,
}

#[cw_serde]
pub struct ReverseSimulationResponse {
    // The amount of offer assets returned by the reverse swap
    pub offer_amount: Uint128,
    // The spread used in the swap operation
    pub spread_amount: Uint128,
    //The amount of fees charged by the transaction
    pub commission_amount: Uint128,
}

// This structure is used to return a cumulative prices query response.
#[cw_serde]
pub struct CumulativePricesResponse {
    // The two assets in the pool to query
    pub assets: [TokenInfo; 2],
    // The last value for the token0 cumulative price
    pub price0_cumulative_last: Uint128,
    // The last value for the token1 cumulative price
    pub price1_cumulative_last: Uint128,
}

#[cw_serde]
pub struct FeeInfoResponse {
    // The two assets in the pool to query
    pub fee_info: CommitFeeInfo,
}

#[cw_serde]
pub struct StablePoolParams {
    // The current stableswap pool amplification
    pub amp: u64,
}

#[cw_serde]
pub struct StablePoolConfig {
    // The stableswap pool amplification
    pub amp: Decimal,
}

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

#[cw_serde]
pub struct PoolStateResponse {
    pub nft_ownership_accepted: bool,
    //asset 0 amount
    pub reserve0: Uint128,
    //asset 1 amount
    pub reserve1: Uint128,
    //total liquidity in pool
    pub total_liquidity: Uint128,
    pub block_time_last: u64,
}

#[cw_serde]
pub struct PoolFeeStateResponse {
    //total fees generated by asset 0 inside pool
    pub fee_growth_global_0: Decimal,
    //total fees generated by asset 1 inside pool
    pub fee_growth_global_1: Decimal,
    //total fees collected by positions for asset 0
    pub total_fees_collected_0: Uint128,
    //total fees collected by positions for asset 1
    pub total_fees_collected_1: Uint128,
}

#[cw_serde]
pub struct PositionResponse {
    pub position_id: String,
    pub liquidity: Uint128,
    //wallet address
    pub owner: Addr,
    // fee_growth_global_0 was when position last collected - this is local to this position
    pub fee_growth_inside_0_last: Decimal,
    // fee_growth_global_1 was when position last collected - this is local to this position
    pub fee_growth_inside_1_last: Decimal,
    pub created_at: u64,
    //last time position collected fees from pool.
    pub last_fee_collection: u64,
    //fee_growth_global - fee_growth_inside
    pub unclaimed_fees_0: Uint128,
    pub unclaimed_fees_1: Uint128,
}

#[cw_serde]
pub struct PositionsResponse {
    pub positions: Vec<PositionResponse>,
}

#[cw_serde]
pub struct PoolInfoResponse {
    pub pool_state: PoolStateResponse,
    pub fee_state: PoolFeeStateResponse,
    pub total_positions: u64,
}
