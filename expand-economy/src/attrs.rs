//! Centralized attribute key + action-value constants.
//!
//! Every `Response::add_attribute` site in this crate references one of
//! these constants rather than a string literal. Off-chain indexers can
//! pin against the constants directly, and a typo in any single emission
//! becomes a compile error rather than a silent wire-format drift.

// ---- Attribute keys ------------------------------------------------------

pub const ACTION: &str = "action";
pub const RECIPIENT: &str = "recipient";
pub const AMOUNT: &str = "amount";
pub const DENOM: &str = "denom";
pub const FACTORY: &str = "factory";
pub const OWNER: &str = "owner";
pub const BLUECHIP_DENOM: &str = "bluechip_denom";
pub const REASON: &str = "reason";
pub const NOTE: &str = "note";
pub const VARIANT: &str = "variant";
pub const REQUESTED_AMOUNT: &str = "requested_amount";
pub const CONTRACT_BALANCE: &str = "contract_balance";
pub const SPENT_IN_WINDOW_AFTER: &str = "spent_in_window_after";
pub const DAILY_CAP: &str = "daily_cap";
/// Timelock unlock time, attached to every propose / execute response on
/// both the config and withdrawal flows. Single key — both timelock
/// kinds emit the same name so off-chain indexers can use one parser.
pub const UNLOCKS_AT: &str = "unlocks_at";
pub const CONTRACT_NAME: &str = "contract_name";
pub const CONTRACT_VERSION: &str = "contract_version";

// ---- Action values -------------------------------------------------------

pub const INSTANTIATE: &str = "instantiate";
pub const REQUEST_REWARD: &str = "request_reward";
pub const REQUEST_REWARD_SKIPPED: &str = "request_reward_skipped";
pub const PROPOSE_CONFIG_UPDATE: &str = "propose_config_update";
pub const EXECUTE_CONFIG_UPDATE: &str = "execute_config_update";
pub const CANCEL_CONFIG_UPDATE: &str = "cancel_config_update";
pub const PROPOSE_WITHDRAWAL: &str = "propose_withdrawal";
pub const EXECUTE_WITHDRAWAL: &str = "execute_withdrawal";
pub const CANCEL_WITHDRAWAL: &str = "cancel_withdrawal";
pub const MIGRATE: &str = "migrate";

// ---- Reason values (for `request_reward_skipped`) -----------------------

pub const REASON_ECONOMY_DORMANT: &str = "economy_dormant";
pub const REASON_INSUFFICIENT_BALANCE: &str = "insufficient_balance";

// ---- Note values ---------------------------------------------------------

pub const NOTE_ECONOMY_DORMANT: &str =
    "ExpandEconomy mint schedule has reached zero; no further expansions \
     will be dispensed. This is the intended end-state of the decay curve.";
pub const NOTE_WITHDRAWAL_NO_FUNDS: &str = "no funds available; withdrawal skipped";

// ---- Migrate variant values ---------------------------------------------

pub const MIGRATE_VARIANT_UPDATE_VERSION: &str = "update_version";
