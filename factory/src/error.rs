use cosmwasm_std::{OverflowError, StdError, Timestamp};
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
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
