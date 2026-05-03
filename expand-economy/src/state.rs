use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};

/// Rolling 24-hour cap on `RequestExpansion` payouts (ubluechip, base
/// units). Bounds the worst-case daily drain if the configured factory
/// address is ever compromised: even with full factory control, an
/// attacker can extract at most this much per day, capped by the
/// expand-economy's own balance.
///
/// 100_000_000_000 ubluechip = 100,000 bluechip in base-decimal units
/// (assumes the canonical 6-decimal denom). The factory's
/// `calculate_mint_amount` polynomial returns up to ~500 bluechip per
/// pool for the very first threshold-crossing and tapers toward zero
/// thereafter, so this cap leaves headroom for ~200 early-pool mints
/// per 24h — well above the natural protocol rate, well below the
/// "drain-the-reservoir" attack rate.
pub const DAILY_EXPANSION_CAP: Uint128 = Uint128::new(100_000_000_000);
/// Length of the rolling cap window in seconds (24 hours).
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

/// Persisted rolling-window state for `RequestExpansion` cap accounting.
pub const EXPANSION_WINDOW: Item<ExpansionWindow> = Item::new("expansion_window");

/// Per-recipient rate limit on `RequestExpansion`. Maps recipient address
/// (as String — the wire format the factory passes) to the block-time of
/// the most recent successful expansion paid to that recipient.
///
/// Defends against `RetryFactoryNotify` storms exhausting the daily
/// `DAILY_EXPANSION_CAP` budget. A per-pool rate limit would have to
/// include the pool's controlling identity to be effective, which would
/// eliminate the permissionlessness of `RetryFactoryNotify` (the design
/// goal there is that any keeper or committer can nudge the system back
/// to consistent state). A per-recipient limit keeps retry permissionless
/// while preventing a flurry of retries against a single bluechip wallet
/// from burning the rolling window's budget on no-op (already-minted)
/// or compressed-into-one-burst payouts.
///
/// Updated only on successful payouts (skipped requests — insufficient
/// balance, dormant economy — do not stamp the timestamp).
pub const LAST_EXPANSION_AT_RECIPIENT: Map<&str, Timestamp> =
    Map::new("last_expansion_at_recipient");

/// Minimum interval between successive `RequestExpansion` payouts to the
/// same recipient. 60s is well below the natural protocol cadence (the
/// factory's `update_internal_oracle_price` is gated at `UPDATE_INTERVAL =
/// 300s`, and threshold-crossings happen at human-driven cadence) but tight
/// enough that retry-storms cannot empty the daily budget faster than
/// `DAILY_EXPANSION_CAP / interval`. With the 100k-bluechip cap and 60s
/// floor, the worst-case drain rate is bounded at the natural rate of
/// legitimate threshold mints.
pub const RECIPIENT_EXPANSION_RATE_LIMIT_SECONDS: u64 = 60;

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

/// Persisted contract `Config` (factory address, owner, bluechip denom).
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

/// Timelock between `ProposeWithdrawal` and `ExecuteWithdrawal` (48 hours).
pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 172_800;
/// Timelock between `ProposeConfigUpdate` and `ExecuteConfigUpdate` (48 hours).
pub const CONFIG_TIMELOCK_SECONDS: u64 = 172_800;
/// Pending withdrawal awaiting timelock expiry.
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
/// Pending config update awaiting timelock expiry.
pub const PENDING_CONFIG_UPDATE: Item<PendingConfigUpdate> = Item::new("pending_config_update");
