//! Multi-hop simulation logic.
//!
//! Phase 2 scaffolds the dispatch path; Phase 4 implements the chained
//! pool simulation queries and price-impact computation. Keeping this in
//! its own module from the start lets the contract dispatcher and the
//! Phase 4 implementation evolve independently of unrelated Phase 3
//! execution work.

use cosmwasm_std::{Deps, StdError, Uint128};
use pool_factory_interfaces::routing::SwapOperation;

use crate::error::RouterError;
use crate::msg::SimulateMultiHopResponse;

/// Entry point for [`crate::msg::QueryMsg::SimulateMultiHop`].
///
/// Phase 4 will replace this body with: walk each hop, query the pool's
/// `Simulation`, chain the output, and return all intermediate amounts
/// alongside the final receive amount and a coarse price impact value.
pub fn simulate_multi_hop(
    _deps: Deps,
    _operations: Vec<SwapOperation>,
    _offer_amount: Uint128,
) -> Result<SimulateMultiHopResponse, RouterError> {
    Err(RouterError::Std(StdError::generic_err(
        "SimulateMultiHop is implemented in Phase 4",
    )))
}
