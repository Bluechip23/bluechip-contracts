use thiserror::Error;

use cosmwasm_std::StdError;

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

    #[error("Contract Address Can Not Be Found")]
    ContractAddressNotFound {},

     #[error("Contract Failed Creating Token {}", pool_id)]
    TokenCreationFailed {
        pool_id: u64,
        reason: String,
    },

    #[error("Contract Failed Creating  {}", id)]
    UnknownReplyId {
        id: u64,
    }

}
