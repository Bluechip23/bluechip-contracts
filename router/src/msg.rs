//! External message surface for the router contract.
//!
//! The router exposes:
//! - `InstantiateMsg` for setup
//! - `ExecuteMsg::ExecuteMultiHop` for native-offered routes (the user
//! attaches bluechip funds with the call)
//! - `ExecuteMsg::Receive` for CW20-offered routes (the user calls
//! `cw20::Send` and the router decodes [`Cw20HookMsg`] from the body)
//! - `ExecuteMsg::UpdateConfig` for admin rotation
//! - Two internal variants (`ExecuteSwapOperation`, `AssertReceived`)
//! that the router invokes on itself; both reject any caller other
//! than the router contract address
//! - `QueryMsg::SimulateMultiHop` for pre-trade UX
//! - `QueryMsg::Config` for config reads

use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use cw20::Cw20ReceiveMsg;
use pool_factory_interfaces::asset::TokenType;
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
///
/// Note: end-to-end slippage is enforced via `minimum_receive` on the
/// final ask token, NOT via per-hop `belief_price` / `max_spread`. A
/// single per-pair belief price is meaningless across hops on heterogeneous
/// pairs (units differ between `A/bluechip` and `bluechip/B`), so the
/// router does not accept those parameters and always passes `None` for
/// them on the underlying pool calls. Frontends should size
/// `minimum_receive` from the simulation result (see `SimulateMultiHop`).
#[cw_serde]
pub enum ExecuteMsg {
    /// Run a multi-hop swap whose first hop offers the native bluechip
    /// denom. The caller attaches the offer amount via `info.funds`.
    /// The router does not perform on-chain pathfinding -- the caller
    /// supplies the entire route.
    ExecuteMultiHop {
        operations: Vec<SwapOperation>,
        minimum_receive: Uint128,
        deadline: Option<Timestamp>,
        recipient: Option<String>,
    },
    /// Admin-only. Step 1 of a 48h-timelocked config change. Records a
    /// `PendingConfigUpdate` with `effective_after = now + 48h`. Either
    /// field may be `None` to leave that field unchanged. Re-proposing
    /// while a pending proposal exists is rejected with
    /// `ConfigUpdateAlreadyPending` — the admin must `CancelConfigUpdate`
    /// first, so any community watcher polling `PENDING_CONFIG` sees an
    /// explicit cancellation event before a replacement proposal lands.
    ProposeConfigUpdate {
        admin: Option<String>,
        factory_addr: Option<String>,
    },
    /// Admin-only. Step 2 of the timelocked flow. Applies the pending
    /// proposal once `effective_after` has elapsed. Errors with
    /// `NoPendingConfigUpdate` if no proposal is pending or
    /// `TimelockNotExpired` if invoked too early.
    UpdateConfig {},
    /// Admin-only. Cancels a pending proposal before it can be applied.
    CancelConfigUpdate {},
    /// CW20 entry path: triggered when a user invokes `cw20::Send`
    /// targeting the router with [`Cw20HookMsg::ExecuteMultiHop`] in the
    /// body. Used when the first hop offers a creator token rather than
    /// native bluechip.
    Receive(Cw20ReceiveMsg),
    /// Internal: invoked by the router on itself once per hop. Each
    /// handler queries the router's current balance of the offer token
    /// and dispatches the underlying pool swap. Rejected unless the
    /// caller is the router contract.
    ExecuteSwapOperation {
        operation: SwapOperation,
        hop_index: u32,
        to: String,
    },
    /// Internal: final slippage assertion. Compares the recipient's
    /// post-route balance against the captured pre-route balance plus
    /// the minimum-receive threshold. Rejected unless the caller is the
    /// router contract.
    AssertReceived {
        ask_info: TokenType,
        recipient: String,
        prev_balance: Uint128,
        minimum_receive: Uint128,
    },
}

/// Body of `cw20::Send.msg` accepted by the router.
///
/// Mirrors the field set of [`ExecuteMsg::ExecuteMultiHop`] because the
/// only difference between the two entry paths is how the offer token
/// arrives at the router.
#[cw_serde]
pub enum Cw20HookMsg {
    ExecuteMultiHop {
        operations: Vec<SwapOperation>,
        minimum_receive: Uint128,
        deadline: Option<Timestamp>,
        recipient: Option<String>,
    },
}

/// Read-only entry points.
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    /// Pre-trade simulation that mirrors the execution path. Lets a
    /// frontend show the expected final amount, every intermediate
    /// amount, and a coarse price-impact estimate before signing.
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
/// `intermediate_amounts` contains the *output* of every hop in order,
/// so `intermediate_amounts.last()` always equals `final_amount`.
#[cw_serde]
pub struct SimulateMultiHopResponse {
    pub final_amount: Uint128,
    pub intermediate_amounts: Vec<Uint128>,
    pub price_impact: Decimal,
}
