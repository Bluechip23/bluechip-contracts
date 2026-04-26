//! Creator-pool (commit-phase) wire-format types.
//!
//! Shared types (response structs, CommitFeeInfo, PoolConfigUpdate,
//! Cw20HookMsg, CommitStatus) live in `pool_core::msg` and are
//! re-exported below so every existing `use crate::msg::X;` import in
//! the creator-pool crate resolves unchanged.
//!
//! Per-contract types — the ExecuteMsg / QueryMsg / MigrateMsg /
//! PoolInstantiateMsg enums and the commit-only response types
//! (FactoryNotifyStatusResponse, PoolCommitResponse, CommitterInfo,
//! LastCommittedResponse) — stay here. Standard-pool (Step 4b) defines
//! its own slimmer versions in its own `msg.rs`.
pub use pool_core::msg::*;

use crate::asset::{TokenInfo, TokenType};
use crate::state::RecoveryType;
// Schema-only refs: cited only by `#[returns(...)]` on QueryMsg
// variants. The QueryResponses derive consumes them but rustc still
// flags them as unused without this allow. Grouping them under one
// outer allow keeps the schema-suppression scoped (so an unused-import
// on `TokenInfo` / `RecoveryType` would still get reported), and folds
// the four prior `#[allow(unused_imports)]` directives into a single
// block. `PoolAnalytics` was removed — it was never referenced.
#[allow(unused_imports)]
use {
    crate::state::{Committing, PoolDetails},
    pool_factory_interfaces::{AllPoolsResponse, PoolStateResponseForFactory},
};
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
    RecoverStuckStates {
        recovery_type: RecoveryType,
    },
    ContinueDistribution {},
    Pause {},
    Unpause {},
    EmergencyWithdraw {},
    Commit {
        asset: TokenInfo,
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
    ClaimCreatorExcessLiquidity {
        // Optional deadline protecting the claim from lying in the mempool
        // indefinitely. Unset preserves the pre-existing behavior for
        // backwards-compatibility with already-built clients.
        #[serde(default)]
        transaction_deadline: Option<Timestamp>,
    },
    // Empties the CREATOR_FEE_POT into the creator wallet. The pot
    // accumulates the portion of LP fees that the fee-size multiplier
    // clipped off small positions — previously orphaned in fee_reserve,
    // now routed here. Creator-only.
    ClaimCreatorFees {
        #[serde(default)]
        transaction_deadline: Option<Timestamp>,
    },
    // Re-sends NotifyThresholdCrossed to the factory when the initial
    // notification during threshold-crossing failed and PENDING_FACTORY_NOTIFY
    // is set. Anyone can call: factory's POOL_THRESHOLD_MINTED idempotency
    // check gates double-mints, so at worst a stray caller burns gas on a
    // no-op. Clears the pending flag on successful reply.
    RetryFactoryNotify {},
    CancelEmergencyWithdraw {},
}

#[cw_serde]
pub enum MigrateMsg {
    UpdateFees { new_fees: Decimal },
    UpdateVersion {},
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
    GetPoolState {},
    #[returns(AllPoolsResponse)]
    GetAllPools {},
    #[returns(pool_factory_interfaces::IsPausedResponse)]
    IsPaused {},
    // Reports whether a NotifyThresholdCrossed-to-factory notification
    // is pending retry (see PENDING_FACTORY_NOTIFY / RetryFactoryNotify).
    // Useful for keepers and ops dashboards watching for stuck pools.
    #[returns(FactoryNotifyStatusResponse)]
    FactoryNotifyStatus {},
}

#[cw_serde]
pub struct FactoryNotifyStatusResponse {
    pub pending: bool,
}

/// Instantiate message dispatched by the factory to a freshly created pool
/// wasm. Tagged enum so the pool's `instantiate` entry point can receive
/// either the commit-pool or standard-pool wire format and branch on
/// which variant was sent.
///
/// Flat struct — standard pools live in their own wasm now (see
/// `standard-pool` crate) so creator-pool's instantiate only ever
/// receives this shape. The factory sends it directly via
/// `WasmMsg::Instantiate { code_id: create_pool_wasm_contract_id, ... }`.
#[cw_serde]
pub struct PoolInstantiateMsg {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    pub cw20_token_contract_id: u64,
    pub used_factory_addr: Addr,
    pub threshold_payout: Option<Binary>,
    pub commit_fee_info: CommitFeeInfo,
    pub commit_threshold_limit_usd: Uint128,
    pub position_nft_address: Addr,
    pub token_address: Addr,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
}

#[cw_serde]
pub struct PoolCommitResponse {
    pub total_count: u32,
    pub committers: Vec<CommitterInfo>,
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
pub struct LastCommittedResponse {
    pub has_committed: bool,
    pub last_committed: Option<Timestamp>,
    pub last_payment_bluechip: Option<Uint128>,
    pub last_payment_usd: Option<Uint128>,
}
