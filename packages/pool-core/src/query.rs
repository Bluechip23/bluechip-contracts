//! Shared query handlers.
//!
//! Every reader of shared state moves here; commit-only queries
//! (`query_check_threshold_limit`, `query_pool_committers`,
//! `query_factory_notify_status`) and the top-level `pub fn query`
//! dispatch stay per-contract in creator-pool / standard-pool.
//!
//! `query_analytics` is factored: `query_analytics_core` assembles the
//! shared-state portion of `PoolAnalyticsResponse`; each contract's
//! wrapper supplies the commit-adjacent fields (`threshold_status`,
//! `total_usd_raised`, `total_bluechip_raised`). Creator-pool loads
//! commit ledger state; standard-pool passes `FullyCommitted` and zero.

use crate::asset::{call_pool_info, TokenInfo};
use crate::liquidity_helpers::calculate_unclaimed_fees;
use crate::msg::{
    CommitStatus, ConfigResponse, CumulativePricesResponse, FeeInfoResponse, PoolAnalyticsResponse,
    PoolFeeStateResponse, PoolInfoResponse, PoolStateResponse, PositionResponse, PositionsResponse,
    ReverseSimulationResponse, SimulationResponse,
};
use crate::state::{
    PoolDetails, COMMITFEEINFO, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS, NEXT_POSITION_ID,
    OWNER_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE,
};
use crate::swap::{compute_offer_amount, compute_swap, update_price_accumulator};
use cosmwasm_std::{
    to_json_binary, Binary, Decimal, Deps, Env, Order, StdError, StdResult, Uint128,
};
use cw_storage_plus::Bound;
use pool_factory_interfaces::{
    AllPoolsResponse, IsPausedResponse, PoolQueryMsg, PoolStateResponseForFactory,
};

pub fn query_is_paused(deps: Deps) -> StdResult<IsPausedResponse> {
    let paused = POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false);
    Ok(IsPausedResponse { paused })
}

pub fn query_pair_info(deps: Deps) -> StdResult<PoolDetails> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pool_info)
}

pub fn query_simulation(deps: Deps, offer_asset: TokenInfo) -> StdResult<SimulationResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let contract_addr = pool_info.pool_info.contract_addr.clone();

    let pools: [TokenInfo; 2] = pool_info
        .pool_info
        .query_pools(&deps.querier, contract_addr)?;

    let offer_pool: TokenInfo;
    let ask_pool: TokenInfo;
    if offer_asset.info.equal(&pools[0].info) {
        offer_pool = pools[0].clone();
        ask_pool = pools[1].clone();
    } else if offer_asset.info.equal(&pools[1].info) {
        offer_pool = pools[1].clone();
        ask_pool = pools[0].clone();
    } else {
        return Err(StdError::generic_err(
            "Given offer asset does not belong in the pair",
        ));
    }

    let (return_amount, spread_amount, commission_amount) = compute_swap(
        offer_pool.amount,
        ask_pool.amount,
        offer_asset.amount,
        pool_specs.lp_fee,
    )?;

    Ok(SimulationResponse {
        return_amount,
        spread_amount,
        commission_amount,
    })
}

pub fn query_reverse_simulation(
    deps: Deps,
    ask_asset: TokenInfo,
) -> StdResult<ReverseSimulationResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let contract_addr = pool_info.pool_info.contract_addr.clone();

    let pools: [TokenInfo; 2] = pool_info
        .pool_info
        .query_pools(&deps.querier, contract_addr)?;

    let offer_pool: TokenInfo;
    let ask_pool: TokenInfo;
    if ask_asset.info.equal(&pools[0].info) {
        ask_pool = pools[0].clone();
        offer_pool = pools[1].clone();
    } else if ask_asset.info.equal(&pools[1].info) {
        ask_pool = pools[1].clone();
        offer_pool = pools[0].clone();
    } else {
        return Err(StdError::generic_err(
            "Given ask asset doesn't belong to pairs",
        ));
    }

    let (offer_amount, spread_amount, commission_amount) = compute_offer_amount(
        offer_pool.amount,
        ask_pool.amount,
        ask_asset.amount,
        pool_specs.lp_fee,
    )?;

    Ok(ReverseSimulationResponse {
        offer_amount,
        spread_amount,
        commission_amount,
    })
}

pub fn query_cumulative_prices(deps: Deps, env: Env) -> StdResult<CumulativePricesResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info.clone())?;

    pool_state.reserve0 = assets[0].amount;
    pool_state.reserve1 = assets[1].amount;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())
        .map_err(|e| StdError::generic_err(format!("Failed to update price accumulator: {}", e)))?;

    let price0_cumulative_last = pool_state.price0_cumulative_last;
    let price1_cumulative_last = pool_state.price1_cumulative_last;

    Ok(CumulativePricesResponse {
        assets,
        price0_cumulative_last,
        price1_cumulative_last,
    })
}

pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(ConfigResponse {
        block_time_last: pool_state.block_time_last,
        params: None,
    })
}

pub fn query_fee_info(deps: Deps) -> StdResult<FeeInfoResponse> {
    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    Ok(FeeInfoResponse { fee_info })
}

/// Returns true only after the threshold crossing has fully completed
/// (IS_THRESHOLD_HIT == true). Gates all post-threshold operations.
/// Standard pools set IS_THRESHOLD_HIT to true at instantiate, so this
/// always returns true for them.
pub fn query_check_commit(deps: Deps) -> StdResult<bool> {
    IS_THRESHOLD_HIT.load(deps.storage)
}

pub fn query_pool_state(deps: Deps) -> StdResult<PoolStateResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(PoolStateResponse {
        nft_ownership_accepted: pool_state.nft_ownership_accepted,
        reserve0: pool_state.reserve0,
        reserve1: pool_state.reserve1,
        total_liquidity: pool_state.total_liquidity,
        block_time_last: pool_state.block_time_last,
    })
}

pub fn query_fee_state(deps: Deps) -> StdResult<PoolFeeStateResponse> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    Ok(PoolFeeStateResponse {
        fee_growth_global_0: pool_fee_state.fee_growth_global_0,
        fee_growth_global_1: pool_fee_state.fee_growth_global_1,
        total_fees_collected_0: pool_fee_state.total_fees_collected_0,
        total_fees_collected_1: pool_fee_state.total_fees_collected_1,
    })
}

pub fn query_position(deps: Deps, position_id: String) -> StdResult<PositionResponse> {
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let unclaimed_fees_0 = calculate_unclaimed_fees(
        liquidity_position.liquidity,
        liquidity_position.fee_growth_inside_0_last,
        pool_fee_state.fee_growth_global_0,
    )?
    .checked_add(liquidity_position.unclaimed_fees_0)?;
    let unclaimed_fees_1 = calculate_unclaimed_fees(
        liquidity_position.liquidity,
        liquidity_position.fee_growth_inside_1_last,
        pool_fee_state.fee_growth_global_1,
    )?
    .checked_add(liquidity_position.unclaimed_fees_1)?;

    Ok(PositionResponse {
        position_id,
        liquidity: liquidity_position.liquidity,
        owner: liquidity_position.owner,
        fee_growth_inside_0_last: liquidity_position.fee_growth_inside_0_last,
        fee_growth_inside_1_last: liquidity_position.fee_growth_inside_1_last,
        created_at: liquidity_position.created_at,
        last_fee_collection: liquidity_position.last_fee_collection,
        unclaimed_fees_0,
        unclaimed_fees_1,
    })
}

pub fn query_positions(
    deps: Deps,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PositionsResponse> {
    let limit = limit.unwrap_or(10).min(30) as usize;
    let start = start_after.as_ref().map(|s| Bound::exclusive(s.as_str()));

    let liquidity_positions: StdResult<Vec<_>> = LIQUIDITY_POSITIONS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .map(|item| {
            let (position_id, _position) = item?;
            query_position(deps, position_id)
        })
        .collect();

    Ok(PositionsResponse {
        positions: liquidity_positions?,
    })
}

pub fn query_positions_by_owner(
    deps: Deps,
    owner: String,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PositionsResponse> {
    let owner_addr = deps.api.addr_validate(&owner)?;
    let limit = limit.unwrap_or(10).min(30) as usize;
    let start = start_after
        .as_ref()
        .map(|s| Bound::<&str>::exclusive(s.as_str()));

    let positions: StdResult<Vec<_>> = OWNER_POSITIONS
        .prefix(&owner_addr)
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .map(|item| {
            let (position_id, _) = item?;
            query_position(deps, position_id)
        })
        .collect();

    Ok(PositionsResponse {
        positions: positions?,
    })
}

pub fn query_pool_info(deps: Deps) -> StdResult<PoolInfoResponse> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let next_position_id = NEXT_POSITION_ID.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;

    Ok(PoolInfoResponse {
        pool_state: PoolStateResponse {
            nft_ownership_accepted: pool_state.nft_ownership_accepted,
            reserve0: pool_state.reserve0,
            reserve1: pool_state.reserve1,
            total_liquidity: pool_state.total_liquidity,
            block_time_last: pool_state.block_time_last,
        },
        fee_state: PoolFeeStateResponse {
            fee_growth_global_0: pool_fee_state.fee_growth_global_0,
            fee_growth_global_1: pool_fee_state.fee_growth_global_1,
            total_fees_collected_0: pool_fee_state.total_fees_collected_0,
            total_fees_collected_1: pool_fee_state.total_fees_collected_1,
        },
        total_positions: next_position_id,
    })
}

/// Assembles the parts of `PoolAnalyticsResponse` that don't depend on
/// commit-phase state. Each contract supplies the commit-adjacent
/// fields (`threshold_status`, `total_usd_raised`, `total_bluechip_raised`)
/// from whatever state it has access to.
pub fn query_analytics_core(
    deps: Deps,
    threshold_status: CommitStatus,
    total_usd_raised: Uint128,
    total_bluechip_raised: Uint128,
) -> StdResult<PoolAnalyticsResponse> {
    let analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    let pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let next_position_id = NEXT_POSITION_ID.load(deps.storage)?;

    let current_price_0_to_1 = if !pool_state.reserve0.is_zero() {
        Decimal::from_ratio(pool_state.reserve1, pool_state.reserve0).to_string()
    } else {
        "0".to_string()
    };
    let current_price_1_to_0 = if !pool_state.reserve1.is_zero() {
        Decimal::from_ratio(pool_state.reserve0, pool_state.reserve1).to_string()
    } else {
        "0".to_string()
    };

    Ok(PoolAnalyticsResponse {
        analytics,
        current_price_0_to_1,
        current_price_1_to_0,
        total_value_locked_0: pool_state.reserve0,
        total_value_locked_1: pool_state.reserve1,
        fee_reserve_0: pool_fee_state.fee_reserve_0,
        fee_reserve_1: pool_fee_state.fee_reserve_1,
        threshold_status,
        total_usd_raised,
        total_bluechip_raised,
        total_positions: next_position_id,
    })
}

/// Build the factory response struct from current pool state.
fn build_factory_response(deps: Deps) -> StdResult<PoolStateResponseForFactory> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets: Vec<String> = pool_info
        .pool_info
        .asset_infos
        .iter()
        .map(|a| a.to_string())
        .collect();

    Ok(PoolStateResponseForFactory {
        pool_contract_address: pool_state.pool_contract_address,
        nft_ownership_accepted: pool_state.nft_ownership_accepted,
        reserve0: pool_state.reserve0,
        reserve1: pool_state.reserve1,
        total_liquidity: pool_state.total_liquidity,
        block_time_last: pool_state.block_time_last,
        price0_cumulative_last: pool_state.price0_cumulative_last,
        price1_cumulative_last: pool_state.price1_cumulative_last,
        assets,
    })
}

pub fn query_for_factory(deps: Deps, _env: Env, msg: PoolQueryMsg) -> StdResult<Binary> {
    match msg {
        PoolQueryMsg::GetPoolState {
            pool_contract_address: _,
        } => to_json_binary(&build_factory_response(deps)?),
        PoolQueryMsg::GetAllPools {} => {
            let response = build_factory_response(deps)?;
            to_json_binary(&AllPoolsResponse {
                pools: vec![(response.pool_contract_address.to_string(), response)],
            })
        }
        PoolQueryMsg::IsPaused {} => to_json_binary(&query_is_paused(deps)?),
    }
}
