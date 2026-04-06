//! Multi-hop execution logic.
//!
//! Phase 2 scaffolds the dispatch path; the real submessage chain that
//! atomically walks each hop is implemented in Phase 3. Until then, this
//! module exposes the entry function with its final signature so the
//! contract dispatcher in [`crate::contract`] can wire to it without
//! churn when Phase 3 lands.

use cosmwasm_std::{Decimal, DepsMut, Env, MessageInfo, Response, StdError, Timestamp, Uint128};
use pool_factory_interfaces::routing::SwapOperation;

use crate::error::RouterError;

/// Entry point for [`crate::msg::ExecuteMsg::ExecuteMultiHop`].
///
/// Phase 3 will replace this body with: validate route -> build per-hop
/// submessages with `ReplyOn::Error` -> verify slippage on the final hop
/// reply -> forward final output to the recipient.
#[allow(clippy::too_many_arguments)]
pub fn execute_multi_hop(
    _deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    _operations: Vec<SwapOperation>,
    _minimum_receive: Uint128,
    _belief_price: Option<Decimal>,
    _max_spread: Option<Decimal>,
    _deadline: Option<Timestamp>,
    _recipient: Option<String>,
) -> Result<Response, RouterError> {
    Err(RouterError::Std(StdError::generic_err(
        "ExecuteMultiHop is implemented in Phase 3",
    )))
}
