//! Creator-pool (commit-phase) state.
//!
//! Shared structs, Items, and constants — every item referenced by the
//! shared code paths in `pool_core` — live in `pool_core::state`. This
//! glob re-export preserves every existing `use crate::state::X;` import
//! in the creator-pool crate (and its tests) without touching call sites.
//!
//! Commit-phase-specific Items / structs / constants follow below.
pub use pool_core::state::*;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};

// -- Commit-phase-only storage -------------------------------------------

pub const USD_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("usd_raised");
pub const COMMIT_INFO: Map<&Addr, Committing> = Map::new("sub_info");
pub const NATIVE_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("bluechip_raised");
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
pub const THRESHOLD_PROCESSING: Item<bool> = Item::new("threshold_processing");
pub const THRESHOLD_PAYOUT_AMOUNTS: Item<ThresholdPayoutAmounts> =
    Item::new("threshold_payout_amounts");
pub const DISTRIBUTION_STATE: Item<DistributionState> = Item::new("distribution_state");
pub const COMMIT_LIMIT_INFO: Item<CommitLimitInfo> = Item::new("commit_config");
pub const CREATOR_EXCESS_POSITION: Item<CreatorExcessLiquidity> = Item::new("creator_excess");
pub const LAST_THRESHOLD_ATTEMPT: Item<Timestamp> = Item::new("last_threshold_attempt");

// Set to `true` when NotifyThresholdCrossed to the factory failed via the
// reply_on_error path during a threshold-crossing commit. All pool-side
// threshold state (IS_THRESHOLD_HIT, reserves, committer distribution)
// still succeeds — only the factory's POOL_THRESHOLD_MINTED flag and the
// per-pool Bluechip mint reward are pending. Any caller can invoke
// ExecuteMsg::RetryFactoryNotify to re-send the notification; on success
// this flag is cleared by the reply handler. Absence or `false` means
// "either never crossed threshold, or factory notification already
// succeeded" — the pool's IS_THRESHOLD_HIT disambiguates.
pub const PENDING_FACTORY_NOTIFY: Item<bool> = Item::new("pending_factory_notify");

// Reply IDs for SubMsg dispatches. Kept sparse so future features can
// slot in without renumbering. The pool has only one caller of reply()
// today (factory-notify), so this is mostly forward-looking.
pub const REPLY_ID_FACTORY_NOTIFY_INITIAL: u64 = 1;
pub const REPLY_ID_FACTORY_NOTIFY_RETRY: u64 = 2;

// Note: `POOL_KIND` / `load_pool_kind` / `require_commit_pool` were
// removed in 4d. Now that standard pools run their own wasm
// (`standard-pool` crate), the kind is determined by which binary is
// executing — the runtime discriminator is gone. The factory still
// tracks `pool_kind` on `PoolDetails` for its own routing (e.g.,
// oracle sample filtering) but the pool side doesn't need it.

// -- Commit-phase-only constants -----------------------------------------

pub const DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION: u64 = 50_000;
pub const DEFAULT_MAX_GAS_PER_TX: u64 = 2_000_000;
pub const MAX_DISTRIBUTIONS_PER_TX: u32 = 40;

// Maximum wall-clock time between successful distribution batches before the
// pool declares the distribution stalled and requires admin recovery via
// `RecoverPoolStuckStates`. Sized for the worst case where the distribution
// keeper is offline: at the default 30-min poll interval the previous 2h
// window left almost no margin and risked bricking a pool on a brief
// keeper outage. 24h gives operators a full day to react.
pub const DISTRIBUTION_STALL_TIMEOUT_SECONDS: u64 = 86_400;

// Distribution keeper bounty is paid by the factory, not the pool.
// See factory::execute_pay_distribution_bounty. The pool just emits a
// WasmMsg to the factory and the factory pays from its own native reserve.
// This keeps LP funds isolated from keeper infrastructure costs.

// -- Commit-phase-only structs -------------------------------------------

#[cw_serde]
pub struct DistributionState {
    pub is_distributing: bool,
    pub total_to_distribute: Uint128,
    pub total_committed_usd: Uint128,
    pub last_processed_key: Option<Addr>,
    pub distributions_remaining: u32,
    pub estimated_gas_per_distribution: u64,
    pub max_gas_per_tx: u64,
    pub last_successful_batch_size: Option<u32>,
    pub consecutive_failures: u32,
    pub started_at: Timestamp,
    pub last_updated: Timestamp,
}

#[cw_serde]
pub enum RecoveryType {
    StuckThreshold,
    StuckDistribution,
    StuckReentrancyGuard,
    Both,
}

#[cw_serde]
pub struct Committing {
    pub pool_contract_address: Addr,
    pub committer: Addr,
    pub total_paid_usd: Uint128,
    pub total_paid_bluechip: Uint128,
    pub last_committed: Timestamp,
    pub last_payment_bluechip: Uint128,
    pub last_payment_usd: Uint128,
}

#[cw_serde]
pub struct ThresholdPayoutAmounts {
    pub creator_reward_amount: Uint128,
    pub bluechip_reward_amount: Uint128,
    pub pool_seed_amount: Uint128,
    pub commit_return_amount: Uint128,
}

#[cw_serde]
pub struct CommitLimitInfo {
    pub commit_amount_for_threshold: Uint128,
    pub commit_amount_for_threshold_usd: Uint128,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
}

#[cw_serde]
pub struct CreatorExcessLiquidity {
    pub creator: Addr,
    pub bluechip_amount: Uint128,
    pub token_amount: Uint128,
    pub unlock_time: Timestamp,
    pub excess_nft_id: Option<String>,
}
