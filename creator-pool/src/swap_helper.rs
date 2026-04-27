//! Oracle-integration helpers (commit-phase only). The pure AMM math
//! that used to live in this file (`compute_swap`, `compute_offer_amount`,
//! `assert_max_spread`, `update_price_accumulator`, `DEFAULT_SLIPPAGE`)
//! now lives in `pool_core::swap` and is re-exported below so existing
//! imports like `use crate::swap_helper::compute_swap;` keep resolving.
pub use pool_core::swap::*;

use crate::state::POOL_INFO;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Deps, StdError, StdResult, Uint128};
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};

#[cw_serde]
enum FactoryQueryWrapper {
    InternalBlueChipOracleQuery(FactoryQueryMsg),
}

// Pool-side acceptance window for the factory oracle's `ConversionResponse`.
// Sized to the factory's oracle update interval (`UPDATE_INTERVAL = 300s`,
// the *minimum* gap between successive `UpdateOraclePrice` calls — keepers
// physically cannot refresh sooner) plus a 60s grace buffer for keeper
// scheduling jitter. The factory's `convert_with_oracle` returns
// `ConversionResponse.timestamp = bluechip_price_cache.last_update`, which
// only advances when a keeper successfully refreshes the cache. With a
// strict 90s window here against a 300s update cadence, ~70% of every
// 5-minute cycle would reject every commit with "Oracle price is stale"
// even on a fully healthy system.
//
// The acceptable Pyth staleness is enforced separately on the factory side
// via `MAX_PRICE_AGE_SECONDS_BEFORE_STALE`; that check guards the upstream
// price feed. This pool-side check guards the cache-read freshness, which
// is a strict superset of the same age in the worst case.
pub const MAX_ORACLE_STALENESS_SECONDS: u64 = 360;

/// Must match factory::internal_bluechip_price_oracle::PRICE_PRECISION.
/// Duplicated here rather than imported because the pool crate intentionally
/// has no compile-time dependency on the factory crate — they communicate
/// only over wasm message boundaries. Any future change to factory-side
/// PRICE_PRECISION must be mirrored here.
pub const ORACLE_PRICE_PRECISION: u128 = 1_000_000;

/// Performs the oracle-backed bluechip→USD conversion and returns the
/// full ConversionResponse (not just the amount). Callers that need to
/// subsequently convert USD back to bluechip can derive the second value
/// from the `rate_used` field without re-querying — see P4-M6. Threads
/// the same price snapshot through the entire commit flow, so every
/// commit path issues at most one oracle query (verified across
/// `execute_commit_logic`, `process_threshold_crossing_with_excess`,
/// `process_pre_threshold_commit`, `process_post_threshold_commit`).
pub fn get_oracle_conversion_with_staleness(
    deps: Deps,
    bluechip_amount: Uint128,
    current_block_time: u64,
) -> StdResult<ConversionResponse> {
    let factory_address = POOL_INFO.load(deps.storage)?;

    let response: ConversionResponse = deps.querier.query_wasm_smart(
        factory_address.factory_addr.clone(),
        &FactoryQueryWrapper::InternalBlueChipOracleQuery(FactoryQueryMsg::ConvertBluechipToUsd {
            amount: bluechip_amount,
        }),
    )?;

    if response.timestamp > 0
        && current_block_time > response.timestamp + MAX_ORACLE_STALENESS_SECONDS
    {
        return Err(StdError::generic_err(format!(
            "Oracle price is stale: last updated at {}, current time {}, max age {}s",
            response.timestamp, current_block_time, MAX_ORACLE_STALENESS_SECONDS
        )));
    }

    Ok(response)
}

/// USD -> bluechip using an already-captured oracle rate. Mirrors the
/// factory's convert_with_oracle math: bluechip = usd * PRICE_PRECISION / rate.
/// Used inside a single commit to make sure the USD-to-threshold
/// conversion uses EXACTLY the same rate as the entry-point USD
/// valuation, so no mid-tx drift is possible even if the factory's
/// cached price were to change between sub-calls in a future refactor.
pub fn usd_to_bluechip_at_rate(usd_amount: Uint128, rate: Uint128) -> StdResult<Uint128> {
    if rate.is_zero() {
        return Err(StdError::generic_err(
            "Cannot convert USD to bluechip: oracle rate is zero",
        ));
    }
    usd_amount
        .checked_mul(Uint128::from(ORACLE_PRICE_PRECISION))
        .map_err(|e| StdError::generic_err(format!("Overflow converting USD to bluechip: {}", e)))?
        .checked_div(rate)
        .map_err(|e| {
            StdError::generic_err(format!("Division error converting USD to bluechip: {}", e))
        })
}

