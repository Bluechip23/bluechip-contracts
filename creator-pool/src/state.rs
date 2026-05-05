//! Creator-pool (commit-phase) state.
//!
//! Shared structs, Items, and constants — every item referenced by the
//! shared code paths in `pool_core` — live in `pool_core::state`. This
//! glob re-export preserves every existing `use crate::state::X;` import
//! in the creator-pool crate (and its tests) without touching call sites.
//!
//! Symbols re-exported from `pool_core::state` (non-exhaustive list, kept
//! here so a `grep` in this crate finds the definition site):
//!
//!   Pool registry:
//!     POOL_INFO, POOL_STATE, POOL_FEE_STATE, POOL_SPECS, POOL_ANALYTICS,
//!     LIQUIDITY_POSITIONS, OWNER_POSITIONS, NEXT_POSITION_ID, POOL_PAUSED,
//!     POOL_PAUSED_AUTO, EMERGENCY_WITHDRAWAL, PENDING_EMERGENCY_WITHDRAW,
//!     EMERGENCY_DRAINED, EXPECTED_FACTORY, REENTRANCY_LOCK, IS_THRESHOLD_HIT,
//!     CREATOR_FEE_POT, USER_LAST_COMMIT
//!
//!   Threshold-cross machinery:
//!     POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK, POST_THRESHOLD_COOLDOWN_BLOCKS,
//!     STUCK_THRESHOLD_RECOVERY_WINDOW_SECONDS,
//!     STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS,
//!     MAX_CONSECUTIVE_DISTRIBUTION_FAILURES,
//!     THRESHOLD_PAYOUT_{CREATOR,BLUECHIP,POOL,COMMIT_RETURN,TOTAL}_BASE_UNITS,
//!     SECONDS_PER_DAY, DEFAULT_LP_FEE, MAX_LP_FEE, MIN_LP_FEE,
//!     DEFAULT_SWAP_RATE_LIMIT_SECS, POOL_KIND_STANDARD, POOL_KIND_COMMIT,
//!     POOL_COMMITS_QUERY_DEFAULT_LIMIT, POOL_COMMITS_QUERY_MAX_LIMIT,
//!     MINIMUM_LIQUIDITY, EMERGENCY_WITHDRAW_DELAY_SECONDS
//!
//!   Deposit-verify reply chain:
//!     DEPOSIT_VERIFY_CTX, DEPOSIT_VERIFY_REPLY_ID
//!
//! Commit-phase-specific Items / structs / constants follow below.
pub use pool_core::state::*;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};

// -- Commit-phase-only storage -------------------------------------------

/// Running total of USD value committed to the pool pre-threshold.
pub const USD_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("usd_raised");
/// Per-committer cumulative deposit/payment record.
pub const COMMIT_INFO: Map<&Addr, Committing> = Map::new("sub_info");
/// Running total of NET-of-fees bluechip that has actually entered the
/// pool's bank balance from threshold-contributing commits. Equals the
/// contract's bank balance for `bluechip_denom` at threshold-crossing
/// time, modulo the per-commit fee floor.
///
/// `trigger_threshold_payout` reads this directly as
/// `pools_bluechip_seed` with no recovery math — every commit handler
/// stores the post-fee amount and there is no second floor to
/// reconcile. Query responses (`PoolAnalyticsResponse.total_bluechip_raised`)
/// and the `total_bluechip_raised_after` event attribute also expose
/// this net value, so frontends displaying "X bluechip raised toward
/// goal" show the post-fee amount.
///
/// Storage key is `"bluechip_raised"` for cross-version compatibility.
pub const NATIVE_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("bluechip_raised");
/// Per-committer USD ledger; drained during post-threshold distribution.
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
/// Re-entrancy/inflight flag set while a threshold-crossing commit is mid-execution.
pub const THRESHOLD_PROCESSING: Item<bool> = Item::new("threshold_processing");
/// Fixed split of creator-token amounts paid out at threshold crossing.
pub const THRESHOLD_PAYOUT_AMOUNTS: Item<ThresholdPayoutAmounts> =
    Item::new("threshold_payout_amounts");
/// Cursor + accounting for the post-threshold distribution batch loop.
pub const DISTRIBUTION_STATE: Item<DistributionState> = Item::new("distribution_state");
/// Threshold target, max bluechip lock, and excess-lock duration.
pub const COMMIT_LIMIT_INFO: Item<CommitLimitInfo> = Item::new("commit_config");
/// Creator-side excess liquidity position created when raised bluechip exceeds the per-pool cap.
pub const CREATOR_EXCESS_POSITION: Item<CreatorExcessLiquidity> = Item::new("creator_excess");
/// Timestamp of the most recent threshold-crossing attempt; used by stuck-state recovery.
pub const LAST_THRESHOLD_ATTEMPT: Item<Timestamp> = Item::new("last_threshold_attempt");

/// Set to `true` when `NotifyThresholdCrossed` to the factory failed
/// via the `reply_on_error` path during a threshold-crossing commit.
///
/// All pool-side threshold state (`IS_THRESHOLD_HIT`, reserves,
/// committer distribution) still succeeds — only the factory's
/// `POOL_THRESHOLD_MINTED` flag and the per-pool Bluechip mint reward
/// are pending. Any caller can invoke
/// [`crate::msg::ExecuteMsg::RetryFactoryNotify`] to re-send the
/// notification; on success this flag is cleared by the reply
/// handler. Absence or `false` means "either never crossed threshold,
/// or factory notification already succeeded" — the pool's
/// `IS_THRESHOLD_HIT` disambiguates.
pub const PENDING_FACTORY_NOTIFY: Item<bool> = Item::new("pending_factory_notify");

/// Reply IDs for SubMsg dispatches. Kept sparse so future features can
/// slot in without renumbering. The pool has only one caller of `reply()`
/// today (factory-notify), so this is mostly forward-looking.
pub const REPLY_ID_FACTORY_NOTIFY_INITIAL: u64 = 1;
pub const REPLY_ID_FACTORY_NOTIFY_RETRY: u64 = 2;

// `POOL_KIND` / `load_pool_kind` / `require_commit_pool` were removed
// in 4d. Now that standard pools run their own wasm (`standard-pool`
// crate), the kind is determined by which binary is executing — the
// runtime discriminator is gone. The factory still tracks `pool_kind`
// on `PoolDetails` for its own routing (e.g. oracle sample filtering)
// but the pool side doesn't need it.

// -- Commit-phase-only constants -----------------------------------------

/// Default per-distribution gas estimate used to size batch dispatch.
pub const DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION: u64 = 50_000;
/// Default per-tx gas budget the batch sizer divides by `estimated_gas_per_distribution`.
pub const DEFAULT_MAX_GAS_PER_TX: u64 = 2_000_000;
/// Hard cap on distributions processed per `ContinueDistribution` call.
pub const MAX_DISTRIBUTIONS_PER_TX: u32 = 40;

/// Maximum wall-clock time between successful distribution batches before
/// the pool declares the distribution stalled and requires admin recovery
/// via `RecoverPoolStuckStates`. Sized for the worst case where the
/// distribution keeper is offline: at the default 30-min poll interval the
/// previous 2h window left almost no margin and risked bricking a pool on
/// a brief keeper outage. 24h gives operators a full day to react.
pub const DISTRIBUTION_STALL_TIMEOUT_SECONDS: u64 = 86_400;

/// Per-keeper rate limit on `ContinueDistribution`. Prevents a single
/// keeper (or a competing keeper losing the race) from same-block
/// spamming no-op tx that pay no bounty but still cost ledger reads
/// and gas. Each `info.sender` must wait
/// [`CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS`] between calls; another
/// address can still call sooner, so legitimate keepers rotating
/// through addresses (e.g. a multi-keeper service) aren't blocked.
///
/// Per-address tracker keyed on the caller. The map is append-only
/// with respect to addresses: every distinct keeper that has ever
/// called gets one entry, and entries are never pruned. Long-lived
/// popular pools accumulate one entry per distinct keeper address
/// forever. Storage growth is bounded by the size of the keeper
/// population (a few addresses for typical deployments) and is not
/// on any hot read path; opportunistic pruning can be added later
/// if a deployment ever shows real growth, but it's deliberately not
/// part of the current contract's scope.
pub const LAST_CONTINUE_DISTRIBUTION_AT: cw_storage_plus::Map<&Addr, u64> =
    cw_storage_plus::Map::new("last_continue_distribution_at");

/// 5 seconds between consecutive ContinueDistribution calls from the
/// same address. Tight enough to deflate same-block spam but loose
/// enough that a legitimate keeper polling at any reasonable cadence
/// (every block, every 30s, every 5min) is unaffected on the slow path.
pub const CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS: u64 = 5;

// Distribution keeper bounty is paid by the factory, not the pool —
// see `factory::execute_pay_distribution_bounty`. The pool just emits
// a `WasmMsg` to the factory and the factory pays from its own native
// reserve. This keeps LP funds isolated from keeper infrastructure
// costs.

// -- Commit-phase-only structs -------------------------------------------

#[cw_serde]
pub struct DistributionState {
    /// True while a distribution is in-flight; false after completion or recovery shutdown.
    pub is_distributing: bool,
    /// Total creator-token amount to be distributed across all committers.
    pub total_to_distribute: Uint128,
    /// Snapshot of total committed USD at threshold-cross; denominator for share math.
    pub total_committed_usd: Uint128,
    /// Cursor into COMMIT_LEDGER; next batch starts strictly after this key.
    pub last_processed_key: Option<Addr>,
    /// Informational counter of remaining committers (ground truth is the ledger).
    pub distributions_remaining: u32,
    /// Adaptive estimate of gas consumed per distribution entry.
    pub estimated_gas_per_distribution: u64,
    /// Per-tx gas budget used to derive batch size.
    pub max_gas_per_tx: u64,
    /// Size of the last batch that completed successfully (for adaptive sizing).
    pub last_successful_batch_size: Option<u32>,
    /// Count of consecutive failed batches; triggers stall after threshold.
    pub consecutive_failures: u32,
    /// Block time when distribution started.
    pub started_at: Timestamp,
    /// Block time of the most recent successful batch (used by stall detection).
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
    /// Pool this commit history is associated with.
    pub pool_contract_address: Addr,
    /// Address that owns this committing record.
    pub committer: Addr,
    /// Cumulative USD value committed by this address.
    pub total_paid_usd: Uint128,
    /// Cumulative native bluechip committed by this address.
    pub total_paid_bluechip: Uint128,
    /// Block time of the most recent commit by this address.
    pub last_committed: Timestamp,
    /// Native bluechip on the most recent commit (for rate-limiting/UX).
    pub last_payment_bluechip: Uint128,
    /// USD value on the most recent commit.
    pub last_payment_usd: Uint128,
}

#[cw_serde]
pub struct ThresholdPayoutAmounts {
    /// Creator-token amount minted to the creator wallet at threshold-cross.
    pub creator_reward_amount: Uint128,
    /// Creator-token amount minted to the Bluechip wallet at threshold-cross.
    pub bluechip_reward_amount: Uint128,
    /// Creator-token amount minted to seed the AMM reserves.
    pub pool_seed_amount: Uint128,
    /// Creator-token amount minted to fund the post-threshold committer distribution.
    pub commit_return_amount: Uint128,
}

#[cw_serde]
pub struct CommitLimitInfo {
    /// USD threshold target; once total committed USD reaches this, the pool seeds.
    pub commit_amount_for_threshold_usd: Uint128,
    /// Max native bluechip locked into pool reserves; remainder becomes creator excess.
    pub max_bluechip_lock_per_pool: Uint128,
    /// Lock duration (days) on the creator-excess liquidity position.
    pub creator_excess_liquidity_lock_days: u64,
}

#[cw_serde]
pub struct CreatorExcessLiquidity {
    /// Creator wallet entitled to claim the excess once unlocked.
    pub creator: Addr,
    /// Bluechip portion of the excess (above max_bluechip_lock_per_pool).
    pub bluechip_amount: Uint128,
    /// Creator-token portion of the excess proportional to bluechip_amount.
    pub token_amount: Uint128,
    /// Earliest block time at which the creator may claim this excess.
    pub unlock_time: Timestamp,
    /// Position-NFT id minted on claim (None until claimed).
    pub excess_nft_id: Option<String>,
}
