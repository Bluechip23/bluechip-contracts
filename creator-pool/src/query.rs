//! Creator-pool query dispatch + commit-only query handlers. Shared
//! handlers live in `pool_core::query` and are re-exported so every
//! existing `use crate::query::X;` import resolves unchanged.
//!
//! `query_analytics` wraps pool-core's `query_analytics_core` by
//! loading commit-phase state (USD_RAISED_FROM_COMMIT,
//! NATIVE_RAISED_FROM_COMMIT) and deriving `threshold_status`.
//! Standard-pool's equivalent (Step 4b) passes `FullyCommitted` and
//! zeroes directly into `query_analytics_core`.
pub use pool_core::query::*;

use crate::msg::{
    CommitStatus, CommitterInfo, DistributionStateResponse, FactoryNotifyStatusResponse,
    LastCommittedResponse, PoolAnalyticsResponse, PoolCommitResponse, QueryMsg,
};
use crate::state::{
    COMMIT_INFO, COMMIT_LIMIT_INFO, DISTRIBUTION_STALL_TIMEOUT_SECONDS, DISTRIBUTION_STATE,
    IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, PENDING_FACTORY_NOTIFY,
    POOL_COMMITS_QUERY_DEFAULT_LIMIT, POOL_COMMITS_QUERY_MAX_LIMIT, USD_RAISED_FROM_COMMIT,
};
use cosmwasm_std::{
    entry_point, to_json_binary, Addr, Binary, Deps, Env, Order, StdResult, Uint128,
};
use cw_storage_plus::Bound;
use pool_factory_interfaces::PoolQueryMsg;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        // Shared
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
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::Simulation { offer_asset } => {
            to_json_binary(&query_simulation(deps, offer_asset)?)
        }
        QueryMsg::ReverseSimulation { ask_asset } => {
            to_json_binary(&query_reverse_simulation(deps, ask_asset)?)
        }
        QueryMsg::CumulativePrices {} => to_json_binary(&query_cumulative_prices(deps, env)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),
        QueryMsg::GetPoolState {} => {
            query_for_factory(deps, env, PoolQueryMsg::GetPoolState {})
        }
        QueryMsg::GetAllPools {} => query_for_factory(deps, env, PoolQueryMsg::GetAllPools {}),
        QueryMsg::IsPaused {} => query_for_factory(deps, env, PoolQueryMsg::IsPaused {}),

        // Commit-only (creator-pool)
        QueryMsg::IsFullyCommited {} => to_json_binary(&query_check_threshold_limit(deps)?),
        QueryMsg::CommittingInfo { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let info = COMMIT_INFO.may_load(deps.storage, &addr)?;
            to_json_binary(&info)
        }
        QueryMsg::LastCommited { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let response = match COMMIT_INFO.may_load(deps.storage, &addr)? {
                Some(committing) => LastCommittedResponse {
                    has_committed: true,
                    last_committed: Some(committing.last_committed),
                    last_payment_bluechip: Some(committing.last_payment_bluechip),
                    last_payment_usd: Some(committing.last_payment_usd),
                },
                None => LastCommittedResponse {
                    has_committed: false,
                    last_committed: None,
                    last_payment_bluechip: None,
                    last_payment_usd: None,
                },
            };
            to_json_binary(&response)
        }
        QueryMsg::PoolCommits {
            pool_contract_address,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        } => to_json_binary(&query_pool_committers(
            deps,
            pool_contract_address,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        )?),
        QueryMsg::FactoryNotifyStatus {} => to_json_binary(&query_factory_notify_status(deps)?),
        QueryMsg::DistributionState {} => to_json_binary(&query_distribution_state(deps, &env)?),

        // Hybrid — wrapper computes creator-only pieces, pool-core assembles
        QueryMsg::Analytics {} => to_json_binary(&query_analytics(deps)?),
    }
}

pub fn query_factory_notify_status(deps: Deps) -> StdResult<FactoryNotifyStatusResponse> {
    let pending = PENDING_FACTORY_NOTIFY
        .may_load(deps.storage)?
        .unwrap_or(false);
    Ok(FactoryNotifyStatusResponse { pending })
}

/// Returns `None` when no distribution is in progress (pre-threshold or
/// fully completed and cleaned up). Returns `Some(...)` with the live
/// `DistributionState` plus `seconds_since_update` and `is_stalled`
/// computed against `DISTRIBUTION_STALL_TIMEOUT_SECONDS`. Replaces the
/// previously-discarded `consecutive_failures = 99` marker as the
/// observable stall signal — this query is what admin dashboards should
/// poll to detect "this pool needs RecoverPoolStuckStates."
pub fn query_distribution_state(
    deps: Deps,
    env: &Env,
) -> StdResult<Option<DistributionStateResponse>> {
    let Some(state) = DISTRIBUTION_STATE.may_load(deps.storage)? else {
        return Ok(None);
    };
    let seconds_since_update = env
        .block
        .time
        .seconds()
        .saturating_sub(state.last_updated.seconds());
    let is_stalled = seconds_since_update > DISTRIBUTION_STALL_TIMEOUT_SECONDS;
    Ok(Some(DistributionStateResponse {
        is_distributing: state.is_distributing,
        distributions_remaining: state.distributions_remaining,
        last_processed_key: state.last_processed_key,
        started_at: state.started_at,
        last_updated: state.last_updated,
        seconds_since_update,
        is_stalled,
        consecutive_failures: state.consecutive_failures,
        total_to_distribute: state.total_to_distribute,
        total_committed_usd: state.total_committed_usd,
    }))
}

/// Public threshold-status helper that takes an already-loaded
/// `usd_raised`. Callers that already loaded `USD_RAISED_FROM_COMMIT`
/// for their own response (e.g. `query_analytics`) call this directly
/// to skip one redundant storage read; standalone callers go through
/// `query_check_threshold_limit` which performs the load.
pub fn threshold_status_from(deps: Deps, usd_raised: Uint128) -> StdResult<CommitStatus> {
    let threshold_hit = IS_THRESHOLD_HIT.load(deps.storage)?;
    if threshold_hit {
        Ok(CommitStatus::FullyCommitted)
    } else {
        let commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
        Ok(CommitStatus::InProgress {
            raised: usd_raised,
            target: commit_config.commit_amount_for_threshold_usd,
        })
    }
}

/// Standalone wrapper around [`threshold_status_from`] that loads
/// `USD_RAISED_FROM_COMMIT` itself. Use when you don't already have
/// the raised total in scope.
pub fn query_check_threshold_limit(deps: Deps) -> StdResult<CommitStatus> {
    let usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
    threshold_status_from(deps, usd_raised)
}

/// Creator-pool wrapper around `query_analytics_core`. Loads commit-
/// phase totals and derives `threshold_status`, then delegates the
/// shared response body construction.
pub fn query_analytics(deps: Deps) -> StdResult<PoolAnalyticsResponse> {
    let usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
    let bluechip_raised = NATIVE_RAISED_FROM_COMMIT.load(deps.storage)?;
    let threshold_status = threshold_status_from(deps, usd_raised)?;
    query_analytics_core(deps, threshold_status, usd_raised, bluechip_raised)
}

pub fn query_pool_committers(
    deps: Deps,
    pool_contract_address: Addr,
    min_payment_usd: Option<Uint128>,
    after_timestamp: Option<u64>,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PoolCommitResponse> {
    let limit = limit
        .unwrap_or(POOL_COMMITS_QUERY_DEFAULT_LIMIT)
        .min(POOL_COMMITS_QUERY_MAX_LIMIT) as usize;

    let start_addr = start_after
        .map(|addr_str| deps.api.addr_validate(&addr_str))
        .transpose()?;
    let start = start_addr.as_ref().map(Bound::exclusive);

    let committers: StdResult<Vec<CommitterInfo>> = COMMIT_INFO
        .range(deps.storage, start, None, Order::Ascending)
        .filter_map(|item| {
            let (committer_addr, committing) = match item {
                Ok(kv) => kv,
                Err(e) => return Some(Err(e)),
            };
            if committing.pool_contract_address != pool_contract_address {
                return None;
            }
            if let Some(min_usd) = min_payment_usd {
                if committing.last_payment_usd < min_usd {
                    return None;
                }
            }
            if let Some(after_ts) = after_timestamp {
                if committing.last_committed.seconds() < after_ts {
                    return None;
                }
            }
            Some(Ok(CommitterInfo {
                wallet: committer_addr.to_string(),
                last_payment_bluechip: committing.last_payment_bluechip,
                last_payment_usd: committing.last_payment_usd,
                last_committed: committing.last_committed,
                total_paid_usd: committing.total_paid_usd,
                total_paid_bluechip: committing.total_paid_bluechip,
            }))
        })
        .take(limit)
        .collect();
    let committers = committers?;

    Ok(PoolCommitResponse {
        page_count: committers.len() as u32,
        committers,
    })
}
