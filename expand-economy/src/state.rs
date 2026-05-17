use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::Item;

/// Rolling 24-hour cap on `RequestExpansion` payouts (ubluechip, base
/// units). Bounds the worst-case daily drain if the configured factory
/// address is ever compromised: even with full factory control, an
/// attacker can extract at most this much per any 24-hour window via
/// this path, capped further by the expand-economy's own balance.
///
/// 100_000_000_000 ubluechip = 100,000 bluechip in base-decimal units
/// (assumes the canonical 6-decimal denom). The factory's
/// `calculate_mint_amount` polynomial returns up to ~500 bluechip per
/// pool for the very first threshold-crossing and tapers toward zero
/// thereafter, so this cap leaves headroom for ~200 early-pool mints
/// per 24h — well above the natural protocol rate, well below the
/// "drain-the-reservoir" attack rate.
///
/// FUTURE: this constant + the owner/factory single-key trust model
/// are scheduled to move behind a multisig as part of the operational
/// hardening roadmap. Until that lands, the cap is the protocol-level
/// belt against any single-key compromise on the expand-economy axis.
pub const DAILY_EXPANSION_CAP: Uint128 = Uint128::new(100_000_000_000);
/// Length of the rolling cap window in seconds (24 hours).
pub const DAILY_WINDOW_SECONDS: u64 = 86_400;

/// One persisted RequestExpansion payout in the rolling-window log.
/// `timestamp` is the block time the payout was persisted at;
/// `amount` is the bluechip-denom amount actually committed (i.e. after
/// the balance-graceful-skip check, so unfulfilled requests are NOT in
/// the log and don't debit cap budget).
#[cw_serde]
pub struct ExpansionEntry {
    pub timestamp: Timestamp,
    pub amount: Uint128,
}

/// Sliding-window log backing the daily cap. Each successful
/// `RequestExpansion` appends one entry; every call prunes entries
/// older than `DAILY_WINDOW_SECONDS` before summing the in-window
/// total and checking it against `DAILY_EXPANSION_CAP`.
///
/// Compared to the prior single-bucket reset, this prevents the
/// boundary-burst case where an attacker could max out the cap just
/// before the bucket flipped at `window_start + 24h` AND immediately
/// max out the fresh bucket on the other side — sliding semantics
/// continuously age entries out one at a time, so any rolling 24h
/// window across the boundary still sees only `DAILY_EXPANSION_CAP`
/// in aggregate.
///
/// Size is bounded in practice: the cap caps total per-window volume,
/// and per-pool mint amounts have a polynomial decay floor that bounds
/// "how many distinct mints fit in 100k bluechip." Even worst-case
/// (tail mints of ~1 bluechip each), the log can hold at most
/// `cap / min_mint` ≈ 1e5 entries; realistic deployments will see <500
/// entries. Stored as `Item<Vec<_>>` rather than a `Map` because the
/// whole log is read on every call (we sum + prune) so the per-call
/// SerDe cost is the same and the simpler shape avoids a sequence
/// counter.
pub const EXPANSION_LOG: Item<Vec<ExpansionEntry>> = Item::new("expansion_log");

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
///
/// Set in `instantiate` and never removed. Handlers use `CONFIG.load`
/// rather than `may_load` because the absence of a value would only
/// mean the contract was never instantiated — a programmer-error
/// condition rather than a domain failure.
pub const CONFIG: Item<Config> = Item::new("config");

/// Default `bluechip_denom` for `InstantiateMsg` when the field is omitted.
/// Matches the pre-existing hardcoded value so upgraders don't need to touch
/// anything unless the chain denom changes.
pub const DEFAULT_BLUECHIP_DENOM: &str = "ubluechip";

/// Pending withdrawal awaiting timelock expiry.
///
/// `recipient` is stored as `Addr` after `addr_validate` at propose time
/// so the apply path doesn't have to re-validate. `unlocks_at` is the
/// block time at-or-after which the withdrawal becomes executable;
/// `serde(alias = "execute_after")` keeps records written by the
/// pre-rename code path deserializing cleanly.
#[cw_serde]
pub struct PendingWithdrawal {
    pub amount: Uint128,
    pub denom: String,
    pub recipient: Addr,
    #[serde(alias = "execute_after")]
    pub unlocks_at: Timestamp,
}

/// Timelock between `ProposeWithdrawal` and `ExecuteWithdrawal` (48 hours
/// in production; 120s under `--features mock` for local integration tests).
#[cfg(not(feature = "integration_short_timing"))]
pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 172_800;
#[cfg(feature = "integration_short_timing")]
pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 120;
/// Timelock between `ProposeConfigUpdate` and `ExecuteConfigUpdate` (48 hours
/// in production; 120s under `--features mock` for local integration tests).
#[cfg(not(feature = "integration_short_timing"))]
pub const CONFIG_TIMELOCK_SECONDS: u64 = 172_800;
#[cfg(feature = "integration_short_timing")]
pub const CONFIG_TIMELOCK_SECONDS: u64 = 120;
/// Pending withdrawal awaiting timelock expiry.
pub const PENDING_WITHDRAWAL: Item<PendingWithdrawal> = Item::new("pending_withdrawal");

/// Pending config update awaiting timelock expiry.
///
/// Address fields are validated at propose time and stored as
/// `Option<Addr>` so the apply path doesn't have to re-validate. The
/// `unlocks_at` field uses `serde(alias = "effective_after")` so
/// records written by the pre-rename code path deserialize cleanly.
#[cw_serde]
pub struct PendingConfigUpdate {
    pub factory_address: Option<Addr>,
    pub owner: Option<Addr>,
    /// If set, applied to `Config.bluechip_denom` when the timelock expires.
    /// Unset means "don't change the denom". Validated at propose time
    /// against the cosmos-sdk denom rules (length, leading char, allowed
    /// inner chars) so an invalid string fails at propose rather than 48h
    /// later at apply.
    #[serde(default)]
    pub bluechip_denom: Option<String>,
    #[serde(alias = "effective_after")]
    pub unlocks_at: Timestamp,
}
/// Pending config update awaiting timelock expiry.
pub const PENDING_CONFIG_UPDATE: Item<PendingConfigUpdate> = Item::new("pending_config_update");
