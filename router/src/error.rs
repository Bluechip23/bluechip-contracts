//! Error type for the router contract.
//!
//! All errors flow through [`RouterError`] so callers receive consistent,
//! contextual diagnostics. Errors that occur inside a specific hop carry
//! the hop index and pool address so frontends can highlight which leg of
//! a route caused a failure.
//!
//! Variants are kept narrow (few fields per variant) so the enum stays
//! within Clippy's `result_large_err` budget without boxing.

use cosmwasm_std::{StdError, Uint128};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum RouterError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Route is empty")]
    EmptyRoute,

    #[error("Route exceeds the maximum of {max} hops (got {got})")]
    MaxHopsExceeded { max: usize, got: usize },

    #[error("Offer amount must be greater than zero")]
    ZeroAmount,

    #[error("Route input and final output must differ")]
    SameInputOutput,

    /// Hop N declares an ask token that does not match hop N+1's offer
    /// token. `transition` is rendered as "<hop_n_ask> -> <hop_n+1_offer>".
    #[error("Route discontinuity at hop {hop_index} -> {next_hop_index}: {transition}")]
    RouteDiscontinuity {
        hop_index: usize,
        next_hop_index: usize,
        transition: String,
    },

    #[error("Hop {hop_index} on pool {pool_addr} failed: {reason}")]
    HopFailed {
        hop_index: usize,
        pool_addr: String,
        reason: String,
    },

    #[error(
        "Pool {pool_addr} at hop {hop_index} is still in its commit phase \
         (raised {raised}, target {target})"
    )]
    PoolInCommitPhase {
        hop_index: usize,
        pool_addr: String,
        raised: Uint128,
        target: Uint128,
    },

    #[error("Transaction deadline exceeded (deadline {deadline}, current {current})")]
    DeadlineExceeded { deadline: u64, current: u64 },

    #[error("Slippage exceeded: minimum receive {minimum}, actual {actual}")]
    SlippageExceeded { minimum: Uint128, actual: Uint128 },

    /// Config-update propose/apply timelock has not yet elapsed. Carries
    /// the effective-after timestamp so callers can compute the wait.
    #[error("Config update timelock not expired (effective after {effective_after})")]
    TimelockNotExpired { effective_after: u64 },

    /// `UpdateConfig` (apply) was called with no pending proposal. The
    /// admin must run `ProposeConfigUpdate` first.
    #[error("No pending config update; call ProposeConfigUpdate first")]
    NoPendingConfigUpdate,

    /// `ProposeConfigUpdate` was called while a prior pending proposal
    /// still exists. The admin must `CancelConfigUpdate` first so that
    /// any community watcher polling `PENDING_CONFIG` sees an explicit
    /// cancellation event before a replacement proposal lands.
    #[error("A config update is already pending; cancel it first via CancelConfigUpdate")]
    ConfigUpdateAlreadyPending,
}
