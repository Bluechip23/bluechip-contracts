use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::Item;

/// Rolling 24-hour cap on `RequestExpansion` payouts (ubluechip, base
/// units). Bounds the worst-case daily drain if the configured factory
/// address is ever compromised: even with full factory control, an
/// attacker can extract at most this much per day, capped by the
/// expand-economy's own balance. Tuned for "big enough to cover normal
/// threshold-crossing schedule (~one per N hours), small enough to make
/// a key-compromise attack uneconomic."
///
/// 100_000 ubluechip = 0.1 bluechip in base-decimal units. Threshold
/// mints are a few hundred bluechip per pool early on, tapering toward
/// zero. The cap is sized to allow ~1000 small mints/day before
/// rate-limiting kicks in — well above the natural protocol rate, well
/// below the "drain-the-reservoir" attack rate.
pub const DAILY_EXPANSION_CAP: Uint128 = Uint128::new(100_000);
pub const DAILY_WINDOW_SECONDS: u64 = 86_400;

/// Snapshot of recent RequestExpansion volume for the rolling cap. We
/// approximate "rolling 24h" with a single-bucket reset: if the saved
/// `window_start` is older than `DAILY_WINDOW_SECONDS`, treat it as a
/// fresh window and reset `spent_in_window` to zero. Drift error vs a
/// proper sliding window is bounded by the bucket size and only ever
/// LETS more through right after a reset, never less — never blocks a
/// legitimate payout that should have fit. Storage cost is constant
/// (one Item) vs O(N) for a true sliding-window log.
#[cw_serde]
pub struct ExpansionWindow {
    pub window_start: Timestamp,
    pub spent_in_window: Uint128,
}

pub const EXPANSION_WINDOW: Item<ExpansionWindow> = Item::new("expansion_window");

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
