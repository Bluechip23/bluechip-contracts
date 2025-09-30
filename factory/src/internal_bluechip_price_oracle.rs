use crate::{
    error::ContractError,
    pyth_types::{PythPriceFeedResponse, PythQueryMsg},
    state::{ATOM_USD_PRICE_FEED_ID, FACTORYINSTANTIATEINFO, POOLS_BY_CONTRACT_ADDRESS},
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult, Uint128, Uint256
};
use cw_storage_plus::Item;
use pool_factory_interfaces::{ConversionResponse, PoolQueryMsg, PoolStateResponseForFactory};
use sha2::{Digest, Sha256};

pub const ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS: &str =
    "cosmos1atom_bluechip_pool_test_addr_000000000000";
pub const ORACLE_POOL_COUNT: usize = 4; // Total pools to sample (including ATOM)
pub const MIN_POOL_LIQUIDITY: Uint128 = Uint128::new(10_000_000_000); // Min liquidity for eligibility
pub const TWAP_WINDOW: u64 = 3600; // 1 hour TWAP window
pub const UPDATE_INTERVAL: u64 = 300; // 5 minutes between updates
pub const ROTATION_INTERVAL: u64 = 3600; // Rotate random pools every hour
pub const INTERNAL_ORACLE: Item<BlueChipPriceInternalOracle> = Item::new("internal_oracle");
const PRICE_PRECISION: u128 = 1_000_000;

#[cw_serde]
pub struct BlueChipPriceInternalOracle {
    pub selected_pools: Vec<String>, // Currently selected pools (always includes ATOM)
    pub atom_pool_contract_address: Addr, // Permanent ATOM/BLUECHIP pool ID
    pub last_rotation: u64,          // When pools were last rotated
    pub rotation_interval: u64,      // How often to rotate pools
    pub bluechip_price_cache: PriceCache,
    pub update_interval: u64, // 5 minutes
}
#[cw_serde]
pub struct PriceCache {
    pub last_price: Uint128, // Last calculated TWAP
    pub last_update: u64,    // Last update timestamp
    pub twap_observations: Vec<PriceObservation>,
}
#[cw_serde]
pub struct PriceObservation {
    pub timestamp: u64,
    pub price: Uint128,           // Weighted average across pools
    pub atom_pool_price: Uint128, // Specific ATOM pool price for USD calc
}

pub fn select_random_pools_with_atom(
    deps: Deps,
    env: Env,
    num_pools: usize,
) -> StdResult<Vec<String>> {
    let atom_pool_contract_contract_address = ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string();

    // Get all eligible creator token pools (excluding ATOM pool)
    let eligible_pools = get_eligible_creator_pools(deps, &atom_pool_contract_contract_address)?;

    // Need num_pools - 1 random pools (ATOM is always included)
    let random_pools_needed = num_pools.saturating_sub(1);

    if eligible_pools.len() <= random_pools_needed {
        // Use all available pools plus ATOM
        let mut all_pools = eligible_pools;
        all_pools.push(atom_pool_contract_contract_address);
        return Ok(all_pools);
    }

    // Generate randomness from block data
    let mut hasher = Sha256::new();
    hasher.update(env.block.time.seconds().to_be_bytes());
    hasher.update(env.block.height.to_be_bytes());
    hasher.update(env.block.chain_id.as_bytes());
    let hash = hasher.finalize();

    let mut selected = Vec::new();
    let mut used_indices = std::collections::HashSet::new();

    // ALWAYS add ATOM pool first
    selected.push(atom_pool_contract_contract_address);

    // Add random creator token pools
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
    // Initialize with ATOM pool and random selection
    let selected_pools = select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
    
    let oracle = BlueChipPriceInternalOracle {
        selected_pools,
        atom_pool_contract_address: Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS),
        last_rotation: env.block.time.seconds(),
        rotation_interval: ROTATION_INTERVAL,
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
    let all_pools = POOLS_BY_CONTRACT_ADDRESS
        .range(deps.storage, None, None, Order::Ascending)
        .collect::<StdResult<Vec<_>>>()?;

    let mut eligible = Vec::new();

    for (pool_address, pool_data) in all_pools {
        if pool_address.as_str() == atom_pool_contract_address {
            continue;
        }
        let total_liquidity = pool_data.reserve0 + pool_data.reserve1;
        if total_liquidity < MIN_POOL_LIQUIDITY {
            continue;
        }
        eligible.push(pool_address.to_string());
    }

    Ok(eligible)
}

pub fn update_internal_oracle_price(deps: DepsMut, env: Env) -> Result<Response, ContractError> {
    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();

    // Check 5-minute interval
    if current_time < oracle.bluechip_price_cache.last_update + oracle.update_interval {
        return Err(ContractError::UpdateTooSoon {
            next_update: oracle.bluechip_price_cache.last_update + oracle.update_interval,
        });
    }

    // Rotate pools every hour (but keep ATOM pool)
    let mut pools_to_use = oracle.selected_pools.clone();
    if current_time >= oracle.last_rotation + oracle.rotation_interval {
        pools_to_use =
            select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
        oracle.selected_pools = pools_to_use.clone();
        oracle.last_rotation = current_time;
    }

    // Calculate weighted price from all pools
    let (weighted_price, atom_price) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pools_to_use)?;

    // Add new observation
    oracle
        .bluechip_price_cache
        .twap_observations
        .push(PriceObservation {
            timestamp: current_time,
            price: weighted_price,
            atom_pool_price: atom_price,
        });

    // Keep only observations within TWAP window
    let cutoff_time = current_time.saturating_sub(TWAP_WINDOW);
    oracle
        .bluechip_price_cache
        .twap_observations
        .retain(|obs| obs.timestamp > cutoff_time);

    // Calculate TWAP
    let twap_price = calculate_twap(&oracle.bluechip_price_cache.twap_observations)?;

    // Update cache
    oracle.bluechip_price_cache.last_price = twap_price;
    oracle.bluechip_price_cache.last_update = current_time;

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;

    Ok(Response::new()
        .add_attribute("action", "update_oracle")
        .add_attribute("twap_price", twap_price.to_string())
        .add_attribute("pools_used", pools_to_use.len().to_string()))
}

fn calculate_weighted_price_with_atom(
    deps: Deps,
    pool_addresses: &[String],
) -> Result<(Uint128, Uint128), ContractError> {
    let atom_pool_address = ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string();

    // Check ATOM pool is included
    if !pool_addresses.contains(&atom_pool_address) {
        return Err(ContractError::MissingAtomPool {});
    }

    let mut weighted_sum = Uint256::zero();
    let mut total_weight = Uint256::zero();
    let mut atom_pool_price = Uint128::zero();
    let mut has_atom_pool = false;
    let mut successful_pools = 0;

    for pool_address in pool_addresses {
        // Query pool state with error handling
        match query_pool_safe(deps, pool_address) {
            Ok(pool_state) => {
                // Skip pools with insufficient liquidity
                let total_liquidity = pool_state
                    .reserve0
                    .checked_add(pool_state.reserve1)
                    .map_err(|_| ContractError::Std(StdError::generic_err("Liquidity overflow")))?;

                if total_liquidity < MIN_POOL_LIQUIDITY {
                    continue; // Skip this pool silently
                }

                // Calculate price
                match calculate_price_from_reserves(pool_state.reserve0, pool_state.reserve1) {
                    Ok(price) => {
                        let liquidity_weight = if pool_address == &atom_pool_address {
                            has_atom_pool = true;
                            atom_pool_price = price;

                            // ATOM pool gets 2x weight for stability
                            pool_state
                                .reserve0
                                .checked_mul(Uint128::from(2u128))
                                .map_err(|_| {
                                    ContractError::Std(StdError::generic_err("Weight overflow"))
                                })?
                        } else {
                            // Normal weight for other pools
                            pool_state.reserve0
                        };

                        // Add to weighted sum
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
                        // Skip pools where price calculation fails
                        continue;
                    }
                }
            }
            Err(_) => {
                // Skip pools that fail to query
                continue;
            }
        }
    }

    // Check if ATOM pool was successfully processed
    if !has_atom_pool {
        return Err(ContractError::Std(StdError::generic_err(
            "ATOM pool price could not be calculated",
        )));
    }

    // Ensure we have at least one successful pool
    if successful_pools == 0 {
        return Err(ContractError::InsufficientData {});
    }

    // Ensure we have non-zero weight
    if total_weight.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Total weight is zero",
        )));
    }

    // Calculate final weighted average
    let weighted_average = weighted_sum
        .checked_div(total_weight)
        .map_err(|_| ContractError::Std(StdError::generic_err("Division by zero")))?;

    let final_price = Uint128::try_from(weighted_average)
        .map_err(|_| ContractError::Std(StdError::generic_err("Price conversion overflow")))?;

    Ok((final_price, atom_pool_price))
}

fn calculate_twap(observations: &[PriceObservation]) -> Result<Uint128, ContractError> {
    if observations.is_empty() {
        return Err(ContractError::InsufficientData {});
    }

    if observations.len() == 1 {
        return Ok(observations[0].price);
    }

    let mut weighted_sum = Uint256::zero();
    let mut total_time = 0u64;

    for i in 1..observations.len() {
        let time_delta = observations[i].timestamp - observations[i - 1].timestamp;
        let avg_price = (observations[i].price + observations[i - 1].price) / Uint128::from(2u128);

        weighted_sum += Uint256::from(avg_price) * Uint256::from(time_delta);
        total_time += time_delta;
    }

    if total_time == 0 {
        return Ok(observations.last().unwrap().price);
    }

    let weighted_average = Uint128::try_from(weighted_sum / Uint256::from(total_time))
        .map_err(|_| ContractError::Std(StdError::generic_err("conversion overflow")))?;

    Ok(weighted_average)
}
pub fn query_pyth_atom_usd_price(deps: Deps, env: Env) -> StdResult<Uint128> {
    let factory = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    // Query Pyth for ATOM/USD
    let query_msg = PythQueryMsg::PythConversionPriceFeed {
        id: ATOM_USD_PRICE_FEED_ID.to_string(), // The feed ID from before
    };

    let response: PythPriceFeedResponse = deps
        .querier
        .query_wasm_smart(factory.pyth_contract_addr_for_conversions, &query_msg)?;

    // Check if price is fresh
    let current_time = env.block.time.seconds() as i64;
    if current_time - response.price_feed.price.publish_time > 60 {
        return Err(StdError::generic_err("ATOM price is stale"));
    }

    // Convert to Uint128 with 6 decimals (simplified)
    let price = response.price_feed.price.price as u128;
    Ok(Uint128::from(price / 100)) // Adjust based on Pyth's exponent
}
pub fn get_bluechip_usd_price(deps: Deps, env: Env) -> StdResult<Uint128> {
    // Step 1: Get ATOM/USD from external oracle (Pyth, Band, etc.)
    let atom_usd_price = query_pyth_atom_usd_price(deps, env)?;

    let atom_pool_addr = Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS);
    // Step 2: Get BLUECHIP/ATOM from your DEX pool
    let atom_pool = POOLS_BY_CONTRACT_ADDRESS.load(deps.storage, atom_pool_addr)?;

    // Calculate how many BLUECHIP per ATOM
    // If reserve0 = 1000 BLUECHIP and reserve1 = 10 ATOM
    // Then 1 ATOM = 100 BLUECHIP
    let bluechip_per_atom =
        (atom_pool.reserve0 * Uint128::from(1_000_000u128)) / atom_pool.reserve1;

    // Step 3: Calculate BLUECHIP price in USD
    // If 1 ATOM = $10 and 1 ATOM = 100 BLUECHIP
    // Then 1 BLUECHIP = $0.10
    let bluechip_usd_price = (atom_usd_price * Uint128::from(1_000_000u128)) / bluechip_per_atom;

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
    let usd_amount = (bluechip_amount * Uint128::from(1_000_000u128)) / cached_price;
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
    let bluechip_amount = (usd_amount * Uint128::from(100u128)) / cached_price;
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
    // Check for zero reserves
    if reserve1.is_zero() {
        return Err(ContractError::Std(StdError::generic_err("Zero reserves")));
    }

    // price = (reserve0 * PRECISION) / reserve1
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
    // Force rotation regardless of time
    let new_pools = select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
    oracle.selected_pools = new_pools.clone();
    oracle.last_rotation = env.block.time.seconds();
    
    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    
    Ok(Response::new()
        .add_attribute("action", "force_rotate_pools")
        .add_attribute("pools_count", new_pools.len().to_string()))
}