use cosmwasm_std::{Decimal, OverflowError, StdError, Timestamp, Uint128};
use thiserror::Error;

/// Unified error type for every pool wasm (creator-pool and standard-pool).
///
/// Variants cover both shared concerns (swap/liquidity/admin) AND commit-
/// phase-specific concerns (ShortOfThreshold, InvalidThresholdParams,
/// TooFrequentCommits, MismatchAmount, etc.). Keeping the commit-phase
/// variants here — even though they are unreachable from the standard-pool
/// wasm — avoids a split-enum design where creator-pool would need its own
/// wrapper crate-error that re-exported `pool_core`'s and added commit
/// variants. A handful of unreachable variants cost nothing at runtime
/// and keep both contracts using the same type.
#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},
    #[error("Ratio deviation exceeded: expected {expected_ratio}, got {actual_ratio}, max {max_deviation_bps}bps vs actual {actual_deviation_bps}bps")]
    RatioDeviationExceeded {
        expected_ratio: Decimal,
        actual_ratio: Decimal,
        max_deviation_bps: u16,
        actual_deviation_bps: u16,
    },
    #[error("Position locked until {unlock_time}")]
    PositionLocked { unlock_time: Timestamp },
    #[error("The pool is paused due to low liquidity, please supply liquidity before swapping")]
    PoolPausedLowLiquidity {},
    #[error("No distribution or threshold locks and none are in progress")]
    NothingToRecover {},

    #[error("Invalid threshold parameters: {msg}")]
    InvalidThresholdParams { msg: String },

    #[error("Reentrancy detected")]
    ReentrancyGuard {},

    #[error("You can not swap until the threshold is crossed. You must commit to transact with this pool")]
    ShortOfThreshold {},

    #[error("You are trying to commit too frequently.")]
    TooFrequentCommits { wait_time: u64 },

    #[error("Division by zero error")]
    DivideByZero,

    #[error("percent must be between 1 and 99")]
    InvalidPercent,

    #[error("Zero won't work.")]
    ZeroAmount {},

    #[error("Transaction deadline has passed")]
    TransactionExpired {},

    #[error("Your commit amount does not match an amount designated by the creator of the pool.")]
    MismatchAmount {},

    #[error("Fee is too great or too small for this transaction")]
    InvalidFee {},

    #[error("Invalid commit floor for {field}: {got} (must be non-zero and <= {max})")]
    InvalidCommitFloor {
        field: &'static str,
        got: Uint128,
        max: Uint128,
    },

    #[error("belief_price cannot be zero")]
    InvalidBeliefPrice {},

    #[error("Slippage exceeded: expected at least {expected} {token}, got {actual}")]
    SlippageExceeded {
        expected: Uint128,
        actual: Uint128,
        token: String,
    },

    #[error("invalid amount of tokens")]
    InvalidAmount {},

    #[error("Invalid bluechip amount: expected {expected}, actual {actual}")]
    InvalidNativeAmount { expected: Uint128, actual: Uint128 },

    #[error("Oracle price is invalid (zero or negative)")]
    InvalidOraclePrice {},

    #[error("the pool is missing needed liquidity to carry out transaction")]
    InsufficientLiquidity {},

    #[error("Insufficient liquidity minted")]
    InsufficientLiquidityMinted {},

    #[error("Operation exceeds max spread limit")]
    MaxSpreadAssertion {},

    #[error("Doubling assets in asset infos")]
    DoublingAssets {},

    #[error("Asset mismatch between the requested and the stored asset in contract")]
    AssetMismatch {},

    #[error("pool can not cover reserves")]
    InsufficientReserves {},

    #[error("Pool has been permanently drained via emergency withdrawal")]
    EmergencyDrained {},

    #[error("Emergency withdraw timelock not yet elapsed. Executable after: {effective_after}")]
    EmergencyTimelockPending { effective_after: Timestamp },

    #[error("No pending emergency withdrawal to cancel")]
    NoPendingEmergencyWithdraw {},

    #[error("Post-threshold cooldown active: trades resume at block {until_block}")]
    PostThresholdCooldownActive { until_block: u64 },

    #[error("Cannot remove locked liquidity (this position has {locked} locked)")]
    LockedLiquidity { locked: Uint128 },

    // ---------------------------------------------------------------------
    // Domain-specific variants used by creator-pool. Replace earlier
    // `Std(StdError::generic_err(...))` sites so off-chain consumers can
    // match structurally rather than regex an English message.
    // ---------------------------------------------------------------------
    #[error(
        "lp_fee {got} is outside the allowed range [{min}, {max}] (set via UpdateFees)"
    )]
    LpFeeOutOfRange {
        got: Decimal,
        min: Decimal,
        max: Decimal,
    },

    #[error("No emergency-drain snapshot found — pool has not been drained yet")]
    NoEmergencyDrainSnapshot,

    #[error(
        "No claimable emergency share for position {position_id} (already claimed, exited \
         pre-drain, or position has zero liquidity)"
    )]
    NoClaimableEmergencyShare { position_id: String },

    #[error(
        "Emergency-claim dormancy not yet elapsed: drained at {drained_at}s, dormancy \
         expires at {dormancy_expires_at}s, current time {now}s. Treasury sweep is \
         only permitted after the dormancy window so passive LPs have a full year to \
         claim their share."
    )]
    EmergencyClaimDormancyNotElapsed {
        drained_at: u64,
        dormancy_expires_at: u64,
        now: u64,
    },

    #[error("No unclaimed emergency-drain residual to sweep")]
    NoUnclaimedEmergencyResidual,

    #[error("Distribution timeout - requires manual recovery")]
    DistributionTimeout,

    #[error(
        "Distribution failed too many times ({attempts} >= cap {cap}) - manual recovery needed: {reason}"
    )]
    DistributionFailedTooManyTimes {
        attempts: u32,
        cap: u32,
        reason: String,
    },

    #[error("Batch processing failed (attempt {attempt}): {reason}")]
    DistributionBatchFailed { attempt: u32, reason: String },

    #[error("Distribution-mint reply id {id} has no in-flight stash entry; pool state is corrupt or this reply id was reused")]
    PendingMintReplyMissing { id: u64 },

    #[error("No failed-mint entry for {user}; nothing to claim")]
    NoFailedMintEntry { user: String },

    #[error("Cannot skip {user}: no entry in COMMIT_LEDGER")]
    LedgerEntryNotFound { user: String },

    #[error(
        "Distribution is not stalled long enough for permissionless recovery: \
         time_since_last_update={elapsed}s, public-recovery window={window}s. \
         Use the admin recovery path (RecoverPoolStuckStates) for the shorter \
         {admin_window}s window."
    )]
    DistributionNotStalledForSelfRecover {
        elapsed: u64,
        window: u64,
        admin_window: u64,
    },

    #[error("No active distribution to self-recover")]
    NoDistributionToSelfRecover,

    #[error(
        "THRESHOLD_PROCESSING is stuck = true; should be unreachable in normal operation. \
         Use the factory's RecoverPoolStuckStates with StuckThreshold to clear it \
         (waits 1 hour from LAST_THRESHOLD_ATTEMPT), then retry the commit."
    )]
    StuckThresholdProcessing,

    #[error("Threshold payout corruption detected: components do not sum to expected total")]
    ThresholdPayoutCorruption,

    #[error("Migration would downgrade contract from {stored} to {current}; refusing.")]
    DowngradeRefused { stored: String, current: String },

    #[error("Stored cw2 contract version {version} is not valid semver: {msg}")]
    StoredVersionInvalid { version: String, msg: String },

    #[error("Compile-time CONTRACT_VERSION {version} is not valid semver: {msg}")]
    CurrentVersionInvalid { version: String, msg: String },

    #[error("Unknown reply id {id}")]
    UnknownReplyId { id: u64 },

    #[error("Invalid pair shape: {reason}")]
    InvalidPairShape { reason: String },

    #[error("Commit too small: ${got} USD (minimum ${min} USD {phase})")]
    CommitTooSmall {
        got: Uint128,
        min: Uint128,
        phase: &'static str,
    },

    #[error(
        "Invalid commit funds: {reason}. Commit must attach exactly the bluechip denom — \
         additional denoms (e.g., gas tokens, IBC assets) would be stranded in the pool with \
         no withdrawal path."
    )]
    InvalidCommitFunds { reason: String },

    /// M-7 audit fix. CW20-side swap entry (`execute_swap_cw20`) verifies
    /// that the CW20's claimed transfer actually landed on the pool by
    /// comparing the pool's current CW20 balance to the pre-Receive
    /// invariant `reserve + fee_reserve + creator_pot` plus the
    /// `cw20_msg.amount` the CW20 claims to have just transferred. A
    /// shortfall means either a malicious CW20 dispatched the Receive
    /// hook with a fabricated amount (no real transfer), a fee-on-
    /// transfer CW20 deducted on its way in, or a negative rebase. In
    /// every case crediting `amount` to the offer-side reserve would
    /// over-state reserves vs the pool's actual balance and let the
    /// caller drain the ask side. Rejecting here keeps the invariant.
    #[error(
        "CW20 ({cw20}) Receive-hook balance verification failed: actual pool \
         balance {actual} is below the expected post-transfer minimum \
         {expected_min} (offer side claimed +{claimed_amount}). \
         Likely cause: hostile/buggy CW20 that dispatched a Receive without \
         a matching transfer, a fee-on-transfer CW20 that skimmed the move, \
         or a negative-rebase token. Transaction reverted."
    )]
    Cw20SwapBalanceMismatch {
        cw20: String,
        expected_min: Uint128,
        actual: Uint128,
        claimed_amount: Uint128,
    },

    #[error("No pending factory notification to retry")]
    NoPendingFactoryNotify,

    #[error(
        "EmergencyWithdraw is disabled before the commit threshold has been crossed. \
         Committed funds are untracked in pool_state reserves and would be stranded."
    )]
    EmergencyWithdrawPreThreshold,

    #[error(
        "Rate-limited: this address can call ContinueDistribution again at {earliest_next} \
         (last call {last_call}, cooldown {cooldown_seconds}s)"
    )]
    ContinueDistributionRateLimited {
        earliest_next: u64,
        last_call: u64,
        cooldown_seconds: u64,
    },
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
