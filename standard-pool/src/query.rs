//! Standard-pool query dispatch. Every variant forwards to a shared
//! handler in `pool_core::query`. `Analytics` uses `query_analytics_core`
//! with `FullyCommitted` + zero raised (standard pools have no commit
//! ledger).

use crate::msg::QueryMsg;
use cosmwasm_std::{entry_point, to_json_binary, Binary, Deps, Env, StdResult, Uint128};
use pool_core::msg::{CommitStatus, PoolAnalyticsResponse};
use pool_core::query::{
    query_analytics_core, query_config, query_cumulative_prices, query_fee_info, query_fee_state,
    query_for_factory, query_pair_info, query_pool_info, query_pool_state, query_position,
    query_positions, query_positions_by_owner, query_reverse_simulation, query_simulation,
};
use pool_factory_interfaces::PoolQueryMsg;

/// Query dispatcher.
///
/// Three rough variant groups:
///
/// - **LP / SDK shapes**: `Pair`, `Config`, `Simulation`,
/// `ReverseSimulation`, `CumulativePrices`, `FeeInfo`, `PoolState`,
/// `FeeState`, `Position*`, `PoolInfo`, `Analytics`. Each
/// forwards to a shared `pool_core::query::*` handler. The
/// `PoolState {}` shape (`PoolStateResponse`) is the LP-side type;
/// `GetPoolState {}` below is a different shape for factory use.
///
/// - **Factory-facing shapes**: `GetPoolState`, `GetAllPools`,
/// `IsPaused`. These return types defined in
/// `pool_factory_interfaces` and are routed through
/// `query_for_factory`. Frontends should use the LP-shape variants
/// above instead.
///
/// - **Standard-pool-only**: `Analytics`. Standard pools have no
/// commit ledger, so `query_analytics` always reports
/// `FullyCommitted` with zero raised on both sides.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::Simulation { offer_asset } => {
            to_json_binary(&query_simulation(deps, offer_asset)?)
        }
        QueryMsg::ReverseSimulation { ask_asset } => {
            to_json_binary(&query_reverse_simulation(deps, ask_asset)?)
        }
        QueryMsg::CumulativePrices {} => to_json_binary(&query_cumulative_prices(deps, env)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),
        QueryMsg::PoolState {} => to_json_binary(&query_pool_state(deps)?),
        QueryMsg::FeeState {} => to_json_binary(&query_fee_state(deps)?),
        QueryMsg::Position { position_id } => to_json_binary(&query_position(deps, position_id)?),
        QueryMsg::Positions { start_after, limit } => {
            to_json_binary(&query_positions(deps, start_after, limit)?)
        }
        QueryMsg::PositionsByOwner {
            owner,
            start_after,
            limit,
        } => to_json_binary(&query_positions_by_owner(deps, owner, start_after, limit)?),
        QueryMsg::PoolInfo {} => to_json_binary(&query_pool_info(deps)?),
        QueryMsg::Analytics {} => to_json_binary(&query_analytics(deps)?),
        QueryMsg::GetPoolState {} => {
            query_for_factory(deps, env, PoolQueryMsg::GetPoolState {})
        }
        QueryMsg::GetAllPools {} => query_for_factory(deps, env, PoolQueryMsg::GetAllPools {}),
        QueryMsg::IsPaused {} => query_for_factory(deps, env, PoolQueryMsg::IsPaused {}),
    }
}

/// Standard-pool analytics wrapper: no commit ledger, no threshold —
/// always FullyCommitted, zero raised on both sides.
fn query_analytics(deps: Deps) -> StdResult<PoolAnalyticsResponse> {
    query_analytics_core(
        deps,
        CommitStatus::FullyCommitted,
        Uint128::zero(),
        Uint128::zero(),
    )
}
