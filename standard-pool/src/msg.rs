//! Standard-pool wire-format types.
//!
//! Drops the commit-phase variants from creator-pool's ExecuteMsg /
//! QueryMsg (nothing ever invokes Commit, ClaimCreator*, RetryFactoryNotify,
//! ContinueDistribution, RecoverStuckStates, IsFullyCommited,
//! CommittingInfo, LastCommited, PoolCommits, or FactoryNotifyStatus
//! on a standard pool). Every response type is re-exported from
//! `pool_core::msg`, so shared queries round-trip the same JSON shape
//! on both pool kinds.
//!
//! `MigrateMsg` is declared locally (rather than re-exported from
//! creator-pool) because migration semantics are per-contract —
//! future migrations can diverge without cross-crate coupling.

use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Decimal, Timestamp, Uint128};
use cw20::Cw20ReceiveMsg;
use pool_core::asset::TokenInfo;
use pool_core::state::PoolDetails;
use pool_core::msg::{
    ConfigResponse, CumulativePricesResponse, FeeInfoResponse, PoolAnalyticsResponse,
    PoolConfigUpdate, PoolFeeStateResponse, PoolInfoResponse, PoolStateResponse, PositionResponse,
    PositionsResponse, ReverseSimulationResponse, SimulationResponse,
};
use pool_factory_interfaces::{AllPoolsResponse, IsPausedResponse, PoolStateResponseForFactory};

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
    Pause {},
    Unpause {},
    EmergencyWithdraw {},
    CancelEmergencyWithdraw {},
    DepositLiquidity {
        amount0: Uint128,
        amount1: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    AddToPosition {
        position_id: String,
        amount0: Uint128,
        amount1: Uint128,
        min_amount0: Option<Uint128>,
        min_amount1: Option<Uint128>,
        transaction_deadline: Option<Timestamp>,
    },
    CollectFees {
        position_id: String,
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
}

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
    #[returns(PoolInfoResponse)]
    PoolInfo {},
    #[returns(PoolAnalyticsResponse)]
    Analytics {},
    #[returns(PoolStateResponseForFactory)]
    GetPoolState { pool_contract_address: String },
    #[returns(AllPoolsResponse)]
    GetAllPools {},
    #[returns(IsPausedResponse)]
    IsPaused {},
}

#[cw_serde]
pub enum MigrateMsg {
    UpdateFees { new_fees: Decimal },
    UpdateVersion {},
}
