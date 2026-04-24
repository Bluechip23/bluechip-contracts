use crate::asset::TokenType;
use crate::pool_struct::{PoolDetails, TempPoolCreation};
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

// Two coupled pool-registry maps. They MUST stay in sync — every pool
// that exists must appear in both. Always go through `register_pool`
// rather than touching them individually.
//   - POOLS_BY_ID:               pool_id  -> PoolDetails (token info, addresses)
//   - POOLS_BY_CONTRACT_ADDRESS: pool addr -> snapshot used by oracle / queries
pub const POOLS_BY_ID: Map<u64, PoolDetails> = Map::new("pools_by_id");
pub const POOLS_BY_CONTRACT_ADDRESS: Map<Addr, PoolStateResponseForFactory> =
    Map::new("pools_by_contract_address");
// Maximum age (seconds) of a Pyth price we are willing to use for USD
// conversions. Tightened from 300s to 90s: a 5-minute window let an
// attacker who spotted a favorable price pick-and-choose any moment in
// the last 5 minutes to land a commit/swap. 90 seconds is inside typical
// Pyth publish cadence while still cutting the attacker's useful window
// to a fraction of a volatility half-life.
pub const MAX_PRICE_AGE_SECONDS_BEFORE_STALE: u64 = 90;

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
// keeper gas is on the order of $0.003–$0.03 per oracle update on typical
// Cosmos chains; $0.10 leaves generous headroom for gas spikes while
// capping the yearly drain if admin is compromised: $0.10 × 288 updates/day
// = $28.80/day ≈ $10.5k/year max. The prior $1.00 cap was 10× higher and
// pure overpayment.
pub const MAX_ORACLE_UPDATE_BOUNTY_USD: Uint128 = Uint128::new(100_000);

// Native denom the bounty is paid in (after USD->bluechip conversion).
// The factory must be pre-funded with this denom by the bluechip main
// wallet.
pub const ORACLE_BOUNTY_DENOM: &str = "ubluechip";

// Keeper bounty paid per successful pool.ContinueDistribution batch.
// USD-denominated (6 decimals). Same conversion-at-payout pattern as
// the oracle bounty, so keeper economics stay stable as bluechip price
// moves. Pool LP reserves are never tapped — the factory pays from its
// own pre-funded native balance.
pub const DISTRIBUTION_BOUNTY_USD: Item<Uint128> = Item::new("distribution_bounty_usd");

// Hard cap. $0.10 USD per batch (6 decimals). A distribution batch is
// up to MAX_DISTRIBUTIONS_PER_TX=40 mints + a handful of storage writes;
// realistic gas ~$0.01–$0.10. The $0.10 cap leaves margin for expensive
// chains and gas-price spikes but still caps admin-compromise blast
// radius: at worst a compromised admin burns $0.10 × committer_count/40
// per pool's full distribution. Was $1.00, which was ~10× overpayment.
pub const MAX_DISTRIBUTION_BOUNTY_USD: Uint128 = Uint128::new(100_000);

// ForceRotateOraclePools is a 2-step action: admin proposes a rotation,
// the timelock elapses, then admin invokes ForceRotateOraclePools to
// execute. Prevents a compromised admin from instantly rotating the
// oracle's pool sample set without a 48h observability window for the
// community to notice and respond.
pub const PENDING_ORACLE_ROTATION: Item<Timestamp> = Item::new("pending_oracle_rotation");

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
    pub commit_amount_for_threshold_bluechip: Uint128,
    pub commit_threshold_limit_usd: Uint128,
    pub pyth_contract_addr_for_conversions: String,
    pub pyth_atom_usd_price_feed_id: String,
    pub cw20_token_contract_id: u64,
    pub cw721_nft_contract_id: u64,
    pub create_pool_wasm_contract_id: u64,
    /// Code ID for the standard-pool wasm (split out of creator-pool in
    /// H14). Defaults to `0` on old serialized records so factories
    /// deployed pre-split continue to deserialize; operators must propose
    /// a config update that sets this field before `CreateStandardPool`
    /// can succeed. Standard pools instantiate against THIS code_id,
    /// not `create_pool_wasm_contract_id`.
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
    pub creator_token_address: Option<Addr>,
    pub mint_new_position_nft_address: Option<Addr>,
    pub pool_address: Option<Addr>,
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
#[cw_serde]
pub struct EligiblePoolSnapshot {
    pub pool_addresses: Vec<String>,
    pub captured_at_block: u64,
}

pub const ELIGIBLE_POOL_SNAPSHOT: Item<EligiblePoolSnapshot> =
    Item::new("eligible_pool_snap");

/// How stale the snapshot is allowed to get before `select_random_pools_with_atom`
/// rebuilds it. 72_000 blocks at 6s per block ≈ 5 days.
pub const ELIGIBLE_POOL_REFRESH_BLOCKS: u64 = 72_000;

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
// Centralized so the two pool-registry maps cannot drift. Direct writes to
// POOLS_BY_ID / POOLS_BY_CONTRACT_ADDRESS outside this module risk leaving
// the factory's view of pools internally inconsistent.

/// Atomically register a freshly created pool across both registry maps.
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
    POOLS_BY_ID.save(storage, pool_id, pool_details)?;

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
