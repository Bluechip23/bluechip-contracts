//! Persistent state for the router contract.
//!
//! The router intentionally caches no pool data. Every execution and
//! simulation queries the factory and target pools live so that the
//! router never serves stale liquidity, fee, or price information.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp};
use cw_storage_plus::Item;

/// Hard cap on hops in a single route. Most real swaps are 2 hops
/// (X1 -> bluechip -> X2). 3 covers exotic cases (e.g. when an indexer
/// finds a better price across an intermediate creator token) without
/// letting callers chain unbounded transactions.
pub const MAX_HOPS: usize = 3;

/// Timelock applied to admin-initiated config mutations on the router.
/// Mirrors `factory::state::ADMIN_TIMELOCK_SECONDS`. Two days gives the
/// community a full observability window to detect a compromised-admin
/// rotation before it lands. The router holds no funds, so the practical
/// blast radius of a hostile admin rotation is "lock the legitimate
/// operator out of further config changes" — bounded but worth the
/// timelock so a key compromise doesn't take effect instantly.
pub const ROUTER_TIMELOCK_SECONDS: u64 = 86_400 * 2;

/// Router configuration. Both fields are mutable, but only via the
/// 48h-timelocked propose/apply flow
/// (`ProposeConfigUpdate` -> wait -> `UpdateConfig`). Set at
/// instantiate; changed only by the admin.
#[cw_serde]
pub struct Config {
    /// Bluechip factory address used to discover and validate pools.
    pub factory_addr: Addr,
    /// Native denom that acts as the routing reserve currency. Every
    /// Bluechip pool pairs this denom with a creator token, so the
    /// indexer almost always builds routes that pass through it.
    pub bluechip_denom: String,
    /// Address authorized to update this config.
    pub admin: Addr,
}

pub const CONFIG: Item<Config> = Item::new("config");

/// Pending router-config update awaiting timelock expiry.
///
/// Either `admin` or `factory_addr` may be `None` (no change to that
/// field). Re-proposing while a pending update exists is rejected; the
/// admin must explicitly `CancelConfigUpdate` first so any community
/// watcher polling `PENDING_CONFIG` always sees an explicit cancellation
/// event before a different proposal replaces it.
#[cw_serde]
pub struct PendingConfigUpdate {
    pub admin: Option<String>,
    pub factory_addr: Option<String>,
    pub effective_after: Timestamp,
}

/// Pending update awaiting `effective_after`. Cleared by
/// `UpdateConfig` (apply) or `CancelConfigUpdate`.
pub const PENDING_CONFIG: Item<PendingConfigUpdate> = Item::new("pending_config");
