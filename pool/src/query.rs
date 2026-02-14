#![allow(non_snake_case)]
use crate::asset::{call_pool_info, TokenInfo};
use crate::liquidity_helpers::calculate_unclaimed_fees;
use crate::msg::{
    CommitStatus, CommiterInfo, ConfigResponse, CumulativePricesResponse, FeeInfoResponse,
    LastCommitedResponse, PoolCommitResponse, PoolFeeStateResponse, PoolInfoResponse, PoolResponse,
    PoolStateResponse, PositionResponse, PositionsResponse, QueryMsg, ReverseSimulationResponse,
    SimulationResponse,
};
use crate::state::{
    PoolDetails, COMMITFEEINFO, COMMIT_LIMIT_INFO, IS_THRESHOLD_HIT, POOL_FEE_STATE,
    POOL_INFO, POOL_SPECS, POOL_STATE, USD_RAISED_FROM_COMMIT,
};
use crate::state::{COMMIT_INFO, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, OWNER_POSITIONS};
use crate::swap_helper::{compute_offer_amount, compute_swap, update_price_accumulator};
use cosmwasm_std::{
    entry_point, to_json_binary, Addr, Binary, Deps, Env, Order, StdError, StdResult, Uint128,
};
use pool_factory_interfaces::{AllPoolsResponse, PoolQueryMsg, PoolStateResponseForFactory};

use cw_storage_plus::Bound;
use std::vec;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::PoolState {} => to_json_binary(&query_pool_state(deps)?),
        QueryMsg::FeeState {} => to_json_binary(&query_fee_state(deps)?),
        QueryMsg::Position { position_id } => to_json_binary(&query_position(deps, position_id)?),
        QueryMsg::Positions { start_after, limit } => {
            to_json_binary(&query_positions(deps, start_after, limit)?)
        }
        QueryMsg::PoolCommits {
            pool_contract_address,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        } => to_json_binary(&query_pool_commiters(
            deps,
            pool_contract_address,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        )?),
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
        QueryMsg::LastCommited { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let response = match COMMIT_INFO.may_load(deps.storage, &addr)? {
                Some(commiting) => LastCommitedResponse {
                    has_commited: true,
                    last_commited: Some(commiting.last_commited),
                    last_payment_bluechip: Some(commiting.last_payment_bluechip),
                    last_payment_usd: Some(commiting.last_payment_usd),
                },
                None => LastCommitedResponse {
                    has_commited: false,
                    last_commited: None,
                    last_payment_bluechip: None,
                    last_payment_usd: None,
                },
            };
            to_json_binary(&response)
        }
        QueryMsg::CumulativePrices {} => to_json_binary(&query_cumulative_prices(deps, env)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),
        QueryMsg::IsFullyCommited {} => to_json_binary(&query_check_threshold_limit(deps)?),
        QueryMsg::CommitingInfo { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let info = COMMIT_INFO.may_load(deps.storage, &addr)?;
            to_json_binary(&info)
        }
    }
}

pub fn query_pair_info(deps: Deps) -> StdResult<PoolDetails> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pool_info)
}

pub fn query_pool(deps: Deps) -> StdResult<PoolResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info)?;

    let resp = PoolResponse { assets };

    Ok(resp)
}

pub fn query_check_threshold_limit(deps: Deps) -> StdResult<CommitStatus> {
    let threshold_hit = IS_THRESHOLD_HIT.load(deps.storage)?;
    let commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
    if threshold_hit {
        Ok(CommitStatus::FullyCommitted)
    } else {
        let usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
        Ok(CommitStatus::InProgress {
            raised: usd_raised,
            target: commit_config.commit_amount_for_threshold_usd,
        })
    }
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

    // Update the accumulator (this mutates pool_state)
    update_price_accumulator(&mut pool_state, env.block.time.seconds())
        .map_err(|e| StdError::generic_err(format!("Failed to update price accumulator: {}", e)))?;

    // Extract the updated cumulative prices
    let price0_cumulative_last = pool_state.price0_cumulative_last;
    let price1_cumulative_last = pool_state.price1_cumulative_last;

    let resp = CumulativePricesResponse {
        assets,
        price0_cumulative_last,
        price1_cumulative_last,
    };

    Ok(resp)
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

pub fn query_check_commit(deps: Deps) -> StdResult<bool> {
    if IS_THRESHOLD_HIT.load(deps.storage)? {
        return Ok(true);
    }
    let commit_info = COMMIT_LIMIT_INFO.load(deps.storage)?;
    let usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
    Ok(usd_raised >= commit_info.commit_amount_for_threshold_usd)
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
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?; // Since fees are in PoolState
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
    )?;
    let unclaimed_fees_1 = calculate_unclaimed_fees(
        liquidity_position.liquidity,
        liquidity_position.fee_growth_inside_1_last,
        pool_fee_state.fee_growth_global_1,
    )?;

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

/// H-5 FIX: Use secondary OWNER_POSITIONS index for efficient owner-based lookups
/// instead of scanning all positions and filtering client-side.
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

pub fn query_pool_commiters(
    deps: Deps,
    pool_contract_address: Addr,
    min_payment_usd: Option<Uint128>,
    after_timestamp: Option<u64>,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PoolCommitResponse> {
    let limit = limit.unwrap_or(30).min(100) as usize;

    // Create the bound - handle the lifetime properly
    let start_addr = start_after
        .map(|addr_str| deps.api.addr_validate(&addr_str))
        .transpose()?;

    let start = start_addr.as_ref().map(Bound::exclusive);

    let mut commiters = vec![];

    for item in COMMIT_INFO.range(deps.storage, start, None, Order::Ascending) {
        let (commiter_addr, commiting) = item?;

        // Filter by pool_contract_address
        if commiting.pool_contract_address != pool_contract_address {
            continue;
        }

        // Apply optional filters
        if let Some(min_usd) = min_payment_usd {
            if commiting.last_payment_usd < min_usd {
                continue;
            }
        }

        if let Some(after_ts) = after_timestamp {
            if commiting.last_commited.seconds() < after_ts {
                continue;
            }
        }

        commiters.push(CommiterInfo {
            wallet: commiter_addr.to_string(),
            last_payment_bluechip: commiting.last_payment_bluechip,
            last_payment_usd: commiting.last_payment_usd,
            last_commited: commiting.last_commited,
            total_paid_usd: commiting.total_paid_usd,
            total_paid_bluechip: commiting.total_paid_bluechip,
        });

        // Stop if we've collected enough
        if commiters.len() >= limit {
            break;
        }
    }

    // L-7 FIX: Use commiters.len() directly instead of redundant counter
    Ok(PoolCommitResponse {
        total_count: commiters.len() as u32,
        commiters,
    })
}

pub fn query_for_factory(deps: Deps, _env: Env, msg: PoolQueryMsg) -> StdResult<Binary> {
    match msg {
        PoolQueryMsg::GetPoolState {
            pool_contract_address: _, // Ignore address check for now, simply return self state
        } => {
            // Fix: Load from POOL_STATE (Item) instead of POOLS (Map which is never populated)
            let pool_state = POOL_STATE.load(deps.storage)?;

            // Load pool info to get assets
            let pool_info = POOL_INFO.load(deps.storage)?;

            // Map TokenType to String
            let assets: Vec<String> = pool_info
                .pool_info
                .asset_infos
                .iter()
                .map(|a| a.to_string())
                .collect();

            // Convert to response
            let response = PoolStateResponseForFactory {
                pool_contract_address: pool_state.pool_contract_address,
                nft_ownership_accepted: pool_state.nft_ownership_accepted,
                reserve0: pool_state.reserve0,
                reserve1: pool_state.reserve1,
                total_liquidity: pool_state.total_liquidity,
                block_time_last: pool_state.block_time_last,
                price0_cumulative_last: pool_state.price0_cumulative_last,
                price1_cumulative_last: pool_state.price1_cumulative_last,
                assets,
            };

            to_json_binary(&response)
        }
        PoolQueryMsg::GetAllPools {} => {
            // Fix: Return single pool state since this contract is a single pool instance
            let pool_state = POOL_STATE.load(deps.storage)?;
            let pool_info = POOL_INFO.load(deps.storage)?;
            let assets: Vec<String> = pool_info
                .pool_info
                .asset_infos
                .iter()
                .map(|a| a.to_string())
                .collect();

            let response = PoolStateResponseForFactory {
                pool_contract_address: pool_state.pool_contract_address.clone(),
                nft_ownership_accepted: pool_state.nft_ownership_accepted,
                reserve0: pool_state.reserve0,
                reserve1: pool_state.reserve1,
                total_liquidity: pool_state.total_liquidity,
                block_time_last: pool_state.block_time_last,
                price0_cumulative_last: pool_state.price0_cumulative_last,
                price1_cumulative_last: pool_state.price1_cumulative_last,
                assets,
            };

            // Return vector containing just this pool
            to_json_binary(&AllPoolsResponse {
                pools: vec![(pool_state.pool_contract_address.to_string(), response)],
            })
        }
    }
}
