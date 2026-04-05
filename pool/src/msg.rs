#[allow(unused_imports)]
use crate::asset::{PoolPairInfo, TokenInfo, TokenType};
#[allow(unused_imports)]
use crate::state::{Commiting, PoolAnalytics, RecoveryType};
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Binary, Decimal, Timestamp, Uint128};
use cw20::Cw20ReceiveMsg;
#[allow(unused_imports)]
use pool_factory_interfaces::{AllPoolsResponse, PoolStateResponseForFactory};

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
    RecoverStuckStates {
        recovery_type: RecoveryType,
    },
    ContinueDistribution {},
    Pause {},
    Unpause {},
    EmergencyWithdraw {},
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
        amount0: Uint128,
        amount1: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    RemovePartialLiquidity {
        position_id: String,
        liquidity_to_remove: Uint128,
        transaction_deadline: Option<Timestamp>,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        max_ratio_deviation_bps: Option<u16>,
    },
    RemovePartialLiquidityByPercent {
        position_id: String,
        percentage: u64,
        transaction_deadline: Option<Timestamp>,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        max_ratio_deviation_bps: Option<u16>,
    },
    RemoveAllLiquidity {
        position_id: String,
        transaction_deadline: Option<Timestamp>,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        max_ratio_deviation_bps: Option<u16>,
    },
    ClaimCreatorExcessLiquidity {},
    CancelEmergencyWithdraw {},
}

#[cw_serde]
pub enum MigrateMsg {
    UpdateFees { new_fees: Decimal },
    UpdateVersion {},
}

#[cw_serde]
pub enum Cw20HookMsg {
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(PoolPairInfo)]
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
    #[returns(Option<Committing>)]
    CommittingInfo { wallet: String },
    #[returns(PoolCommitResponse)]
    PoolCommits {
        pool_contract_address: Addr,
        min_payment_usd: Option<Uint128>,
        after_timestamp: Option<u64>,
        start_after: Option<String>,
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
    #[returns(LastCommittedResponse)]
    LastCommited { wallet: String },
    #[returns(PoolInfoResponse)]
    PoolInfo {},
    #[returns(PoolAnalyticsResponse)]
    Analytics {},
    #[returns(PoolStateResponseForFactory)]
    GetPoolState { pool_contract_address: String },
    #[returns(AllPoolsResponse)]
    GetAllPools {},
}

#[cw_serde]
pub struct PoolInstantiateMsg {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    pub cw20_token_contract_id: u64,
    pub used_factory_addr: Addr,
    pub threshold_payout: Option<Binary>,
    pub commit_fee_info: CommitFeeInfo,
    pub commit_threshold_limit_usd: Uint128,
    pub commit_amount_for_threshold: Uint128,
    pub position_nft_address: Addr,
    pub token_address: Addr,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
    pub is_standard_pool: Option<bool>,
}

#[cw_serde]
pub struct PoolCommitResponse {
    pub total_count: u32,
    pub committers: Vec<CommitterInfo>,
}

#[cw_serde]
pub struct PoolConfigUpdate {
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: Option<u64>,
    pub usd_payment_tolerance_bps: Option<u16>,
    pub oracle_address: Option<String>,
}

#[cw_serde]
pub struct CommitterInfo {
    pub wallet: String,
    pub last_payment_bluechip: Uint128,
    pub last_payment_usd: Uint128,
    pub last_committed: Timestamp,
    pub total_paid_usd: Uint128,
    pub total_paid_bluechip: Uint128,
}

#[cw_serde]
pub struct CommitFeeInfo {
    pub bluechip_wallet_address: Addr,
    pub creator_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
}

#[cw_serde]
pub struct PoolResponse {
    pub assets: [TokenInfo; 2],
}

#[cw_serde]
pub struct ConfigResponse {
    pub block_time_last: u64,
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct LastCommittedResponse {
    pub has_committed: bool,
    pub last_committed: Option<Timestamp>,
    pub last_payment_bluechip: Option<Uint128>,
    pub last_payment_usd: Option<Uint128>,
}

#[cw_serde]
pub struct SimulationResponse {
    pub return_amount: Uint128,
    pub spread_amount: Uint128,
    pub commission_amount: Uint128,
}

#[cw_serde]
pub struct ReverseSimulationResponse {
    pub offer_amount: Uint128,
    pub spread_amount: Uint128,
    pub commission_amount: Uint128,
}

#[cw_serde]
pub struct CumulativePricesResponse {
    pub assets: [TokenInfo; 2],
    pub price0_cumulative_last: Uint128,
    pub price1_cumulative_last: Uint128,
}

#[cw_serde]
pub struct FeeInfoResponse {
    pub fee_info: CommitFeeInfo,
}

#[cw_serde]
pub enum CommitStatus {
    InProgress { raised: Uint128, target: Uint128 },
    FullyCommitted,
}

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

#[cw_serde]
pub struct PositionResponse {
    pub position_id: String,
    pub liquidity: Uint128,
    pub owner: Addr,
    pub fee_growth_inside_0_last: Decimal,
    pub fee_growth_inside_1_last: Decimal,
    pub created_at: u64,
    pub last_fee_collection: u64,
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

#[cw_serde]
pub struct PoolAnalyticsResponse {
    pub analytics: PoolAnalytics,
    pub current_price_0_to_1: String,
    pub current_price_1_to_0: String,
    pub total_value_locked_0: Uint128,
    pub total_value_locked_1: Uint128,
    pub fee_reserve_0: Uint128,
    pub fee_reserve_1: Uint128,
    pub threshold_status: CommitStatus,
    pub total_usd_raised: Uint128,
    pub total_bluechip_raised: Uint128,
    pub total_positions: u64,
}
