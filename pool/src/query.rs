#![allow(non_snake_case)]
use crate::asset::{call_pool_info,};
use crate::msg::{
    CommitStatus, CommiterInfo, ConfigResponse, CumulativePricesResponse, FeeInfoResponse,
    LastCommitedResponse, PoolCommitResponse, PoolFeeStateResponse, PoolInfoResponse, PoolResponse,
    PoolStateResponse, QueryMsg, 
};

use crate::state::{
    PairInfo, COMMITSTATUS, COMMIT_CONFIG, FEEINFO, POOL_FEE_STATE, POOL_INFO, POOL_STATE,
    THRESHOLD_HIT, USD_RAISED,
};
use crate::state::{COMMIT_INFO, NEXT_POSITION_ID};
use crate::swap_helper::{update_price_accumulator};
use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, Env, Order, StdError, StdResult, Uint128,
};

use cw_storage_plus::Bound;
use std::vec;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::PoolState {} => to_json_binary(&query_pool_state(deps)?),
        QueryMsg::FeeState {} => to_json_binary(&query_fee_state(deps)?),
        QueryMsg::PoolCommits {
            pool_id,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        } => to_json_binary(&query_pool_commiters(
            deps,
            pool_id,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        )?),
        QueryMsg::PoolInfo {} => to_json_binary(&query_pool_info(deps)?),
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::LastCommited { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let response = match COMMIT_INFO.may_load(deps.storage, &addr)? {
                Some(commiting) => LastCommitedResponse {
                    has_commited: true,
                    last_commited: Some(commiting.last_commited),
                    last_payment_native: Some(commiting.last_payment_native),
                    last_payment_usd: Some(commiting.last_payment_usd),
                },
                None => LastCommitedResponse {
                    has_commited: false,
                    last_commited: None,
                    last_payment_native: None,
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

pub fn query_pair_info(deps: Deps) -> StdResult<PairInfo> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pair_info)
}

pub fn query_pool(deps: Deps) -> StdResult<PoolResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info)?;

    let resp = PoolResponse { assets };

    Ok(resp)
}

pub fn query_check_threshold_limit(deps: Deps) -> StdResult<CommitStatus> {
    let threshold_hit = THRESHOLD_HIT.load(deps.storage)?;
    let commit_config = COMMIT_CONFIG.load(deps.storage)?;
    if threshold_hit {
        Ok(CommitStatus::FullyCommitted)
    } else {
        let usd_raised = USD_RAISED.load(deps.storage)?;
        Ok(CommitStatus::InProgress {
            raised: usd_raised,
            target: commit_config.commit_limit_usd,
        })
    }
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
    let fee_info = FEEINFO.load(deps.storage)?;
    Ok(FeeInfoResponse { fee_info })
}

pub fn query_check_commit(deps: Deps) -> StdResult<bool> {
    let commit_info = COMMIT_CONFIG.load(deps.storage)?;
    let usd_raised = COMMITSTATUS.load(deps.storage)?;
    // true once we've raised at least the USD threshold
    Ok(usd_raised >= commit_info.commit_limit_usd)
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
    pool_id: u64,
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
    let mut count = 0;

    for item in COMMIT_INFO.range(deps.storage, start, None, Order::Ascending) {
        let (commiter_addr, commiting) = item?;

        // Filter by pool_id
        if commiting.pool_id != pool_id {
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
            last_payment_native: commiting.last_payment_native,
            last_payment_usd: commiting.last_payment_usd,
            last_commited: commiting.last_commited,
            total_paid_usd: commiting.total_paid_usd,
        });

        count += 1;

        // Stop if we've collected enough
        if commiters.len() >= limit {
            break;
        }
    }

    Ok(PoolCommitResponse {
        total_count: count,
        commiters,
    })
}
