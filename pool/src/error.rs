use cosmwasm_std::{OverflowError, StdError, Uint128};
use thiserror::Error;

pub const MINIMUM_LIQUIDITY_AMOUNT: Uint128 = Uint128::new(1_000);
/// ## Description
/// This enum describes pair contract errors!
#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Reentrancy detected")]
    ReentrancyGuard {},

    #[error("You can not swap until the threshold is crossed. You must subscribe to transact with this pool")]
    ShortOfThreshold {},

    #[error("You are trying to commit too frequently.")]
    TooFrequentCommits { wait_time: u64 },

    #[error("Division by zero error")]
    DivideByZero,

    #[error("percent must be between 1 and 99")]
    InvalidPercent,

    #[error("Zero won't work.")]
    ZeroAmount {},

    #[error(
        "Your subscription amount does not match an amount designated by the creator of the pool."
    )]
    MismatchAmount {},

    #[error("Fee is to great or to small for this transaction")]
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

    #[error("invalid bluechip amount")]
    InvalidNativeAmount {},

    #[error("the pool is missing needed liquidity to carry out transaction")]
    InsufficientLiquidity {},

    #[error("Unauthorized: Only creator can perform this action")]
    UnauthorizedNotCreator {},

    #[error("Invalid payment tiers: cannot be empty")]
    InvalidPaymentTiers {},

    #[error("CW20 tokens can be swapped via Cw20::Send message only")]
    Cw20DirectSwap {},

    #[error("Operation non supported")]
    NonSupported {},

    #[error("Event of zero transfer")]
    InvalidZeroAmount {},

    #[error("Operation exceeds max spread limit")]
    MaxSpreadAssertion {},

    #[error("Provided spread amount exceeds allowed limit")]
    AllowedSpreadAssertion {},

    #[error("Operation exceeds max splippage tolerance")]
    MaxSlippageAssertion {},

    #[error("Doubling assets in asset infos")]
    DoublingAssets {},

    #[error("Asset mismatch between the requested and the stored asset in contract")]
    AssetMismatch {},

    #[error("Pair type mismatch. Check factory pair configs")]
    PairTypeMismatch {},

    #[error("Generator address is not set in factory. Cannot auto-stake")]
    AutoStakeError {},

    #[error("Initial liquidity must be more than {}", MINIMUM_LIQUIDITY_AMOUNT)]
    MinimumLiquidityAmountError {},

    #[error("Failed to migrate the contract")]
    MigrationError {},

    #[error("Cannot migrate from different contract type: {previous_contract}")]
    CannotMigrate { previous_contract: String },

    #[error("InsufficientFunds")]
    InsufficientFunds {},

    #[error("Incorrect native denom: provided: {provided}, required: {required}")]
    IncorrectNativeDenom { provided: String, required: String },

    #[error("Invalid payment amount: ${usd_amount} USD. Available tiers: {available:?}")]
    InvalidUSDPaymentTier {
        usd_amount: String,
        available: Vec<String>,
    },

    #[error("Invalid payment amount. Sent: {sent_native} (${sent_usd}). Available native tiers: {available_native:?}. Available USD tiers: {available_usd:?}")]
    InvalidPaymentAmount {
        sent_native: String,
        sent_usd: String,
        available_native: Vec<String>,
        available_usd: Vec<String>,
    },
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
