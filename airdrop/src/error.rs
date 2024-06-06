use cosmwasm_std::StdError;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Insufficient funds to cover all rewards")]
    InsufficientFunds {},

    #[error("Tokens already claimed")]
    AlreadyClaimed {},

    #[error("No rewards found for address")]
    NoRewards {},
}
