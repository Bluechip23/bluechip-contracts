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
// block.
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
        #[serde(default)]
        allow_high_max_spread: Option<bool>,
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
        /// Optional caller-supplied deadline. Same shape as the
        /// `transaction_deadline` field on every other liquidity path;
        /// rejects mempool-replayed collects that land after the caller's
        /// intended expiry. Pure fee math grows monotonically, so a late
        /// collect can only return *more* fees — not a security gate
        /// against value loss, but kept for client-side symmetry so a
        /// frontend can pass the same deadline to every LP action.
        /// `#[serde(default)]` keeps pre-this-field clients
        /// wire-compatible (the field deserializes as `None` when absent).
        #[serde(default)]
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

    // Factory-only escape hatch for distribution liveness. Removes a single
    // committer's COMMIT_LEDGER row, computes their pro-rata reward, and
    // moves the amount into FAILED_MINTS so the user can claim it later via
    // ClaimFailedDistribution against an alternate recipient. Use when a
    // committer's address is genuinely un-mintable (e.g., a contract
    // recipient that rejects CW20 mint hooks, a CW20 with a future
    // blacklist) and the per-mint reply isolation isn't enough — for
    // example, when iteration over the ledger itself fails on a corrupt
    // row.
    //
    // Resets `consecutive_failures` and re-enables `is_distributing` so
    // distribution resumes without an additional admin call.
    SkipDistributionUser {
        user: String,
    },

    // Permissionless distribution restart for the catastrophic case where
    // the admin path is unavailable for an extended period. Available
    // only after PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS (7 days)
    // since the last successful batch — the admin's 1h window has many
    // chances to fire first. Restarts the cursor at None and resets
    // failure counters; preserves `distributed_so_far` so dust settlement
    // still mints exactly the post-distribution residual.
    SelfRecoverDistribution {},

    // Withdraw a previously-failed distribution mint. Caller must have a
    // non-zero entry in FAILED_MINTS (the original committer address).
    // Optional `recipient` lets the user route the claim to a fresh
    // wallet — useful when the original recipient is the reason the mint
    // failed (e.g., a contract that rejects CW20 receive). Defaults to
    // `info.sender` so the simple case requires no parameters.
    //
    // Mint is dispatched as a reply_always SubMsg using the same
    // isolation harness as the bulk distribution path: if it fails again
    // (e.g., the alternate recipient is also blocked) the amount is
    // re-stashed into FAILED_MINTS under the original committer address
    // so they can try again with yet another recipient.
    ClaimFailedDistribution {
        recipient: Option<String>,
    },

    // per-position claim against the post-emergency
    // -drain escrow. Caller must own (CW721) the position NFT — the
    // handler checks `verify_position_ownership` against the stored NFT
    // contract. Each position can be claimed exactly once; a successful
    // claim sets `position.liquidity = 0` and bumps the snapshot's
    // `total_claimed_*` running tally. The funds the claimant receives
    // are their pro-rata share of `(reserve_*_at_drain +
    // fee_reserve_*_at_drain) * position.liquidity /
    // total_liquidity_at_drain`, transferred to `info.sender`.
    //
    // Available immediately after Phase-2 drain and through the full
    // 1-year `EMERGENCY_CLAIM_DORMANCY_SECONDS` window. After
    // dormancy, individual claims still work in principle but the
    // pool's bank balance may have been swept by
    // SweepUnclaimedEmergencyShares, so a late claim would error on
    // insufficient balance.
    ClaimEmergencyShare {
        position_id: String,
    },

    // factory-only sweep of the unclaimed residual
    // after the dormancy window expires. `info.sender` must equal the
    // pool's `factory_addr`; the handler verifies
    // `env.block.time >= dormancy_expires_at` (1 year post-drain) and
    // sends the residual to `bluechip_wallet_address`. One-shot —
    // `residual_swept` flag prevents double-sweeps.
    SweepUnclaimedEmergencyShares {},

    // Factory-only callback invoked once at pool finalize. The factory
    // dispatches `Cw721ExecuteMsg::TransferOwnership { new_owner: pool }`
    // to the position-NFT during `finalize_pool`, which only sets
    // `pending_owner` on the NFT (cw_ownable is two-phase). This handler
    // is the matching second half: it sends `AcceptOwnership` back to
    // the NFT contract and flips `pool_state.nft_ownership_accepted` so
    // the deposit-side lazy fallback in pool-core becomes a no-op.
    //
    // Closes the pre-accept window that previously sat between pool
    // creation and threshold cross — until this handler ran, the
    // factory remained the NFT contract's actual owner.
    //
    // Authorisation: `info.sender` must equal `pool_info.factory_addr`.
    // Idempotent: returns Ok with no NFT message if the flag is already
    // set (e.g. the deposit-side fallback fired first in a test fixture).
    AcceptNftOwnership {},
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
    // Reports the live state of post-threshold committer payouts so admin
    // dashboards can detect a stalled distribution. Returns `None` when
    // no distribution is active (pre-threshold, or fully completed and
    // cleaned up). Returns `Some(...)` with a computed `is_stalled` flag
    // (true when the per-pool 24h DISTRIBUTION_STALL_TIMEOUT_SECONDS has
    // elapsed since the last batch advanced).
    #[returns(Option<DistributionStateResponse>)]
    DistributionState {},
}

#[cw_serde]
pub struct DistributionStateResponse {
    pub is_distributing: bool,
    pub distributions_remaining: u32,
    pub last_processed_key: Option<Addr>,
    pub started_at: Timestamp,
    pub last_updated: Timestamp,
    /// Block-time seconds since `last_updated` advanced. Computed at
    /// query time so dashboards don't have to do their own block-time
    /// math.
    pub seconds_since_update: u64,
    /// True when `seconds_since_update > DISTRIBUTION_STALL_TIMEOUT_SECONDS`.
    /// The on-chain handler (`process_distribution_batch`) will reject
    /// every keeper call with `"Distribution timeout - requires manual
    /// recovery"` while this flag is true; admin should call
    /// `RecoverPoolStuckStates::StuckDistribution` to reset the cursor.
    pub is_stalled: bool,
    pub consecutive_failures: u32,
    pub total_to_distribute: Uint128,
    pub total_committed_usd: Uint128,
    /// Running sum of creator-token rewards already minted across
    /// processed batches. Lets dashboards compute the residual dust
    /// (`total_to_distribute - distributed_so_far`) that will be
    /// settled to the creator wallet on the final batch.
    pub distributed_so_far: Uint128,
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
    /// Number of `committers` entries in THIS page after filtering by
    /// `pool_contract_address` / `min_payment_usd` / `after_timestamp`
    /// and capping at `limit`. NOT a pre-filter total — paginating
    /// callers should treat `committers.len() < limit` as the
    /// end-of-data signal rather than relying on this field.
    pub page_count: u32,
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
