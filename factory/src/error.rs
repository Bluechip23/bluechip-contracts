use thiserror::Error;

use cosmwasm_std::{StdError, Uint128};

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("InsufficientFunds")]
    InsufficientFunds {},

    #[error("This user already claimed for this day")]
    AlreadyClaimed {},

    #[error("Wrong configuration")]
    WrongConfiguration {},
}
