#[cfg(not(test))]
use crate::pyth_types::{PriceFeedResponse, PythQueryMsg};
#[cfg(not(test))]
use crate::state::ATOM_USD_PRICE_FEED_ID;
use crate::state::{FACTORYINSTANTIATEINFO, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID};
use crate::{asset::TokenType, error::ContractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult, Uint128, Uint256,
};
use cw_storage_plus::Item;
use pool_factory_interfaces::{ConversionResponse, PoolQueryMsg, PoolStateResponseForFactory};
use sha2::{Digest, Sha256};
#[cfg(test)]
pub const MOCK_PYTH_PRICE: Item<Uint128> = Item::new("mock_pyth_price");

pub const ORACLE_POOL_COUNT: usize = 5;
pub const MIN_POOL_LIQUIDITY: Uint128 = Uint128::new(10_000_000_000);
pub const TWAP_WINDOW: u64 = 3600;
pub const UPDATE_INTERVAL: u64 = 300;
pub const ROTATION_INTERVAL: u64 = 3600;
pub const INTERNAL_ORACLE: Item<BlueChipPriceInternalOracle> = Item::new("internal_oracle");
const PRICE_PRECISION: u128 = 1_000_000;

#[cw_serde]
pub struct BlueChipPriceInternalOracle {
    pub selected_pools: Vec<String>,
    pub atom_pool_contract_address: Addr,
    pub last_rotation: u64,
    pub rotation_interval: u64,
    pub bluechip_price_cache: PriceCache,
    pub update_interval: u64,
    /// Per-pool snapshots of cumulative price accumulators from the previous oracle update.
    /// Used to compute manipulation-resistant TWAPs from on-chain accumulators.
    pub pool_cumulative_snapshots: Vec<PoolCumulativeSnapshot>,
}
#[cw_serde]
pub struct PriceCache {
    pub last_price: Uint128,
    pub last_update: u64,
    pub twap_observations: Vec<PriceObservation>,
}
#[cw_serde]
pub struct PriceObservation {
    pub timestamp: u64,
    pub price: Uint128,
    pub atom_pool_price: Uint128,
}
/// Stores the cumulative price accumulator values from a pool at a given time.
/// Used to compute TWAP = (cumulative_now - cumulative_prev) / (time_now - time_prev).
#[cw_serde]
pub struct PoolCumulativeSnapshot {
    pub pool_address: String,
    pub price0_cumulative: Uint128,
    pub block_time: u64,
}

pub fn select_random_pools_with_atom(
    deps: Deps,
    env: Env,
    num_pools: usize,
) -> StdResult<Vec<String>> {
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let atom_pool_contract_contract_address =
        factory_config.atom_bluechip_anchor_pool_address.to_string();

    // Mock Mode Check: If atom pool is admin, we are in local testing
    if factory_config.atom_bluechip_anchor_pool_address == factory_config.factory_admin_address {
        return Ok(vec![atom_pool_contract_contract_address]);
    }

    // Real Network Logic
    let eligible_pools = get_eligible_creator_pools(deps, &atom_pool_contract_contract_address)?;
    let random_pools_needed = num_pools.saturating_sub(1);

    if eligible_pools.len() <= random_pools_needed {
        let mut all_pools = eligible_pools;
        all_pools.push(atom_pool_contract_contract_address);
        return Ok(all_pools);
    }
    let mut hasher = Sha256::new();
    hasher.update(env.block.time.seconds().to_be_bytes());
    hasher.update(env.block.height.to_be_bytes());
    hasher.update(env.block.chain_id.as_bytes());
    let hash = hasher.finalize();

    let mut selected = Vec::new();
    let mut used_indices = std::collections::HashSet::new();
    selected.push(atom_pool_contract_contract_address);
    for i in 0..random_pools_needed {
        let seed = u64::from_be_bytes([
            hash[i % 32],
            hash[(i + 1) % 32],
            hash[(i + 2) % 32],
            hash[(i + 3) % 32],
            hash[(i + 4) % 32],
            hash[(i + 5) % 32],
            hash[(i + 6) % 32],
            hash[(i + 7) % 32],
        ]);

        let mut index = (seed as usize) % eligible_pools.len();

        while used_indices.contains(&index) {
            index = (index + 1) % eligible_pools.len();
        }

        used_indices.insert(index);
        selected.push(eligible_pools[index].clone());
    }

    Ok(selected)
}

pub fn initialize_internal_bluechip_oracle(
    deps: DepsMut,
    env: Env,
) -> Result<Response, ContractError> {
    let selected_pools =
        select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
    if selected_pools.is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "Cannot initialize oracle: ATOM pool must exist",
        )));
    }

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let oracle = BlueChipPriceInternalOracle {
        selected_pools,
        atom_pool_contract_address: factory_config.atom_bluechip_anchor_pool_address,
        last_rotation: env.block.time.seconds(),
        rotation_interval: ROTATION_INTERVAL,
        pool_cumulative_snapshots: vec![],
        bluechip_price_cache: PriceCache {
            last_price: Uint128::zero(),
            last_update: 0,
            twap_observations: vec![],
        },
        update_interval: UPDATE_INTERVAL,
    };

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    Ok(Response::new())
}

pub fn get_eligible_creator_pools(
    deps: Deps,
    atom_pool_contract_address: &str,
) -> StdResult<Vec<String>> {
    // Build a set of pool addresses that have bluechip tokens (single pass over POOLS_BY_ID)
    let bluechip_pool_addrs: std::collections::HashSet<Addr> = POOLS_BY_ID
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|result| {
            if let Ok((_, pool_details)) = result {
                let has_bluechip = pool_details
                    .pool_token_info
                    .iter()
                    .any(|token| matches!(token, TokenType::Bluechip { .. }));
                if has_bluechip {
                    return Some(pool_details.creator_pool_addr);
                }
            }
            None
        })
        .collect();

    let mut eligible = Vec::new();

    for result in POOLS_BY_CONTRACT_ADDRESS.range(deps.storage, None, None, Order::Ascending) {
        let (pool_address, _pool_data) = result?;

        if pool_address.as_str() == atom_pool_contract_address {
            continue;
        }

        if !bluechip_pool_addrs.contains(&pool_address) {
            continue;
        }

        let pool_state: PoolStateResponseForFactory = deps.querier.query_wasm_smart(
            pool_address.to_string(),
            &PoolQueryMsg::GetPoolState {
                pool_contract_address: pool_address.to_string(),
            },
        )?;

        let total_liquidity = pool_state.reserve0.saturating_add(pool_state.reserve1);
        if total_liquidity >= MIN_POOL_LIQUIDITY {
            eligible.push(pool_address.to_string());
        }
    }
    Ok(eligible)
}

pub fn update_internal_oracle_price(deps: DepsMut, env: Env) -> Result<Response, ContractError> {
    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();
    let next_update = oracle.bluechip_price_cache.last_update.saturating_add(oracle.update_interval);
    if current_time < next_update {
        return Err(ContractError::UpdateTooSoon {
            next_update,
        });
    }

    let mut pools_to_use = oracle.selected_pools.clone();
    if current_time >= oracle.last_rotation.saturating_add(oracle.rotation_interval) {
        pools_to_use =
            select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
        oracle.selected_pools = pools_to_use.clone();
        oracle.last_rotation = current_time;
        // Clear snapshots on rotation — new pools need fresh baseline
        oracle.pool_cumulative_snapshots = vec![];
    }
    let (weighted_price, atom_price, new_snapshots) =
        calculate_weighted_price_with_atom(
            deps.as_ref(),
            &pools_to_use,
            &oracle.pool_cumulative_snapshots,
        )?;
    oracle.pool_cumulative_snapshots = new_snapshots;
    oracle
        .bluechip_price_cache
        .twap_observations
        .push(PriceObservation {
            timestamp: current_time,
            price: weighted_price,
            atom_pool_price: atom_price,
        });
    let cutoff_time = current_time.saturating_sub(TWAP_WINDOW);
    oracle
        .bluechip_price_cache
        .twap_observations
        .retain(|obs| obs.timestamp > cutoff_time);

    let twap_price = calculate_twap(&oracle.bluechip_price_cache.twap_observations)?;
    oracle.bluechip_price_cache.last_price = twap_price;
    oracle.bluechip_price_cache.last_update = current_time;

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;

    Ok(Response::new()
        .add_attribute("action", "update_oracle")
        .add_attribute("twap_price", twap_price.to_string())
        .add_attribute("pools_used", pools_to_use.len().to_string()))
}

/// Calculates a liquidity-weighted price across sampled pools using cumulative
/// price accumulators (Uniswap V2 TWAP pattern) instead of spot reserves.
///
/// For each pool:
/// - If a previous cumulative snapshot exists, computes TWAP from the difference
/// - If no snapshot exists (first observation), falls back to spot price
///
/// Returns (weighted_price, atom_pool_price, new_snapshots).
pub fn calculate_weighted_price_with_atom(
    deps: Deps,
    pool_addresses: &[String],
    prev_snapshots: &[PoolCumulativeSnapshot],
) -> Result<(Uint128, Uint128, Vec<PoolCumulativeSnapshot>), ContractError> {
    let factory_config = FACTORYINSTANTIATEINFO
        .load(deps.storage)
        .map_err(|e| ContractError::Std(e))?;
    let atom_pool_address = factory_config.atom_bluechip_anchor_pool_address.to_string();
    if !pool_addresses.contains(&atom_pool_address) {
        return Err(ContractError::MissingAtomPool {});
    }

    let mut weighted_sum = Uint256::zero();
    let mut total_weight = Uint256::zero();
    let mut atom_pool_price = Uint128::zero();
    let mut has_atom_pool = false;
    let mut successful_pools = 0;
    let mut new_snapshots = Vec::new();

    for pool_address in pool_addresses {
        match query_pool_safe(deps, pool_address) {
            Ok(pool_state) => {
                let total_liquidity = pool_state
                    .reserve0
                    .checked_add(pool_state.reserve1)
                    .map_err(|_| ContractError::Std(StdError::generic_err("Liquidity overflow")))?;

                if total_liquidity < MIN_POOL_LIQUIDITY {
                    continue;
                }

                // Determine if Bluechip is reserve0 or reserve1 based on assets
                let is_bluechip_second = if pool_state.assets.len() >= 2 {
                    deps.api.addr_validate(&pool_state.assets[0]).is_ok()
                } else {
                    false
                };

                // Save cumulative snapshot for next update cycle.
                // price0_cumulative tracks reserve1/reserve0 (creator_per_bluechip).
                // We want bluechip_per_creator = reserve0/reserve1 = price1_cumulative.
                // But the pool accumulator `price0_cumulative_last` = sum(reserve1/reserve0 * dt)
                // and `price1_cumulative_last` = sum(reserve0/reserve1 * dt).
                // For bluechip pricing: we need reserve0(bluechip) / reserve1(other).
                let cumulative_for_price = if is_bluechip_second {
                    // bluechip is reserve1, other is reserve0
                    // price we want: reserve1/reserve0 = price0_cumulative
                    pool_state.price0_cumulative_last
                } else {
                    // bluechip is reserve0, other is reserve1
                    // price we want: reserve0/reserve1 = price1_cumulative
                    pool_state.price1_cumulative_last
                };

                new_snapshots.push(PoolCumulativeSnapshot {
                    pool_address: pool_address.clone(),
                    price0_cumulative: cumulative_for_price,
                    block_time: pool_state.block_time_last,
                });

                // Try to compute TWAP from cumulative accumulators
                let price = if let Some(prev) = prev_snapshots
                    .iter()
                    .find(|s| s.pool_address == *pool_address)
                {
                    let time_delta = pool_state.block_time_last.saturating_sub(prev.block_time);
                    let cumulative_delta =
                        cumulative_for_price.saturating_sub(prev.price0_cumulative);

                    if time_delta > 0 && !cumulative_delta.is_zero() {
                        // TWAP = cumulative_delta / time_delta
                        // The accumulator stores sum(price * time) in integer units,
                        // so dividing by time gives average price.
                        // Scale to PRICE_PRECISION for consistency.
                        let twap = cumulative_delta
                            .checked_mul(Uint128::from(PRICE_PRECISION))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err("TWAP scale overflow"))
                            })?
                            .checked_div(Uint128::from(time_delta))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err("TWAP division error"))
                            })?;
                        twap
                    } else {
                        // No time elapsed or no cumulative change — fall back to spot
                        let (bluechip_reserve, other_reserve) = if is_bluechip_second {
                            (pool_state.reserve1, pool_state.reserve0)
                        } else {
                            (pool_state.reserve0, pool_state.reserve1)
                        };
                        calculate_price_from_reserves(bluechip_reserve, other_reserve)?
                    }
                } else {
                    // No previous snapshot — first observation, use spot price as baseline
                    let (bluechip_reserve, other_reserve) = if is_bluechip_second {
                        (pool_state.reserve1, pool_state.reserve0)
                    } else {
                        (pool_state.reserve0, pool_state.reserve1)
                    };
                    calculate_price_from_reserves(bluechip_reserve, other_reserve)?
                };

                let (bluechip_reserve, _) = if is_bluechip_second {
                    (pool_state.reserve1, pool_state.reserve0)
                } else {
                    (pool_state.reserve0, pool_state.reserve1)
                };

                let liquidity_weight = if pool_address == &atom_pool_address {
                    has_atom_pool = true;
                    atom_pool_price = price;
                    // ATOM pool gets 2x weight
                    bluechip_reserve
                        .checked_mul(Uint128::from(2u128))
                        .map_err(|_| {
                            ContractError::Std(StdError::generic_err("Weight overflow"))
                        })?
                } else {
                    bluechip_reserve
                };

                weighted_sum = weighted_sum
                    .checked_add(
                        Uint256::from(price)
                            .checked_mul(Uint256::from(liquidity_weight))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err(
                                    "Weighted sum overflow",
                                ))
                            })?,
                    )
                    .map_err(|_| {
                        ContractError::Std(StdError::generic_err("Sum overflow"))
                    })?;

                total_weight = total_weight
                    .checked_add(Uint256::from(liquidity_weight))
                    .map_err(|_| {
                        ContractError::Std(StdError::generic_err("Weight sum overflow"))
                    })?;

                successful_pools += 1;
            }
            Err(_) => {
                continue;
            }
        }
    }

    if !has_atom_pool {
        return Err(ContractError::Std(StdError::generic_err(
            "ATOM pool price could not be calculated",
        )));
    }

    if successful_pools == 0 {
        return Err(ContractError::InsufficientData {});
    }

    if total_weight.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Total weight is zero",
        )));
    }
    let weighted_average = weighted_sum
        .checked_div(total_weight)
        .map_err(|_| ContractError::Std(StdError::generic_err("Division by zero")))?;

    let final_price = Uint128::try_from(weighted_average)
        .map_err(|_| ContractError::Std(StdError::generic_err("Price conversion overflow")))?;

    Ok((final_price, atom_pool_price, new_snapshots))
}

pub fn calculate_twap(observations: &[PriceObservation]) -> Result<Uint128, ContractError> {
    if observations.is_empty() {
        return Err(ContractError::InsufficientData {});
    }

    if observations.len() == 1 {
        return Ok(observations[0].price);
    }

    let mut weighted_sum = Uint256::zero();
    let mut total_time = 0u64;

    for i in 1..observations.len() {
        let time_delta = observations[i].timestamp.saturating_sub(observations[i - 1].timestamp);
        let avg_price = observations[i].price
            .checked_add(observations[i - 1].price)
            .map_err(|_| ContractError::Std(StdError::generic_err("Price addition overflow")))?
            / Uint128::from(2u128);

        weighted_sum = weighted_sum.checked_add(
            Uint256::from(avg_price).checked_mul(Uint256::from(time_delta))
                .map_err(|_| ContractError::Std(StdError::generic_err("TWAP weighted sum overflow")))?
        ).map_err(|_| ContractError::Std(StdError::generic_err("TWAP accumulator overflow")))?;
        total_time = total_time.saturating_add(time_delta);
    }

    if total_time == 0 {
        return observations.last()
            .map(|obs| obs.price)
            .ok_or_else(|| ContractError::Std(StdError::generic_err("No observations available")));
    }

    let weighted_average = Uint128::try_from(
        weighted_sum.checked_div(Uint256::from(total_time))
            .map_err(|_| ContractError::Std(StdError::generic_err("TWAP division error")))?
    ).map_err(|_| ContractError::Std(StdError::generic_err("conversion overflow")))?;

    Ok(weighted_average)
}
pub fn query_pyth_atom_usd_price(deps: Deps, env: Env) -> StdResult<Uint128> {
    #[cfg(not(test))]
    {
        let factory = FACTORYINSTANTIATEINFO.load(deps.storage)?;

        // Use the configurable feed ID from factory config, not the hardcoded constant
        let feed_id = &factory.pyth_atom_usd_price_feed_id;

        let query_msg = PythQueryMsg::PythConversionPriceFeed {
            id: feed_id.clone(),
        };

        // Try the standard query first, fallback to GetPrice for local/mock oracle
        let response: PriceFeedResponse = match deps.querier.query_wasm_smart(
            factory.pyth_contract_addr_for_conversions.clone(),
            &query_msg,
        ) {
            Ok(res) => res,
            //used for mock oracle
            Err(_) => {
                let fallback_msg = PythQueryMsg::GetPrice {
                    price_id: feed_id.clone(),
                };
                deps.querier
                    .query_wasm_smart(factory.pyth_contract_addr_for_conversions, &fallback_msg)?
            }
        };

        let current_time = env.block.time.seconds() as i64;

        // Extract price data from either standard Pyth response or Mock Oracle response
        let price_data = if let Some(feed) = response.price_feed {
            feed.price
        } else if let Some(price) = response.price {
            price
        } else {
            return Err(StdError::generic_err(
                "Invalid oracle response: missing price data",
            ));
        };

        if current_time - price_data.publish_time > crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE as i64 {
            return Err(StdError::generic_err("ATOM price is stale"));
        }

        // Validate price is positive
        if price_data.price <= 0 {
            return Err(StdError::generic_err("Invalid negative or zero price"));
        }

        let price_u128 = price_data.price as u128;
        let expo = price_data.expo;

        // Validate expo is within reasonable range for price feeds
        if expo > -4 || expo < -12 {
            return Err(StdError::generic_err(format!(
                "Unexpected Pyth expo: {}. Expected between -12 and -4",
                expo
            )));
        }

        // Normalize to 6 decimals (system standard)
        let normalized_price = if expo == -6 {
            Uint128::from(price_u128)
        } else if expo < -6 {
            let divisor = 10u128.pow((expo.abs() - 6) as u32);
            Uint128::from(price_u128 / divisor)
        } else {
            let multiplier = 10u128.pow((6 - expo.abs()) as u32);
            Uint128::from(price_u128 * multiplier)
        };

        Ok(normalized_price)
    }
    #[cfg(test)]
    {
        let _ = env;
        let mock_price = MOCK_PYTH_PRICE
            .may_load(deps.storage)?
            .unwrap_or(Uint128::new(10_000_000)); // Default $10
        Ok(mock_price)
    }
}

pub fn get_bluechip_usd_price(deps: Deps, env: Env) -> StdResult<Uint128> {
    let atom_usd_price = query_pyth_atom_usd_price(deps, env.clone())?;

    // Check for Mock/Local Mode
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if factory_config.atom_bluechip_anchor_pool_address == factory_config.factory_admin_address {
        // BYPASS INTERNAL ORACLE FOR LOCAL TESTING
        return Ok(atom_usd_price);
    }

    // Load the internal oracle to get the TWAP of Bluechip/ATOM
    let oracle = INTERNAL_ORACLE
        .load(deps.storage)
        .map_err(|_| StdError::generic_err("Internal oracle not initialized"))?;

    let bluechip_per_atom_twap = oracle.bluechip_price_cache.last_price;

    if bluechip_per_atom_twap.is_zero() {
        return Err(StdError::generic_err(
            "TWAP price is zero - oracle may need update",
        ));
    }

    // Calculate USD price using TWAP
    // bluechip_usd_price = atom_usd_price / bluechip_per_atom_twap
    // Units: (USD/ATOM) / (Bluechip/ATOM) = USD/Bluechip
    let bluechip_usd_price = atom_usd_price
        .checked_mul(Uint128::from(PRICE_PRECISION))
        .map_err(|e| {
            StdError::generic_err(format!("Overflow calculating bluechip USD price: {}", e))
        })?
        .checked_div(bluechip_per_atom_twap)
        .map_err(|e| {
            StdError::generic_err(format!(
                "Division error calculating bluechip USD price: {}",
                e
            ))
        })?;

    if bluechip_usd_price.is_zero() {
        return Err(StdError::generic_err("Calculated bluechip price is zero"));
    }

    Ok(bluechip_usd_price)
}

pub fn bluechip_to_usd(
    deps: Deps,
    bluechip_amount: Uint128,
    env: Env,
) -> StdResult<ConversionResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let cached_price = get_bluechip_usd_price(deps, env)?;

    if cached_price.is_zero() {
        return Err(StdError::generic_err("Invalid zero price"));
    }

    let usd_amount = bluechip_amount
        .checked_mul(cached_price)
        .map_err(|e| {
            StdError::generic_err(format!("Overflow in bluechip to USD conversion: {}", e))
        })?
        .checked_div(Uint128::from(PRICE_PRECISION))
        .map_err(|e| {
            StdError::generic_err(format!(
                "Division error in bluechip to USD conversion: {}",
                e
            ))
        })?;

    Ok(ConversionResponse {
        amount: usd_amount,
        rate_used: cached_price,
        timestamp: oracle.bluechip_price_cache.last_update,
    })
}
pub fn usd_to_bluechip(deps: Deps, usd_amount: Uint128, env: Env) -> StdResult<ConversionResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let cached_price = get_bluechip_usd_price(deps, env)?;

    if cached_price.is_zero() {
        return Err(StdError::generic_err("Invalid zero price"));
    }

    let bluechip_amount = usd_amount
        .checked_mul(Uint128::from(PRICE_PRECISION))
        .map_err(|e| {
            StdError::generic_err(format!("Overflow in USD to bluechip conversion: {}", e))
        })?
        .checked_div(cached_price)
        .map_err(|e| {
            StdError::generic_err(format!(
                "Division error in USD to bluechip conversion: {}",
                e
            ))
        })?;

    Ok(ConversionResponse {
        amount: bluechip_amount,
        rate_used: cached_price,
        timestamp: oracle.bluechip_price_cache.last_update,
    })
}

pub fn get_price_with_staleness_check(
    deps: Deps,
    env: Env,
    max_staleness: u64,
) -> StdResult<Uint128> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();

    if current_time > oracle.bluechip_price_cache.last_update + max_staleness {
        return Err(StdError::generic_err("Price is stale"));
    }

    Ok(oracle.bluechip_price_cache.last_price)
}

fn calculate_price_from_reserves(
    reserve0: Uint128, // bluechip
    reserve1: Uint128, // other token
) -> Result<Uint128, ContractError> {
    if reserve1.is_zero() {
        return Err(ContractError::Std(StdError::generic_err("Zero reserves")));
    }

    let price = reserve0
        .checked_mul(Uint128::from(PRICE_PRECISION))
        .map_err(|_| ContractError::Std(StdError::generic_err("Price calculation overflow")))?
        .checked_div(reserve1)
        .map_err(|_| ContractError::Std(StdError::generic_err("Price division error")))?;

    Ok(price)
}

fn query_pool_safe(
    deps: Deps,
    pool_address: &str,
) -> Result<PoolStateResponseForFactory, ContractError> {
    #[cfg(not(test))]
    {
        deps.querier
            .query_wasm_smart(
                pool_address.to_string(),
                &PoolQueryMsg::GetPoolState {
                    pool_contract_address: pool_address.to_string(),
                },
            )
            .map_err(|e| ContractError::QueryError {
                msg: format!("Failed to query pool {}: {}", pool_address, e),
            })
    }

    #[cfg(test)]
    {
        let addr = deps
            .api
            .addr_validate(pool_address)
            .map_err(|e| ContractError::QueryError {
                msg: format!("Invalid pool address {}: {}", pool_address, e),
            })?;

        POOLS_BY_CONTRACT_ADDRESS
            .load(deps.storage, addr)
            .map_err(|_| ContractError::QueryError {
                msg: format!("Pool {} not found in storage", pool_address),
            })
    }
}

pub fn execute_force_rotate_pools(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Check admin permission
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if info.sender != config.factory_admin_address {
        return Err(ContractError::Unauthorized {});
    }

    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let new_pools = select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
    oracle.selected_pools = new_pools.clone();
    oracle.last_rotation = env.block.time.seconds();

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;

    Ok(Response::new()
        .add_attribute("action", "force_rotate_pools")
        .add_attribute("pools_count", new_pools.len().to_string()))
}
