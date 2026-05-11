// ---------------------------------------------------------------------------
// Storage-key CHANGELOG
//
// Most `Item` / `Map` constants in this file use a key string that matches
// the Rust identifier (e.g. `POOLS_BY_ID -> "pools_by_id"`). The drifts
// below are kept on purpose for migration compatibility — renaming the
// key would orphan existing chain state. Add a row here when introducing
// a new drift, or removing one (which is a breaking migration in itself).
//
//   Const                          Storage key                     Reason
//   ------------------------------ ------------------------------- -------------------------------------------------
//   FIRST_THRESHOLD_TIMESTAMP      "first_pool_timestamp"          Renamed from FIRST_POOL_TIMESTAMP; key preserved.
//   POOL_CREATION_CONTEXT          "pool_creation_ctx_v3"          v3 schema; v1/v2 predate the unified Temp+State context.
//   COMMIT_POOL_COUNTER            "commit_pool_counter"           Matches.
//   POOLS_BY_CONTRACT_ADDRESS      "pools_by_contract_address"     Matches.
//   STANDARD_POOL_CREATION_CONTEXT "std_pool_ctx"                  Shorter key chosen to keep prefix bytes small.
//   LAST_STANDARD_POOL_CREATE_AT   "last_std_pool_create_at"       Shorter key (per above).
//
// Unlisted Items/Maps follow the convention "key == lowercase(IDENT)";
// any future addition that diverges should be appended here.
// ---------------------------------------------------------------------------

use crate::asset::TokenType;
use crate::pool_struct::{PoolDetails, TempPoolCreation, ThresholdPayoutAmounts};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, StdResult, Storage, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::PoolStateResponseForFactory;

pub const FACTORYINSTANTIATEINFO: Item<FactoryInstantiate> = Item::new("config");
// Single source of truth for every in-flight pool creation. Combines the
// formerly-split TEMP_POOL_CREATION (pool inputs + discovered addresses)
// and lifecycle status into one map so the reply handlers can't leave the
// two halves out of sync. On any failure the whole tx reverts (every step
// uses `SubMsg::reply_on_success`), so no retry/cleanup bookkeeping is
// needed: a failed create leaves no trace in this map.
pub const POOL_CREATION_CONTEXT: Map<u64, PoolCreationContext> =
    Map::new("pool_creation_ctx_v3");
pub const PENDING_CONFIG: Item<PendingConfig> = Item::new("pending_config");
pub const POOL_COUNTER: Item<u64> = Item::new("pool_counter");

/// Commit-pool-only ordinal. Bumped exactly once per `execute_create_creator_pool`
/// and stored on the commit pool's `PoolDetails.commit_pool_ordinal` so the
/// threshold-mint decay formula can use it as `x` instead of `pool_id`.
///
/// This split exists because `POOL_COUNTER` is bumped by both commit and
/// standard pool creations; using `pool_id` directly in the decay formula
/// would let permissionless `CreateStandardPool` calls inflate `x` and
/// shrink (toward zero) the bluechip mint reward for legitimate commit
/// pools created later. The dedicated counter keeps the decay schedule
/// anchored to actual commit-pool creation activity.
pub const COMMIT_POOL_COUNTER: Item<u64> = Item::new("commit_pool_counter");

// Three coupled pool-registry maps. They MUST stay in sync — every pool
// that exists must appear in all three. Always go through `register_pool`
// rather than touching them individually.
//   - POOLS_BY_ID:               pool_id  -> PoolDetails (token info, addresses)
//   - POOLS_BY_CONTRACT_ADDRESS: pool addr -> snapshot used by oracle / queries
//   - PAIRS:                     canonical (asset_a, asset_b) key -> pool_id.
//     Single-pool-per-pair guard. The Uniswap-style invariant: at most one
//     pool exists per (asset_a, asset_b) tuple. Without it, any sender can
//     register an arbitrary number of identical pairs (each from a different
//     `info.sender` to bypass the per-address rate limit), bloating the
//     registry, fragmenting LP, and — most concretely — letting attackers
//     spawn thin "anchor candidate" duplicates that the oracle's snapshot
//     refresh may sample. See `canonical_pair_key` for the encoding.
pub const POOLS_BY_ID: Map<u64, PoolDetails> = Map::new("pools_by_id");
pub const POOLS_BY_CONTRACT_ADDRESS: Map<Addr, PoolStateResponseForFactory> =
    Map::new("pools_by_contract_address");
pub const PAIRS: Map<(String, String), u64> = Map::new("pairs");

/// Reverse index: pool contract address -> `pool_id`. Maintained alongside
/// `POOLS_BY_ID` by `register_pool` so any caller that has a pool address
/// and needs the full `PoolDetails` can do two O(1) loads
/// (`POOL_ID_BY_ADDRESS.load(addr) -> POOLS_BY_ID.load(id)`) instead of
/// the previous O(N) linear scan of `POOLS_BY_ID` inside
/// `lookup_pool_by_addr`. `POOLS_BY_CONTRACT_ADDRESS` exists but stores
/// `PoolStateResponseForFactory` (a snapshot for oracle/queries), not the
/// kind/ordinal-bearing `PoolDetails`, so it can't short-circuit the
/// lookup on its own.
///
/// MUST stay in sync with `POOLS_BY_ID`. `register_pool` writes both
/// atomically. Direct writes outside `register_pool` risk drift.
pub const POOL_ID_BY_ADDRESS: Map<Addr, u64> = Map::new("pool_id_by_address");
// Maximum age (seconds) of a Pyth price we are willing to use for USD
// conversions. 300 seconds (5 minutes) gives Pyth headroom across
// publisher hiccups and short network outages without making the
// staleness window so wide that a stale-but-acceptable price becomes
// useful for "pick a favorable point in time" manipulation. The same
// threshold is enforced on the live Pyth read, on the cache-fallback
// re-read inside `get_bluechip_usd_price_with_meta`, and on the
// best-effort warm-up path.
pub const MAX_PRICE_AGE_SECONDS_BEFORE_STALE: u64 = 300;

/// Confidence-interval gate on Pyth ATOM/USD reads, expressed as basis
/// points of price. Admin-tunable via `SetPythConfThresholdBps`,
/// bounded to `[PYTH_CONF_THRESHOLD_BPS_MIN, PYTH_CONF_THRESHOLD_BPS_MAX]`
/// so neither a missing storage slot nor an admin mistake can disable
/// the gate or set it impossibly tight.
///
/// The same value is enforced on (a) the live Pyth read inside
/// `query_pyth_atom_usd_price_with_conf` and (b) the cache-fallback
/// re-read inside `get_bluechip_usd_price_with_meta` — so tightening
/// the bps immediately rejects any stale-cached price whose
/// sampling-time conf no longer satisfies the new gate.
pub const PYTH_CONF_THRESHOLD_BPS: Item<u16> = Item::new("pyth_conf_threshold_bps");

/// Default conf gate (bps). Tightened from the previous hardcoded
/// 500 bps (5%) audit fix. 200 bps = 2% is well inside what a healthy
/// Pyth ATOM/USD feed reports during steady state.
pub const PYTH_CONF_THRESHOLD_BPS_DEFAULT: u16 = 200;

/// Strict floor — rejecting < 50 bps would make the protocol
/// effectively unable to use the oracle through any market wobble.
pub const PYTH_CONF_THRESHOLD_BPS_MIN: u16 = 50;

/// Hard ceiling — never let an admin loosen past the original 5%
/// gate, even via direct storage write.
pub const PYTH_CONF_THRESHOLD_BPS_MAX: u16 = 500;

/// Read the configured conf gate, falling back to
/// `PYTH_CONF_THRESHOLD_BPS_DEFAULT` when the slot is unset (fresh
/// deployments, pre-upgrade chains migrating in).
pub fn load_pyth_conf_threshold_bps(storage: &dyn cosmwasm_std::Storage) -> u16 {
    PYTH_CONF_THRESHOLD_BPS
        .may_load(storage)
        .ok()
        .flatten()
        .unwrap_or(PYTH_CONF_THRESHOLD_BPS_DEFAULT)
}

/// Validates a cached `(price, conf)` pair against the currently
/// configured bps gate.
///
/// Fail-closed semantics — when `cached_conf == 0` we treat the
/// sample as "conf unknown" (almost certainly a record persisted
/// before this field existed) and refuse to serve. Real Pyth
/// publishes essentially never produce conf == 0; treating zero as
/// the "unknown" sentinel lets pre-upgrade caches fall through to
/// the no-cache error path rather than silently passing the gate.
pub fn ensure_cached_pyth_conf_acceptable(
    storage: &dyn cosmwasm_std::Storage,
    cached_price: cosmwasm_std::Uint128,
    cached_conf: u64,
) -> cosmwasm_std::StdResult<()> {
    if cached_conf == 0 {
        return Err(cosmwasm_std::StdError::generic_err(
            "Cached Pyth price has no recorded confidence interval (pre-upgrade record); \
             refusing to serve from cache. The next successful UpdateOraclePrice will \
             persist a conf-validated cache and unblock the fallback path.",
        ));
    }
    let bps = load_pyth_conf_threshold_bps(storage);
    let price_u64: u64 = cached_price.u128().min(u64::MAX as u128) as u64;
    let threshold = price_u64
        .saturating_mul(bps as u64)
        .saturating_div(10_000);
    if cached_conf > threshold {
        return Err(cosmwasm_std::StdError::generic_err(format!(
            "Cached Pyth confidence interval too wide for current gate: \
             cached_conf={} exceeds {} bps of cached_price={}",
            cached_conf, bps, cached_price
        )));
    }
    Ok(())
}

// Standard timelock applied to admin-initiated mutations of factory state
// (config, pool config, pool upgrades, force-rotate). 48h gives the
// community a full two days to observe a pending change and respond.
// Single source of truth — every propose/execute pair below MUST use this
// constant rather than spelling out `86400 * 2`.
pub const ADMIN_TIMELOCK_SECONDS: u64 = 86_400 * 2;
pub const PENDING_POOL_UPGRADE: Item<PoolUpgrade> = Item::new("pending_upgrade");
// Timestamp of the *first pool that crossed its commit threshold*.
// Despite the old name `FIRST_POOL_TIMESTAMP`, this is NOT set on first
// pool creation — it's lazy-set inside `calculate_and_mint_bluechip`
// the first time any pool crosses its threshold. The mint-decay formula
// uses `block.time - first_threshold_time` as its `s` input, so the decay
// is anchored to the first threshold event, not to when the factory was
// deployed. Storage key is preserved for migration compatibility.
pub const FIRST_THRESHOLD_TIMESTAMP: Item<Timestamp> = Item::new("first_pool_timestamp");
pub const POOL_THRESHOLD_MINTED: Map<u64, bool> = Map::new("pool_threshold_minted");
pub const PENDING_POOL_CONFIG: Map<u64, PendingPoolConfig> = Map::new("pending_pool_config");

// Per-address rate limit on commit-pool creation: timestamp of each
// creator's last successful `Create`. Defends against spam that would
// inflate the commit-pool ordinal and gas-amplify any future per-pool
// storage scan. Per-address (not global) so coordinated multi-address
// spam still has to fund + sign from each address it rotates through.
pub const LAST_COMMIT_POOL_CREATE_AT: Map<Addr, Timestamp> =
    Map::new("last_commit_pool_create_at");

/// Time-ordered secondary index over `LAST_COMMIT_POOL_CREATE_AT`,
/// keyed by `(timestamp_secs, Addr)`. Exists so the permissionless
/// `PruneRateLimits` handler can iterate stale entries in O(stale_count)
/// instead of walking the full address-keyed map (which is alphabetic
/// in `Addr` and therefore uncorrelated with timestamp — a prune call
/// against a million-entry map would otherwise visit every entry
/// looking for the first stale one).
///
/// Maintained alongside `LAST_COMMIT_POOL_CREATE_AT` by the create
/// handler: on each stamp it removes the prior `(old_ts, addr)`
/// entry (if any) and inserts the new `(now_ts, addr)`. Prune deletes
/// from BOTH on each stale entry it processes. Both updates ride in
/// the same tx as the primary save, so a failure reverts both maps
/// atomically and they cannot drift.
pub const COMMIT_POOL_CREATE_TS_INDEX: Map<(u64, Addr), ()> =
    Map::new("commit_pool_create_ts_idx");

/// Minimum seconds between consecutive `Create` calls from the same
/// `info.sender`. 3600s = 1h. Reasonable for legitimate creator-pool
/// flows (you launch one token at a time) and asymmetric enough against
/// spam that even a fully-funded attacker would need to rotate through
/// thousands of addresses to materially inflate `commit_pool_ordinal`.
pub const COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS: u64 = 3600;

// Per-address rate limit on standard-pool creation. Mirror of the
// commit-pool rate limit; defends against the same registry-bloat
// shape. The standard-pool USD creation fee is the primary economic
// barrier, but the fee path falls back to a hardcoded
// `STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP` (100 bluechip)
// whenever the oracle is unavailable (warm-up window, Pyth+cache
// outage, etc.). Without a per-address cooldown, an attacker who
// engineers an oracle-unavailable window — or simply happens to act
// during one — can spam pools at the fallback rate. Same 1h cooldown
// as commit pools so the two flows symmetric.
pub const LAST_STANDARD_POOL_CREATE_AT: Map<Addr, Timestamp> =
    Map::new("last_std_pool_create_at");

/// Time-ordered secondary index for `LAST_STANDARD_POOL_CREATE_AT`.
/// Mirror of `COMMIT_POOL_CREATE_TS_INDEX`; same invariant — every
/// write to the primary updates this index in the same tx; prune walks
/// in ascending timestamp order with an early-exit on the first
/// non-stale entry.
pub const STANDARD_POOL_CREATE_TS_INDEX: Map<(u64, Addr), ()> =
    Map::new("std_pool_create_ts_idx");

pub const STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS: u64 = 3600;

// Keeper bounty paid to whoever successfully calls UpdateOraclePrice.
// Stored as a USD value (6 decimals: 1_000_000 = $1.00). At payout time
// the factory converts USD to bluechip via the internal oracle so the
// bounty stays approximately constant in USD as bluechip price moves.
// The existing UPDATE_INTERVAL cooldown gates frequency, so the payout
// can fire at most once per window and cannot be spammed.
// Admin tunable up to MAX_ORACLE_UPDATE_BOUNTY_USD via
// SetOracleUpdateBounty. Zero disables the bounty entirely.
pub const ORACLE_UPDATE_BOUNTY_USD: Item<Uint128> = Item::new("oracle_update_bounty_usd");

// Hard cap to protect the factory's reserve if the admin key is
// compromised. $0.10 USD per successful update (6 decimals). Realistic
// keeper gas is ~$0.003–$0.03 per oracle update on typical Cosmos
// chains; $0.10 leaves headroom for gas spikes while capping the yearly
// drain at ~$28.80/day ≈ $10.5k/year if admin is compromised.
pub const MAX_ORACLE_UPDATE_BOUNTY_USD: Uint128 = Uint128::new(100_000);

// (`ORACLE_BOUNTY_DENOM` was removed — the bounty path now reads the
// canonical bluechip denom from `FACTORYINSTANTIATEINFO.bluechip_denom`
// directly. A separate const created two divergeable sources of truth
// for the same value and was non-functional on chains where bluechip
// is a tokenfactory denom (e.g. `factory/<addr>/ubluechip`).)

// Keeper bounty paid per successful pool.ContinueDistribution batch.
// USD-denominated (6 decimals). Same conversion-at-payout pattern as
// the oracle bounty, so keeper economics stay stable as bluechip price
// moves. Pool LP reserves are never tapped — the factory pays from its
// own pre-funded native balance.
pub const DISTRIBUTION_BOUNTY_USD: Item<Uint128> = Item::new("distribution_bounty_usd");

// Hard cap. $0.10 USD per batch (6 decimals). A distribution batch is
// up to MAX_DISTRIBUTIONS_PER_TX=40 mints + a handful of storage writes;
// realistic gas ~$0.01–$0.10. Caps admin-compromise blast radius at
// $0.10 × committer_count/40 per pool's full distribution.
pub const MAX_DISTRIBUTION_BOUNTY_USD: Uint128 = Uint128::new(100_000);

// ForceRotateOraclePools is a 2-step action: admin proposes a rotation,
// the timelock elapses, then admin invokes ForceRotateOraclePools to
// execute. Prevents a compromised admin from instantly rotating the
// oracle's pool sample set without a 48h observability window for the
// community to notice and respond.
pub const PENDING_ORACLE_ROTATION: Item<Timestamp> = Item::new("pending_oracle_rotation");

/// Bootstrap price candidate (HIGH-4 audit fix). At true bootstrap
/// (`prior == 0 && pre_reset == 0 && pending_first_price == None`), the
/// oracle's first published TWAP previously landed in `last_price`
/// directly with no circuit-breaker protection — a single-block
/// manipulation of the freshly-seeded anchor reserves could anchor the
/// breaker to an attacker-chosen value.
///
/// New flow: branch (d) of `update_internal_oracle_price` writes the
/// candidate here instead of publishing to `last_price`. Each
/// subsequent successful TWAP round in branch (d) overwrites the
/// price (with `proposed_at` preserved from the first proposal so the
/// observation window is enforced from the FIRST observation forward).
/// Once the admin is satisfied — typically after watching the
/// candidate stabilize for ≥ `BOOTSTRAP_OBSERVATION_SECONDS` —
/// `ConfirmBootstrapPrice` publishes the latest candidate as
/// `last_price`. `CancelBootstrapPrice` discards the pending and
/// forces re-bootstrap on the next update.
///
/// Reachable only on first deployment OR when
/// `INITIAL_ANCHOR_SET == false`. Once a price is published, the
/// breaker takes over via branches (a)/(b)/(c) on subsequent updates
/// and this item stays empty.
#[cw_serde]
pub struct PendingBootstrapPrice {
    pub price: Uint128,
    /// ATOM-pool individual TWAP captured on the same round that
    /// produced `price`. Carried through so `ConfirmBootstrapPrice`
    /// can push a `PriceObservation` whose `atom_pool_price` field
    /// matches the observation it's anchoring (rather than
    /// duplicating `price` and misleading any downstream observability
    /// query that reads the field).
    pub atom_pool_price: Uint128,
    pub proposed_at: Timestamp,
    pub observation_count: u32,
}

pub const PENDING_BOOTSTRAP_PRICE: Item<PendingBootstrapPrice> =
    Item::new("pending_bootstrap_price");

/// Minimum observation window between the first bootstrap-candidate
/// proposal and the earliest moment the admin may call
/// `ConfirmBootstrapPrice`. 1h forces the admin to watch the
/// candidate move across at least 12 successful update rounds
/// (UPDATE_INTERVAL = 300s) before locking it in, which makes a
/// sustained-manipulation attack noticeably more expensive than a
/// single-block perturbation.
pub const BOOTSTRAP_OBSERVATION_SECONDS: u64 = 3600;

// One-shot bootstrap flag for the anchor pool. False until the admin
// invokes `ExecuteMsg::SetAnchorPool { pool_id }` exactly once; flipped
// to true at that point. After flip, any subsequent change to
// `atom_bluechip_anchor_pool_address` must go through the standard 48h
// `ProposeConfigUpdate` → `UpdateConfig` flow. The one-shot exists
// purely to dodge the launch-day chicken-and-egg: the admin needs to
// (a) deploy the factory, (b) create the ATOM/bluechip standard pool
// via CreateStandardPool, then (c) point the factory at it as the
// anchor — and (c) needs to be immediate, not 48h after deploy. After
// the one-shot fires, normal change-control resumes.
pub const INITIAL_ANCHOR_SET: Item<bool> = Item::new("initial_anchor_set");

// Hardcoded fallback fee in ubluechip when the USD-to-bluechip oracle
// conversion fails (Pyth stale, oracle uninitialized, anchor pool not
// liquid yet, etc.). Load-bearing during launch, since the very first
// CreateStandardPool call — the one that creates the ATOM/bluechip
// anchor pool — necessarily happens before the oracle has any data to
// price USD against. 100 bluechip is a reasonable safety floor; the
// admin tunes the USD-denominated fee separately via
// `FactoryInstantiate.standard_pool_creation_fee_usd`.
pub const STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP: Uint128 = Uint128::new(100_000_000);

#[cw_serde]
pub struct PendingPoolConfig {
    pub pool_id: u64,
    pub update: crate::pool_struct::PoolConfigUpdate,
    pub effective_after: Timestamp,
}

#[cw_serde]
pub struct FactoryInstantiate {
    pub factory_admin_address: Addr,
    pub commit_threshold_limit_usd: Uint128,
    pub pyth_contract_addr_for_conversions: String,
    pub pyth_atom_usd_price_feed_id: String,
    pub cw20_token_contract_id: u64,
    pub cw721_nft_contract_id: u64,
    pub create_pool_wasm_contract_id: u64,
    /// Code ID for the standard-pool wasm. Defaults to `0` on old
    /// serialized records so pre-split factories continue to
    /// deserialize; operators must propose a config update that sets
    /// this before `CreateStandardPool` can succeed. Standard pools
    /// instantiate against THIS code_id, not `create_pool_wasm_contract_id`.
    #[serde(default)]
    pub standard_pool_wasm_contract_id: u64,
    pub bluechip_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
    pub atom_bluechip_anchor_pool_address: Addr,
    pub bluechip_mint_contract_address: Option<Addr>,
    /// Canonical native bank denom for the bluechip token on this chain
    /// (e.g. "ubluechip"). Pinned at factory instantiate time and enforced
    /// whenever a pool is created: the `TokenType::Native { denom }` entry
    /// in `pool_token_info` MUST match this value exactly. Prevents an
    /// attacker from registering a pool with an arbitrary native denom
    /// (tokenfactory-minted fake bluechip, low-value IBC denom, etc.) and
    /// having every downstream oracle/commit path treat that denom's
    /// balance as real bluechip.
    pub bluechip_denom: String,
    /// Bank denom for the asset paired against bluechip in the
    /// ATOM/bluechip anchor pool. On Cosmos Hub this is `"uatom"`
    /// directly; on other chains it's the IBC-wrapped denom hash
    /// (e.g. `"ibc/27394FB..."`). Pinned at factory instantiate time.
    /// `execute_set_anchor_pool` enforces that the anchor pool's
    /// non-bluechip side matches this value exactly, preventing the
    /// admin (or a compromised admin key) from pointing the anchor at
    /// a bluechip/<arbitrary> standard pool whose price has no relation
    /// to the configured Pyth ATOM/USD feed.
    ///
    /// Non-empty at instantiate; tunable via the standard 48h
    /// `ProposeConfigUpdate` flow (e.g. if the chain swaps the
    /// IBC channel underlying the atom denom).
    ///
    /// `#[serde(default)]` keeps old serialized factory records
    /// (instantiated pre-this-field) deserializing — they round-trip
    /// with an empty string, and `execute_set_anchor_pool` rejects
    /// with a clear "atom_denom not configured" error pointing at
    /// `ProposeConfigUpdate` until the operator backfills it.
    #[serde(default)]
    pub atom_denom: String,
    /// USD-denominated fee (6 decimals: 1_000_000 = $1.00) charged on
    /// every `CreateStandardPool` call. Paid in ubluechip — the handler
    /// converts USD → bluechip via the internal oracle at call time. If
    /// the oracle is unavailable (bootstrap, Pyth outage, no anchor
    /// liquidity yet), the handler falls back to the hardcoded
    /// `STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP` constant so that
    /// the very first standard pool — usually the anchor pool itself —
    /// can still be created before the oracle has any data.
    ///
    /// Tunable via the existing 48h `ProposeConfigUpdate` flow.
    /// Setting this to zero disables the fee entirely (legitimate
    /// configuration choice for permissioned deployments).
    pub standard_pool_creation_fee_usd: Uint128,
    /// Per-pool threshold-payout splits applied when a commit pool
    /// crosses its USD threshold. The sum is also used as the CW20
    /// mint cap pinned at create time, so changing these values
    /// after launch only affects pools created AFTER the timelock
    /// expires — already-instantiated pools have their cap baked in.
    ///
    /// `#[serde(default)]` lets pre-this-field factory records
    /// deserialize cleanly with the launch defaults
    /// (creator 325e9 / bluechip 25e9 / pool_seed 350e9 / commit_return 500e9
    /// = 1.2e12 total). New deployments must still pass an explicit value
    /// at instantiate time; the default exists purely for migration
    /// compatibility with old serialized config snapshots.
    #[serde(default)]
    pub threshold_payout_amounts: ThresholdPayoutAmounts,
    /// Timelock between `EmergencyWithdraw` Phase 1 (initiate) and Phase 2
    /// (drain) on every pool spawned by this factory. Queried at runtime by
    /// `pool-core::execute_emergency_withdraw_initiate` via the
    /// `FactoryQueryMsg::EmergencyWithdrawDelaySeconds` cross-contract query,
    /// so pools always read the current factory-side value rather than a
    /// snapshot taken at instantiate time.
    ///
    /// Default `86_400` (24h). Range-validated in `validate_factory_config`:
    /// minimum `EMERGENCY_WITHDRAW_DELAY_MIN_SECONDS` (60s), maximum
    /// `EMERGENCY_WITHDRAW_DELAY_MAX_SECONDS` (7 days).
    ///
    /// Tunable via the standard 48h `ProposeConfigUpdate` flow. Changing
    /// this affects in-flight emergency-withdraws? No — the
    /// `effective_after` timestamp is computed at initiate time from the
    /// then-current value and stored in `PENDING_EMERGENCY_WITHDRAW`; a
    /// later config update changes only the cadence applied to NEXT
    /// initiations.
    ///
    /// `#[serde(default)]` lets old serialized factory records (no field)
    /// deserialize cleanly with the legacy default, so existing
    /// deployments behave identically until the admin proposes an update.
    #[serde(default = "default_emergency_withdraw_delay_seconds")]
    pub emergency_withdraw_delay_seconds: u64,
}

pub const EMERGENCY_WITHDRAW_DELAY_MIN_SECONDS: u64 = 60;
pub const EMERGENCY_WITHDRAW_DELAY_MAX_SECONDS: u64 = 86_400 * 7;

pub fn default_emergency_withdraw_delay_seconds() -> u64 {
    86_400
}

#[cw_serde]
pub struct PendingConfig {
    pub new_config: FactoryInstantiate,
    pub effective_after: Timestamp,
}

#[cw_serde]
pub struct PoolCreationState {
    pub pool_id: u64,
    pub creator: Addr,
    pub creation_time: Timestamp,
    pub status: CreationStatus,
}

/// Unified view of an in-flight pool creation. `temp` carries the original
/// create msg plus the CW20/CW721 addresses discovered during the reply
/// chain; `state` carries lifecycle/status for failure-recovery and queries.
#[cw_serde]
pub struct PoolCreationContext {
    pub temp: TempPoolCreation,
    pub state: PoolCreationState,
    /// Captured at commit-pool create time and threaded through the reply
    /// chain into `PoolDetails.commit_pool_ordinal`. Stored on the context
    /// rather than re-computed in `finalize_pool` so the ordinal is fixed
    /// at create time even if a concurrent commit-pool create races —
    /// commit pools share the global `COMMIT_POOL_COUNTER` allocator but
    /// each pool's ordinal is locked in here on its own create tx.
    #[serde(default)]
    pub commit_pool_ordinal: u64,
}

/// Per-`CreateStandardPool` in-flight context. Mirrors the role of
/// `PoolCreationContext` for the much shorter standard-pool reply chain.
/// Standard pools don't mint a fresh CW20, so the chain is just
/// CW721-instantiate → pool-instantiate (no SET_TOKENS step), and the
/// state is correspondingly leaner. Removed by `finalize_standard_pool`
/// once registration completes; on failure the entire tx reverts and
/// nothing persists (same atomicity guarantees as commit pools — see
/// pool_create_cleanup.rs comment block).
#[cw_serde]
pub struct StandardPoolCreationContext {
    pub pool_id: u64,
    pub pool_token_info: [crate::asset::TokenType; 2],
    pub creator: Addr,
    /// Caller-supplied label propagated to the pool wasm's instantiate
    /// label field (visible to block explorers and operator tooling).
    pub label: String,
    /// Set after the CW721 NFT instantiate sub-message returns; consumed
    /// by `finalize_standard_pool` to wire ownership to the new pool.
    pub nft_addr: Option<Addr>,
}

pub const STANDARD_POOL_CREATION_CONTEXT: Map<u64, StandardPoolCreationContext> =
    Map::new("std_pool_ctx");

/// Cached list of pool contract addresses eligible for oracle TWAP sampling.
/// Rebuilt by a full O(N) scan of `POOLS_BY_ID` at most once per
/// `ELIGIBLE_POOL_REFRESH_BLOCKS` blocks (≈5 days at 6s blocks); between
/// refreshes the oracle samples directly from the snapshot without touching
/// POOLS_BY_ID. The cross-contract liquidity / paused check still runs
/// per-sample at oracle-update time, so freshly-drained pools are dropped
/// from the sample set immediately; they stay in the snapshot's `pool_addresses`
/// until the next 5-day refresh but have no observable effect on the TWAP.
///
/// A newly-threshold-crossed pool is NOT visible to the oracle until the
/// next refresh (up to 5 days). This is an intentional tradeoff: an explicit
/// admin force-refresh was considered and rejected.
///
/// `pool_addresses` and `bluechip_indices` are coupled — entry `i` of the
/// addresses array has its bluechip side at reserve-index `bluechip_indices[i]`.
/// Hoisting the bluechip-side lookup into the snapshot eliminates the
/// per-sample O(N) scan of `POOLS_BY_ID` that previously dominated oracle
/// update gas at scale (75 sampled pools × N total pools = O(N²) reads).
/// `#[serde(default)]` lets snapshots written by the pre-cache code path
/// deserialize cleanly with an empty `bluechip_indices`; the oracle's
/// is_bluechip_second resolution falls back to the linear scan in that
/// (one-time) case until the next refresh repopulates the cache.
#[cw_serde]
pub struct EligiblePoolSnapshot {
    pub pool_addresses: Vec<String>,
    /// 0 = bluechip is reserve0, 1 = bluechip is reserve1.
    /// Stored as u8 (rather than bool) because future pool variants may
    /// extend the encoding, and u8 round-trips cleanly through cw_serde.
    #[serde(default)]
    pub bluechip_indices: Vec<u8>,
    pub captured_at_block: u64,
}

pub const ELIGIBLE_POOL_SNAPSHOT: Item<EligiblePoolSnapshot> =
    Item::new("eligible_pool_snap");

/// How stale the snapshot is allowed to get before `select_random_pools_with_atom`
/// rebuilds it. 72_000 blocks at 6s per block ≈ 5 days.
pub const ELIGIBLE_POOL_REFRESH_BLOCKS: u64 = 72_000;

// ---------------------------------------------------------------------------
// Oracle-eligible pool curation (audit M-3)
// ---------------------------------------------------------------------------
//
// Two parallel inputs feed `get_eligible_creator_pools`:
//
//   1. `ORACLE_ELIGIBLE_POOLS` — admin-curated allowlist. Any pool kind
//      (Standard or Commit). Required for the early-stage roadmap where
//      bluechip/IBC standard pools are the only externally-priced sources.
//      Add: 48h timelock via `PENDING_ORACLE_ELIGIBLE_POOL_ADD`. Remove:
//      immediate (always safe to drop a contributor).
//
//   2. `COMMIT_POOLS_AUTO_ELIGIBLE` — global flag. When true, every
//      threshold-crossed `PoolKind::Commit` pool also flows in
//      automatically (legacy behaviour, programmatic gate). Default
//      false on fresh deployments; the migrate handler sets it true
//      to preserve current-behaviour for existing chains. Flip:
//      48h timelock via `PENDING_COMMIT_POOLS_AUTO_ELIGIBLE`.
//
// Snapshot rebuild reads both, dedupes, runs the cross-contract
// liquidity / bluechip-side check, and writes `ELIGIBLE_POOL_SNAPSHOT`.
// The refresh job NEVER writes to either input — only admin actions
// (gated by the timelocks above) can change which pools the oracle
// samples.

/// One entry in the admin-curated oracle allowlist.
///
/// `bluechip_index` is resolved at apply-time (post-timelock) so the
/// per-sample lookup at oracle-update time is O(1) instead of an O(N)
/// re-scan of `POOLS_BY_ID`. The same index is mirrored into
/// `ELIGIBLE_POOL_SNAPSHOT.bluechip_indices` on every refresh.
///
/// `added_at` is for observability only — operators can audit the age
/// of every allowlist entry without scanning logs.
#[cw_serde]
pub struct AllowlistedOraclePool {
    pub bluechip_index: u8,
    pub added_at: Timestamp,
}

pub const ORACLE_ELIGIBLE_POOLS: Map<Addr, AllowlistedOraclePool> =
    Map::new("oracle_eligible_pools");

/// Pending allowlist additions awaiting timelock expiry. Keyed by the
/// proposed pool's contract address so the admin can have multiple
/// distinct adds in flight at once. Each entry stores the proposal
/// timestamp; `effective_after` = `proposed_at + ADMIN_TIMELOCK_SECONDS`.
/// `apply` reverts if `block.time < effective_after`. `cancel` removes
/// the entry without applying.
#[cw_serde]
pub struct PendingOracleEligiblePoolAdd {
    pub proposed_at: Timestamp,
    /// Pre-resolved bluechip-side index, captured at propose time so the
    /// apply path doesn't have to re-scan `POOLS_BY_ID`. Re-validated
    /// against the current pool token info on apply (defense in depth
    /// against pool-token-info mutations between propose and apply,
    /// which shouldn't happen but isn't disprovable in storage).
    pub bluechip_index: u8,
}

pub const PENDING_ORACLE_ELIGIBLE_POOL_ADD: Map<Addr, PendingOracleEligiblePoolAdd> =
    Map::new("pending_oracle_eligible_add");

/// Global flag controlling whether threshold-crossed `PoolKind::Commit`
/// pools flow into the oracle snapshot automatically (in addition to
/// the curated allowlist).
///
/// Default behaviour:
///   - Fresh instantiate: flag missing → treated as `false`. Admin
///     must explicitly opt in to auto-include commit pools (matches
///     stages 1–3 of the roadmap where curation is manual).
///   - Migrate from pre-flag versions: handler explicitly sets `true`
///     to preserve the legacy "commits auto-eligible on threshold
///     cross" behaviour for existing deployments.
///
/// Flipping the flag goes through the standard 48h timelock (both
/// directions — turning OFF deserves observability so creators
/// can react before their pools stop contributing).
pub const COMMIT_POOLS_AUTO_ELIGIBLE: Item<bool> =
    Item::new("commit_pools_auto_eligible");

pub fn load_commit_pools_auto_eligible(storage: &dyn Storage) -> bool {
    COMMIT_POOLS_AUTO_ELIGIBLE
        .may_load(storage)
        .ok()
        .flatten()
        .unwrap_or(false)
}

#[cw_serde]
pub struct PendingCommitPoolsAutoEligible {
    pub new_value: bool,
    pub proposed_at: Timestamp,
}

pub const PENDING_COMMIT_POOLS_AUTO_ELIGIBLE: Item<PendingCommitPoolsAutoEligible> =
    Item::new("pending_commit_auto_eligible");

/// Rate limit on the permissionless `RefreshOraclePoolSnapshot`.
/// Mirrors the cadence of `ELIGIBLE_POOL_REFRESH_BLOCKS / 10` so the
/// permissionless path is meaningfully more responsive than the lazy
/// in-line refresh inside `select_random_pools_with_atom`, but can't
/// be spammed to burn keeper / public gas. ≈12h between forced
/// refreshes at 6s blocks.
pub const ORACLE_REFRESH_RATE_LIMIT_BLOCKS: u64 = 7_200;
pub const LAST_ORACLE_REFRESH_BLOCK: Item<u64> = Item::new("last_oracle_refresh_block");


#[cw_serde]
pub enum CreationStatus {
    Started,
    TokenCreated,
    NftCreated,
    PoolCreated,
    Completed,
    Failed,
    CleaningUp,
}

#[cw_serde]
pub struct PoolUpgrade {
    pub new_code_id: u64,
    pub migrate_msg: Binary,
    pub pools_to_upgrade: Vec<u64>,
    pub upgraded_count: u32,
    pub effective_after: Timestamp,
}

// ---------------------------------------------------------------------------
// Pool registry helpers
// ---------------------------------------------------------------------------
// Centralized so the three pool-registry maps cannot drift. Direct writes to
// POOLS_BY_ID / POOLS_BY_CONTRACT_ADDRESS / PAIRS outside this module risk
// leaving the factory's view of pools internally inconsistent.

/// Canonicalized fingerprint of a single side of a pool pair.
///
/// Native denoms and CW20 contract addresses are both stringly-typed, so a
/// kind-tag prefix is required to keep them in disjoint namespaces — a
/// chain that ever ended up with a CW20 contract address that happens to
/// equal a native denom string would otherwise alias two different
/// asset references onto the same key. The prefixes (`n:` for native,
/// `c:` for creator-token) are short, opaque to user-facing surfaces
/// (the key is internal-only), and stable forever — changing them is a
/// breaking storage migration.
fn token_fingerprint(t: &TokenType) -> String {
    match t {
        TokenType::Native { denom } => format!("n:{}", denom),
        TokenType::CreatorToken { contract_addr } => format!("c:{}", contract_addr),
    }
}

/// Order-independent key for the `(asset_a, asset_b)` uniqueness map.
///
/// The two fingerprints are sorted lexicographically before being returned
/// as `(min, max)`, so `[A, B]` and `[B, A]` map to the same storage slot.
/// This matches Uniswap V2's `getPair[a][b] == getPair[b][a]` convention
/// and is the right shape for "at most one pool per unordered pair." If a
/// future pool variant ever needs to permit parallel pools at different
/// fee tiers / curve types / hook configurations, widen this key with the
/// extra discriminator(s) — do NOT add a parallel uniqueness map.
pub fn canonical_pair_key(pair: &[TokenType; 2]) -> (String, String) {
    let a = token_fingerprint(&pair[0]);
    let b = token_fingerprint(&pair[1]);
    if a <= b { (a, b) } else { (b, a) }
}

/// Atomically register a freshly created pool across all three registry
/// maps. Rejects with a generic_err if `pair` already exists in `PAIRS`
/// — this is the canonical guard against silent duplicate registrations
/// from any code path (entry-point pre-check, future admin restore,
/// migrate back-fill, etc). The pre-check at the create entry points
/// exists purely to fail-fast before the caller's fee is forwarded;
/// THIS is the load-bearing check.
///
/// Initial `PoolStateResponseForFactory` is materialized from `pool_details`
/// — caller doesn't need to construct it. Reserves and TWAP accumulators
/// start at zero; the pool itself updates them as activity flows through.
pub fn register_pool(
    storage: &mut dyn Storage,
    pool_id: u64,
    pool_address: &Addr,
    pool_details: &PoolDetails,
) -> StdResult<()> {
    let pair_key = canonical_pair_key(&pool_details.pool_token_info);
    if let Some(existing) = PAIRS.may_load(storage, pair_key.clone())? {
        return Err(cosmwasm_std::StdError::generic_err(format!(
            "duplicate pair: pool_id {} already registered for ({}, {})",
            existing, pair_key.0, pair_key.1
        )));
    }
    PAIRS.save(storage, pair_key, &pool_id)?;

    POOLS_BY_ID.save(storage, pool_id, pool_details)?;
    // Reverse index — see `POOL_ID_BY_ADDRESS` doc. Written here so the
    // three-map invariant becomes a four-map invariant inside this
    // single helper rather than every call site having to know about it.
    POOL_ID_BY_ADDRESS.save(storage, pool_address.clone(), &pool_id)?;

    let asset_strings: Vec<String> = pool_details
        .pool_token_info
        .iter()
        .map(|t| match t {
            TokenType::Native { denom } => denom.clone(),
            TokenType::CreatorToken { contract_addr } => contract_addr.to_string(),
        })
        .collect();

    POOLS_BY_CONTRACT_ADDRESS.save(
        storage,
        pool_address.clone(),
        &PoolStateResponseForFactory {
            pool_contract_address: pool_address.clone(),
            nft_ownership_accepted: false,
            reserve0: Uint128::zero(),
            reserve1: Uint128::zero(),
            total_liquidity: Uint128::zero(),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: asset_strings,
        },
    )?;

    Ok(())
}
