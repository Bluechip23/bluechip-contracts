//! External message surface for the router contract.
//!
//! The router intentionally exposes a tiny API: instantiate, two execute
//! variants (multi-hop swap and admin config update), and two queries
//! (multi-hop simulation and config read). Any extra surface area would
//! invite bugs without adding routing capability that frontends/indexers
//! cannot already build on top of these primitives.

use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use pool_factory_interfaces::routing::SwapOperation;

/// Parameters for instantiating the router. The bluechip denom and
/// factory address pin the router to a single Bluechip deployment;
/// admin can be rotated later via [`ExecuteMsg::UpdateConfig`].
#[cw_serde]
pub struct InstantiateMsg {
    pub factory_addr: String,
    pub bluechip_denom: String,
    pub admin: String,
}

/// Mutating entry points.
#[cw_serde]
pub enum ExecuteMsg {
    /// Run a multi-hop swap. The caller supplies the entire route -- the
    /// router does not perform on-chain pathfinding. Each hop's output is
    /// fed into the next hop's input, and the final output must be at
    /// least `minimum_receive` or the whole transaction reverts.
    ExecuteMultiHop {
        operations: Vec<SwapOperation>,
        minimum_receive: Uint128,
        /// Optional belief price passed through to every hop's swap call.
        belief_price: Option<Decimal>,
        /// Optional max spread passed through to every hop's swap call.
        max_spread: Option<Decimal>,
        /// Hard deadline; if `Some` and the block time has passed it,
        /// the entire transaction is rejected before any swaps run.
        deadline: Option<Timestamp>,
        /// Optional final recipient. Defaults to the message sender.
        recipient: Option<String>,
    },
    /// Admin-only configuration update. Both fields are optional so the
    /// admin can rotate ownership independently of swapping the factory
    /// reference (e.g. after a factory migration).
    UpdateConfig {
        admin: Option<String>,
        factory_addr: Option<String>,
    },
}

/// Read-only entry points.
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    /// Pre-trade simulation that mirrors the execution path. Critical for
    /// frontend UX: lets users see the expected final amount, every
    /// intermediate amount, and a coarse price-impact estimate before
    /// signing a transaction.
    #[returns(SimulateMultiHopResponse)]
    SimulateMultiHop {
        operations: Vec<SwapOperation>,
        offer_amount: Uint128,
    },
    /// Returns the current router configuration.
    #[returns(ConfigResponse)]
    Config {},
}

/// Response for [`QueryMsg::Config`].
#[cw_serde]
pub struct ConfigResponse {
    pub factory_addr: Addr,
    pub bluechip_denom: String,
    pub admin: Addr,
}

/// Response for [`QueryMsg::SimulateMultiHop`].
///
/// `intermediate_amounts` contains the *output* of every hop in order, so
/// `intermediate_amounts.last()` always equals `final_amount`. Frontends
/// can use the per-hop values to render a route preview.
#[cw_serde]
pub struct SimulateMultiHopResponse {
    pub final_amount: Uint128,
    pub intermediate_amounts: Vec<Uint128>,
    pub price_impact: Decimal,
}
