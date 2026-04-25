use cosmwasm_std::{OverflowError, StdError, Uint128};
use thiserror::Error;

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
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
