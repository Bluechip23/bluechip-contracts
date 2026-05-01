use cosmwasm_std::{OverflowError, StdError, Uint128};
use cw_utils::PaymentError;
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

    /// Any execute path on this contract is non-payable. Surfaces
    /// `cw_utils::nonpayable` rejections directly so the underlying
    /// `PaymentError::NonPayable {}` reason ("This message does no accept
    /// funds") is preserved for clients.
    #[error("{0}")]
    Payment(#[from] PaymentError),
}

impl From<OverflowError> for ContractError {
    fn from(o: OverflowError) -> Self {
        StdError::from(o).into()
    }
}
