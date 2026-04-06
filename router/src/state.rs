//! Persistent state for the router contract.
//!
//! The router intentionally caches no pool data. Every execution and
//! simulation queries the factory and target pools live so that the
//! router never serves stale liquidity, fee, or price information.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::Addr;
use cw_storage_plus::Item;

/// Hard cap on hops in a single route. Most real swaps are 2 hops
/// (X1 -> bluechip -> X2). 3 covers exotic cases (e.g. when an indexer
/// finds a better price across an intermediate creator token) without
/// letting callers chain unbounded transactions.
pub const MAX_HOPS: usize = 3;

/// Router configuration. The only mutable field is `admin`; the bluechip
/// reserve denom and factory address are set at instantiation and
/// changed only via [`crate::msg::ExecuteMsg::UpdateConfig`] by the admin.
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
