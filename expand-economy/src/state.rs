use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::Item;

#[cw_serde]
pub struct Config {
    pub factory_address: Addr,
    pub owner: Addr,
    /// Native bank denom used by `RequestExpansion` when minting rewards.
    /// Previously hardcoded to `"ubluechip"` in the handler; lifted here so
    /// the chain denom is a deployment parameter rather than a compile-time
    /// string. Changeable via the standard 48h timelocked config-update flow.
    pub bluechip_denom: String,
}

pub const CONFIG: Item<Config> = Item::new("config");

/// Default `bluechip_denom` for `InstantiateMsg` when the field is omitted.
/// Matches the pre-existing hardcoded value so upgraders don't need to touch
/// anything unless the chain denom changes.
pub const DEFAULT_BLUECHIP_DENOM: &str = "ubluechip";

#[cw_serde]
pub struct PendingWithdrawal {
    pub amount: Uint128,
    pub denom: String,
    pub recipient: String,
    pub execute_after: Timestamp,
}

pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 172_800;
pub const CONFIG_TIMELOCK_SECONDS: u64 = 172_800;
pub const PENDING_WITHDRAWAL: Item<PendingWithdrawal> = Item::new("pending_withdrawal");

#[cw_serde]
pub struct PendingConfigUpdate {
    pub factory_address: Option<String>,
    pub owner: Option<String>,
    /// If set, applied to `Config.bluechip_denom` when the timelock expires.
    /// Unset means "don't change the denom". Stored as raw String — validated
    /// at propose time, not at apply time, so an empty string fails early.
    #[serde(default)]
    pub bluechip_denom: Option<String>,
    pub effective_after: Timestamp,
}
pub const PENDING_CONFIG_UPDATE: Item<PendingConfigUpdate> = Item::new("pending_config_update");
