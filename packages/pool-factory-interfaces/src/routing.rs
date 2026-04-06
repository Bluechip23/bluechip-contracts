//! Shared types used by the multi-hop swap router.
//!
//! These types live in the interfaces crate (not the router or pool crate)
//! so that the router can build messages and decode responses for the pool
//! contract without depending on the entire pool crate. The JSON shape of
//! every type defined here is intentionally byte-identical to the matching
//! pool message or response so that messages serialized by the router are
//! accepted by the pool unmodified.

use crate::asset::{PoolPairType, TokenInfo, TokenType};
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};

/// One leg of a multi-hop swap route.
///
/// The caller of the router supplies an ordered list of these. Each entry
/// names the pool to be invoked plus the side of that pool's pair being
/// offered and the side expected back. The router uses these declarations
/// to chain hop outputs into hop inputs and to reject obviously malformed
/// routes (mismatched pairs, unknown tokens) before any funds move.
#[cw_serde]
pub struct SwapOperation {
    /// Address of the target pool contract for this hop.
    pub pool_addr: String,
    /// Token being sent into the pool for this hop.
    pub offer_asset_info: TokenType,
    /// Token expected back from the pool for this hop.
    pub ask_asset_info: TokenType,
}

/// Subset of the pool contract's `ExecuteMsg` that the router needs to send.
///
/// Only `SimpleSwap` is included because that is the only execute path the
/// router invokes for native (bluechip) hops. CW20 hops are routed through
/// the cw20 contract via [`PoolSwapCw20HookMsg`] inside a `Send` envelope,
/// so they do not appear here.
#[cw_serde]
pub enum PoolSwapExecuteMsg {
    SimpleSwap {
        offer_asset: TokenInfo,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
}

/// Subset of the pool's CW20 receive hook used by the router for CW20 hops.
///
/// The router wraps this inside a `cw20::Cw20ExecuteMsg::Send` whose
/// `contract` field is the target pool. The pool's `Receive` handler
/// dispatches on this enum to perform the swap.
#[cw_serde]
pub enum PoolSwapCw20HookMsg {
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
}

/// Subset of the pool's `QueryMsg` the router needs for route validation
/// and pre-trade simulation.
///
/// Kept narrow on purpose: the router never reads liquidity positions,
/// fee state, analytics, or commit history. Only the three queries below
/// are required to (a) validate that a hop's offer/ask align with the
/// pool's pair, (b) simulate the hop's output, and (c) reject routes that
/// touch a pool which is still in its pre-threshold commit phase.
#[cw_serde]
#[derive(QueryResponses)]
pub enum PoolSwapQueryMsg {
    /// Mirrors `pool::msg::QueryMsg::Simulation`.
    #[returns(RouterSwapSimulationResponse)]
    Simulation { offer_asset: TokenInfo },
    /// Mirrors `pool::msg::QueryMsg::Pair`.
    #[returns(RouterPoolPairInfo)]
    Pair {},
    /// Mirrors `pool::msg::QueryMsg::IsFullyCommited`.
    #[returns(RouterPoolCommitStatus)]
    IsFullyCommited {},
}

/// JSON-equivalent of `pool::msg::SimulationResponse`.
///
/// Defined separately so the router can decode pool simulation results
/// without pulling in the pool crate.
#[cw_serde]
pub struct RouterSwapSimulationResponse {
    pub return_amount: Uint128,
    pub spread_amount: Uint128,
    pub commission_amount: Uint128,
}

/// JSON-equivalent of `pool::asset::PoolPairInfo`.
///
/// The router only consults `asset_infos` to validate that a hop names a
/// valid (offer, ask) pair for the targeted pool, but the full struct is
/// mirrored to keep deserialization tolerant of additional fields the
/// pool returns.
#[cw_serde]
pub struct RouterPoolPairInfo {
    pub asset_infos: [TokenType; 2],
    pub contract_addr: Addr,
    pub pair_type: PoolPairType,
    pub assets: [TokenInfo; 2],
}

/// JSON-equivalent of `pool::msg::CommitStatus`.
///
/// Used by router queries and execution to refuse routes touching pools
/// whose commit phase has not yet completed. Such pools cannot be swapped
/// against, and silently returning zero would be misleading to callers.
#[cw_serde]
pub enum RouterPoolCommitStatus {
    InProgress { raised: Uint128, target: Uint128 },
    FullyCommitted,
}
