use cosmwasm_std::{Decimal, OverflowError, StdError, Timestamp, Uint128};
use thiserror::Error;

/// Unified error type for every pool wasm (creator-pool and standard-pool).
///
/// Variants cover both shared concerns (swap/liquidity/admin) AND commit-
/// phase-specific concerns (ShortOfThreshold, InvalidThresholdParams,
/// TooFrequentCommits, NotStuckYet, MismatchAmount, etc.). Keeping the
/// commit-phase variants here — even though they are unreachable from the
/// standard-pool wasm — avoids a split-enum design where creator-pool
/// would need its own wrapper crate-error that re-exported `pool_core`'s
/// and added commit variants. A handful of unreachable variants cost
/// nothing at runtime and keep both contracts using the same type.
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

    #[error("Unauthorized: Only creator can perform this action")]
    UnauthorizedNotCreator {},

    #[error("Invalid payment tiers: cannot be empty")]
    InvalidPaymentTiers {},

    #[error("CW20 tokens can be swapped via Cw20::Send message only")]
    Cw20DirectSwap {},

    #[error("Event of zero transfer")]
    InvalidZeroAmount {},

    #[error("Operation exceeds max spread limit")]
    MaxSpreadAssertion {},

    #[error("Provided spread amount exceeds allowed limit")]
    AllowedSpreadAssertion {},

    #[error("Operation exceeds max slippage tolerance")]
    MaxSlippageAssertion {},

    #[error("Doubling assets in asset infos")]
    DoublingAssets {},

    #[error("Asset mismatch between the requested and the stored asset in contract")]
    AssetMismatch {},

    #[error("InsufficientFunds")]
    InsufficientFunds {},

    #[error("pool can not cover reserves")]
    InsufficientReserves {},

    #[error("Incorrect bluechip denom: provided: {provided}, required: {required}")]
    IncorrectNativeDenom { provided: String, required: String },

    #[error("Invalid payment amount: ${usd_amount} USD. Available tiers: {available:?}")]
    InvalidUSDPaymentTier {
        usd_amount: String,
        available: Vec<String>,
    },
    #[error("The threshold lock is not stuck")]
    NotStuckYet {},

    #[error("Pool has been permanently drained via emergency withdrawal")]
    EmergencyDrained {},

    #[error("Emergency withdraw timelock not yet elapsed. Executable after: {effective_after}")]
    EmergencyTimelockPending { effective_after: Timestamp },

    #[error("No pending emergency withdrawal to cancel")]
    NoPendingEmergencyWithdraw {},
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
