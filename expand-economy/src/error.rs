use cosmwasm_std::{OverflowError, StdError, Timestamp, Uint128};
use cw_utils::PaymentError;
use thiserror::Error;

/// Reason a `bluechip_denom` failed [`crate::denom::validate_native_denom`].
/// Lets clients distinguish "wrong length" from "wrong leading char" from
/// "bad inner character" without parsing English error messages.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum InvalidDenomReason {
    #[error("length {len} is outside the cosmos-sdk allowed range [3, 128]")]
    LengthOutOfRange { len: usize },
    #[error("must start with an ASCII letter; got leading character '{first}'")]
    BadLeadingCharacter { first: char },
    #[error(
        "contains disallowed character '{ch}'; cosmos-sdk denoms accept only \
         alphanumerics and / : . _ -"
    )]
    DisallowedCharacter { ch: char },
}

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Daily expansion cap exceeded: requested {requested}, already spent {spent_in_window} in current 24h window, cap {cap}")]
    DailyExpansionCapExceeded {
        requested: Uint128,
        spent_in_window: Uint128,
        cap: Uint128,
    },

    /// Any execute path on this contract is non-payable. Surfaces
    /// `cw_utils::nonpayable` rejections directly so the underlying
    /// `PaymentError::NonPayable {}` reason ("This message does no accept
    /// funds") is preserved for clients.
    #[error("{0}")]
    Payment(#[from] PaymentError),

    // -----------------------------------------------------------------
    // Domain-specific variants. Replace earlier `Std(generic_err(...))`
    // sites so off-chain consumers can match structurally rather than
    // regex an English message.
    // -----------------------------------------------------------------
    #[error("Timelock not expired. Execute after: {ready_at}")]
    TimelockNotExpired { ready_at: Timestamp },

    #[error(
        "bluechip_denom mismatch: factory has \"{factory}\", expand-economy \
         has \"{expand_economy}\". Update one side via its config-update flow \
         before retrying."
    )]
    BluechipDenomMismatch {
        factory: String,
        expand_economy: String,
    },

    #[error("Failed to query factory config for denom validation: {reason}")]
    FactoryQueryFailed { reason: String },

    #[error("A config update is already pending. Cancel it first.")]
    ConfigUpdateAlreadyPending,

    #[error("A withdrawal is already pending. Cancel it first.")]
    WithdrawalAlreadyPending,

    #[error("Withdrawal amount must be non-zero")]
    WithdrawalAmountZero,

    #[error("No pending config update to execute")]
    NoPendingConfigUpdateToExecute,

    #[error("No pending config update to cancel")]
    NoPendingConfigUpdateToCancel,

    #[error("No pending withdrawal to execute")]
    NoPendingWithdrawalToExecute,

    #[error("No pending withdrawal to cancel")]
    NoPendingWithdrawalToCancel,

    #[error("bluechip_denom \"{denom}\" is invalid: {reason}")]
    InvalidDenom {
        denom: String,
        reason: InvalidDenomReason,
    },

    #[error("Migration would downgrade contract from {stored} to {current}; refusing.")]
    DowngradeRefused { stored: String, current: String },

    #[error("Stored cw2 contract version {version} is not valid semver: {msg}")]
    StoredVersionInvalid { version: String, msg: String },

    #[error("Compile-time CONTRACT_VERSION {version} is not valid semver: {msg}")]
    CurrentVersionInvalid { version: String, msg: String },
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
