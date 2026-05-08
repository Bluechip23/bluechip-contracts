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

// Response types referenced ONLY by the `#[returns(T)]` annotations on
// `QueryMsg` variants below. cosmwasm-schema's `QueryResponses` derive
// reads these when the `schema` feature is active (`cargo schema`), but
// the wasm release build drops the derive and sees them as unused —
// hence the per-import `#[allow(unused_imports)]` annotations.
//
// Important: do NOT collapse these into the regular `use` blocks above.
// `cargo fix` (and tooling that piggy-backs on it) does not understand
// the cfg-gated derive that consumes them and will helpfully delete
// every one, breaking schema generation. Each annotation is per-import
// so a future addition only needs to copy the surrounding line shape.
//
// If schema generation breaks after a `cargo fix` run, this block is
// the most likely culprit — restore the imports + the allow attributes
// from git history.
#[allow(unused_imports)]
use pool_core::msg::{
    ConfigResponse, CumulativePricesResponse, FeeInfoResponse, PoolAnalyticsResponse,
    PoolConfigUpdate, PoolFeeStateResponse, PoolInfoResponse, PoolStateResponse, PositionResponse,
    PositionsResponse, ReverseSimulationResponse, SimulationResponse,
};
#[allow(unused_imports)]
use pool_core::state::PoolDetails;
#[allow(unused_imports)]
use pool_factory_interfaces::{AllPoolsResponse, IsPausedResponse, PoolStateResponseForFactory};

#[cw_serde]
pub enum ExecuteMsg {
    Receive(Cw20ReceiveMsg),
    SimpleSwap {
        offer_asset: TokenInfo,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        #[serde(default)]
        allow_high_max_spread: Option<bool>,
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
    /// Factory-only callback dispatched at the tail of the factory's
    /// `finalize_standard_pool` reply chain. The NFT contract
    /// has already been told `TransferOwnership { new_owner: pool }`;
    /// this variant lets the pool itself send the matching
    /// `AcceptOwnership` message back to the NFT contract, closing the
    /// pending-ownership window IN THE SAME TRANSACTION as pool
    /// creation rather than waiting for the first user deposit.
    ///
    /// Idempotent: if `pool_state.nft_ownership_accepted` is already
    /// true (e.g. the first deposit happened to land first), the
    /// handler is a no-op. Authorisation: sender must equal
    /// `POOL_INFO.factory_addr`.
    AcceptNftOwnership {},
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

    // H-NFT-4 audit fix: per-position claim against the post-emergency-
    // drain escrow. Caller must own the position NFT (CW721 OwnerOf
    // check). Each position can be claimed exactly once; a successful
    // claim sets `position.liquidity = 0` and bumps the snapshot's
    // `total_claimed_*` running tally. Funds = pro-rata share of
    // `(reserve_*_at_drain + fee_reserve_*_at_drain)` weighted by
    // `position.liquidity / total_liquidity_at_drain`, transferred to
    // `info.sender`.
    //
    // Available immediately post-drain through the full 1-year
    // `EMERGENCY_CLAIM_DORMANCY_SECONDS` window. After dormancy, claims
    // still compute math but the pool's bank balance may have been
    // swept by `SweepUnclaimedEmergencyShares`, so a late claim
    // would error on insufficient balance.
    ClaimEmergencyShare {
        position_id: String,
    },

    // H-NFT-4 audit fix: factory-only post-dormancy sweep. After 1
    // year, factory admin sends the unclaimed residual to the
    // bluechip wallet. One-shot; `residual_swept` flag prevents
    // double-sweeps.
    SweepUnclaimedEmergencyShares {},
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
    /// Returns the LP-side pool state shape — `PoolStateResponse`
    /// (reserves, total liquidity, cumulative prices, NFT-ownership
    /// flag). This is the response used by frontends and SDKs.
    /// Do NOT confuse with the factory-facing `GetPoolState {}`
    /// variant below, which returns a DIFFERENT type
    /// (`PoolStateResponseForFactory`) consumed by the factory's
    /// oracle / liquidity-snapshot machinery.
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
    /// Factory-facing variant — returns `PoolStateResponseForFactory`
    /// (different shape from the LP-side `PoolState {}` above).
    /// Forwarded to `pool_core::query::query_for_factory`. Frontend
    /// consumers should use `PoolState {}` instead.
    #[returns(PoolStateResponseForFactory)]
    GetPoolState {},
    /// Factory-facing list query. Returns `AllPoolsResponse`. Used by
    /// the factory's pool-set scans; not intended for direct LP / SDK
    /// consumption.
    #[returns(AllPoolsResponse)]
    GetAllPools {},
    /// Factory-facing pause-status query. Returns `IsPausedResponse`.
    /// Used by the factory's oracle / health-checks; LP-facing pause
    /// state is exposed indirectly via `PoolState {}` reserves and the
    /// per-handler error responses.
    #[returns(IsPausedResponse)]
    IsPaused {},
}

#[cw_serde]
pub enum MigrateMsg {
    /// Tune `PoolSpecs.lp_fee` to `new_fees`. Accepted range:
    /// `MIN_LP_FEE` (0.1% / `Decimal::permille(1)`) up to
    /// `MAX_LP_FEE` (10% / `Decimal::percent(10)`) inclusive. Values
    /// outside this range are rejected at runtime with
    /// `ContractError::LpFeeOutOfRange`. The schema accepts any
    /// `Decimal` so client tooling that wants to encode the bounds
    /// must do so out-of-band; the runtime gate is authoritative.
    UpdateFees { new_fees: Decimal },
    /// No-op variant. Bumps the cw2 stored version on a successful
    /// migrate without touching any other state. Use when the only
    /// change between releases is the wasm code id.
    UpdateVersion {},
}
