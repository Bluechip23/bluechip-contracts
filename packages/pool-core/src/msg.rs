//! Shared wire-format types used by both creator-pool and standard-pool.
//!
//! Split boundary:
//!   - Shared (this module): CommitFeeInfo, PoolConfigUpdate, Cw20HookMsg,
//!     CommitStatus, and every response struct returned by a query
//!     handler that lives in `pool_core::query`.
//!   - Per-contract (in creator-pool / standard-pool): ExecuteMsg,
//!     QueryMsg, MigrateMsg, PoolInstantiateMsg / CommitPoolInstantiateMsg,
//!     and commit-only response types (FactoryNotifyStatusResponse,
//!     PoolCommitResponse, CommitterInfo, LastCommittedResponse).
//!
//! Wire format is preserved — every struct moves with its `#[cw_serde]`
//! attribute intact, so JSON shapes (field names, nested layouts) are
//! byte-for-byte identical to the creator-pool pre-split build.

use crate::asset::TokenInfo;
use crate::state::PoolAnalytics;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, Timestamp, Uint128};

#[cw_serde]
pub struct CommitFeeInfo {
    pub bluechip_wallet_address: Addr,
    pub creator_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
}

#[cw_serde]
pub struct PoolConfigUpdate {
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: Option<u64>,
    // `usd_payment_tolerance_bps` removed — see `PoolSpecs` doc-comment
    // in `pool-core::state` for rationale.
    //
    // `oracle_address` removed (audit fix). Per-pool oracle rotation is a
    // documented admin-compromise vector: a malicious oracle can return
    // arbitrary `ConversionResponse.amount`, letting a $5 commit register
    // as a $25k threshold-cross and capturing the full pool seed +
    // creator/bluechip rewards on a single pool. There is no current
    // operational need to point an individual pool at a non-factory
    // oracle. If a future architecture splits the oracle off the
    // factory, the accepted re-routing path is a coordinated wasm
    // migration via `UpgradePools` (already 48h-timelocked + batched)
    // that updates `ORACLE_INFO` directly, not a per-pool config knob.
}

#[cw_serde]
pub enum Cw20HookMsg {
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        #[serde(default)]
        allow_high_max_spread: Option<bool>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
}

#[cw_serde]
pub enum CommitStatus {
    InProgress { raised: Uint128, target: Uint128 },
    FullyCommitted,
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
