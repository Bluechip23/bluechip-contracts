use cosmwasm_std::{OverflowError, StdError, Timestamp, Uint128};
use semver::Error as SemVerError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),
    #[error("Query error: {msg}")]
    QueryError { msg: String },
    #[error("Unauthorized")]
    Unauthorized {},

    #[error("The anchor atom/bluechip pool must exist and be active before generating a new set of pools for price")]
    MissingAtomPool {},

    #[error("Trying to update the oracle price too quickly. Please wait before updating again.")]
    UpdateTooSoon { next_update: u64 },

    #[error("You are missing important times and prices")]
    InsufficientData {},

    #[error("Contract Address Can Not Be Found")]
    ContractAddressNotFound {},

    #[error("Contract Failed Creating {}", id)]
    UnknownReplyId { id: u64 },

    #[error("SemVer parse error: {0}")]
    SemVer(#[from] SemVerError),
    #[error("Update is not yet effective. Can be applied after {effective_after}")]
    TimelockNotExpired { effective_after: Timestamp },

    #[error("TWAP circuit breaker tripped: drift {drift_bps} bps exceeds max {max_bps} bps (prior {prior}, new {new}). Oracle update rejected; investigate price-source pools before retrying.")]
    TwapCircuitBreaker {
        prior: Uint128,
        new: Uint128,
        drift_bps: u128,
        max_bps: u64,
    },

    // ---------------------------------------------------------------------
    // Oracle / Pyth domain errors. Replace earlier `Std(generic_err(...))`
    // sites so off-chain consumers (monitoring, retry, indexers) can match
    // structurally rather than regex an English message.
    // ---------------------------------------------------------------------
    #[error("Internal oracle not initialized")]
    OracleNotInitialized,

    #[error("Pyth ATOM/USD price unavailable: {reason}")]
    PythUnavailable { reason: String },

    #[error("Oracle price is stale (last update {last_update}s, now {now}s, max age {max_age}s)")]
    OraclePriceStale {
        last_update: u64,
        now: u64,
        max_age: u64,
    },

    #[error("Oracle TWAP price is zero")]
    TwapPriceZero,

    #[error("Calculated bluechip price is zero")]
    BluechipPriceZero,

    // ---------------------------------------------------------------------
    // Force-rotate / bootstrap timelock errors.
    // ---------------------------------------------------------------------
    #[error("A force-rotate is already pending. Cancel it first.")]
    ForceRotateAlreadyPending,

    #[error("No pending force-rotate to cancel")]
    NoPendingForceRotate,

    #[error("No pending bootstrap price to confirm. Wait for the next successful UpdateOraclePrice in branch (d) to populate one.")]
    NoPendingBootstrapPriceToConfirm,

    #[error("No pending bootstrap price to cancel")]
    NoPendingBootstrapPriceToCancel,

    // ---------------------------------------------------------------------
    // Oracle-eligibility curation.
    // ---------------------------------------------------------------------
    #[error("Pool {pool_addr} is already in the oracle allowlist")]
    OracleEligiblePoolAlreadyAdded { pool_addr: String },

    #[error("Pool {pool_addr} not found in factory pool registry")]
    OracleEligiblePoolNotInRegistry { pool_addr: String },

    #[error("Pool {pool_addr} has no bluechip side (cannot be priced against ATOM)")]
    OracleEligiblePoolMissingBluechipSide { pool_addr: String },

    #[error("Pool {pool_addr} already has a pending oracle-allowlist add")]
    OracleEligiblePoolAddAlreadyPending { pool_addr: String },

    #[error("Pool {pool_addr} has no pending oracle-allowlist add")]
    NoPendingOracleEligiblePoolAdd { pool_addr: String },

    #[error("Pool {pool_addr} is not in the oracle allowlist")]
    OracleEligiblePoolNotAllowlisted { pool_addr: String },

    #[error("Pool {pool_addr} is a pre-threshold commit pool (no real swap activity yet) — cannot contribute oracle price; allowlist it after its threshold has been crossed")]
    OracleEligiblePoolCommitPreThreshold { pool_addr: String },

    #[error("A commit-pools-auto-eligible flip is already pending. Cancel it first.")]
    CommitPoolsAutoEligibleAlreadyPending,

    #[error("No pending commit-pools-auto-eligible flip to cancel")]
    NoPendingCommitPoolsAutoEligible,

    #[error("commit_pools_auto_eligible already equals {value}; nothing to flip")]
    CommitPoolsAutoEligibleNoChange { value: bool },

    #[error("Oracle pool snapshot was refreshed too recently; next refresh available at block {next_block}")]
    OracleRefreshRateLimited { next_block: u64 },

    // ---------------------------------------------------------------------
    // Pool-creation reply chain errors.
    // ---------------------------------------------------------------------
    #[error("Pool reply '{step}' missing address: {kind}")]
    ReplyMissingAddress {
        step: &'static str,
        kind: &'static str,
    },

    #[error("Threshold payout corruption detected: components do not match factory config")]
    ThresholdPayoutCorruption,

    #[error("Reply for SubMsg id={id} returned an error in a reply_on_success path: {msg}")]
    ReplyOnSuccessSawError { id: u64, msg: String },

    #[error("Invalid pair shape: {reason}")]
    InvalidPairShape { reason: String },

    #[error(
        "Duplicate pair: pool_id {existing_pool_id} is already registered for ({asset_a}, {asset_b})"
    )]
    DuplicatePair {
        existing_pool_id: u64,
        asset_a: String,
        asset_b: String,
    },

    // ---------------------------------------------------------------------
    // Migration / config errors.
    // ---------------------------------------------------------------------
    #[error("Downgrade refused: stored {stored}, current {current}")]
    DowngradeRefused { stored: String, current: String },
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
