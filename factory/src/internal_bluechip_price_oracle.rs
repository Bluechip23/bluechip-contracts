#[cfg(not(test))]
use crate::pyth_types::{PriceFeedResponse, PythQueryMsg};

use crate::state::{
    FACTORYINSTANTIATEINFO, ORACLE_BOUNTY_DENOM, ORACLE_UPDATE_BOUNTY_USD,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_THRESHOLD_MINTED,
};
use crate::{asset::TokenType, error::ContractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError,
    StdResult, Uint128, Uint256,
};
use cw_storage_plus::Item;
use pool_factory_interfaces::{ConversionResponse, PoolQueryMsg, PoolStateResponseForFactory};
use sha2::{Digest, Sha256};
#[cfg(test)]
pub const MOCK_PYTH_PRICE: Item<Uint128> = Item::new("mock_pyth_price");
// When set to true in tests, query_pyth_atom_usd_price returns Err,
// letting tests exercise the cache-fallback branch of get_bluechip_usd_price.
#[cfg(test)]
pub const MOCK_PYTH_SHOULD_FAIL: Item<bool> = Item::new("mock_pyth_should_fail");

pub const ORACLE_POOL_COUNT: usize = 5;
pub const MIN_POOL_LIQUIDITY: Uint128 = Uint128::new(10_000_000_000);
pub const TWAP_WINDOW: u64 = 3600;
pub const UPDATE_INTERVAL: u64 = 300;
pub const ROTATION_INTERVAL: u64 = 3600;

// Minimum number of threshold-crossed creator pools (in addition to the
// anchor ATOM/bluechip pool) required before the factory is willing to
// return a TWAP-derived bluechip USD price. Until at least this many
// creator pools have crossed threshold, every price query falls back to
// the single anchor pool alone — which is trivially manipulable by
// anyone who can move that one pool. Raising the floor forces the oracle
// to error ("insufficient data") during the bootstrap window instead of
// serving a single-pool-dominated price that is effectively attacker-
// controlled. Callers (commit, conversion queries) then freeze until the
// ecosystem has enough pools to dilute any single actor's influence.
pub const MIN_ELIGIBLE_POOLS_FOR_TWAP: usize = 3;
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
    pub pool_cumulative_snapshots: Vec<PoolCumulativeSnapshot>,
}
#[cw_serde]
pub struct PriceCache {
    pub last_price: Uint128,
    pub last_update: u64,
    pub twap_observations: Vec<PriceObservation>,

    #[serde(default)]
    pub cached_pyth_price: Uint128,
    #[serde(default)]
    pub cached_pyth_timestamp: u64,
}
#[cw_serde]
pub struct PriceObservation {
    pub timestamp: u64,
    pub price: Uint128,
    pub atom_pool_price: Uint128,
}

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

    #[cfg(feature = "mock")]
    {
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

    let oracle_state =
        INTERNAL_ORACLE
            .may_load(deps.storage)?
            .unwrap_or_else(|| BlueChipPriceInternalOracle {
                selected_pools: vec![],
                atom_pool_contract_address: factory_config
                    .atom_bluechip_anchor_pool_address
                    .clone(),
                last_rotation: 0,
                rotation_interval: ROTATION_INTERVAL,
                pool_cumulative_snapshots: vec![],
                bluechip_price_cache: PriceCache {
                    last_price: Uint128::zero(),
                    last_update: 0,
                    twap_observations: vec![],
                    cached_pyth_price: Uint128::zero(),
                    cached_pyth_timestamp: 0,
                },
                update_interval: UPDATE_INTERVAL,
            });
    let mut hasher = Sha256::new();
    hasher.update(env.block.time.seconds().to_be_bytes());
    hasher.update(env.block.height.to_be_bytes());
    hasher.update(env.block.chain_id.as_bytes());
    // Unpredictable at block-production time: determined by previous oracle update
    hasher.update(
        oracle_state
            .bluechip_price_cache
            .last_price
            .u128()
            .to_be_bytes(),
    );
    hasher.update(oracle_state.bluechip_price_cache.last_update.to_be_bytes());
    hasher.update((oracle_state.bluechip_price_cache.twap_observations.len() as u64).to_be_bytes());
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
            cached_pyth_price: Uint128::zero(),
            cached_pyth_timestamp: 0,
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
    // Return every pool eligible for oracle sampling. A pool is eligible iff:
    //   1. It contains a bluechip token (so we can price it against ATOM).
    //   2. It has crossed its commit threshold (POOL_THRESHOLD_MINTED == true).
    //   3. Its current reserves sum to >= MIN_POOL_LIQUIDITY.
    //
    // The threshold-crossed gate is the important one: pool creation is
    // permissionless, so without this check a spammer could bloat the oracle
    // sample set with pre-threshold pools. The MIN_POOL_LIQUIDITY check is
    // defense-in-depth for pools that crossed threshold but later drained.
    //
    // Single pass over POOLS_BY_ID: for each candidate we check the two
    // cheap in-storage gates first and only incur the cross-contract
    // PoolStateResponseForFactory query when both pass. The older
    // implementation did two full range scans plus a HashSet build, which
    // dominated oracle-update gas at scale.
    let mut eligible = Vec::new();
    for row in POOLS_BY_ID.range(deps.storage, None, None, Order::Ascending) {
        let (pool_id, pool_details) = row?;

        if pool_details.creator_pool_addr.as_str() == atom_pool_contract_address {
            continue;
        }
        let has_bluechip = pool_details
            .pool_token_info
            .iter()
            .any(|token| matches!(token, TokenType::Bluechip { .. }));
        if !has_bluechip {
            continue;
        }
        if !POOL_THRESHOLD_MINTED
            .may_load(deps.storage, pool_id)?
            .unwrap_or(false)
        {
            continue;
        }

        let pool_state: PoolStateResponseForFactory = deps.querier.query_wasm_smart(
            pool_details.creator_pool_addr.to_string(),
            &PoolQueryMsg::GetPoolState {
                pool_contract_address: pool_details.creator_pool_addr.to_string(),
            },
        )?;

        let total_liquidity = pool_state.reserve0.saturating_add(pool_state.reserve1);
        if total_liquidity >= MIN_POOL_LIQUIDITY {
            eligible.push(pool_details.creator_pool_addr.to_string());
        }
    }
    Ok(eligible)
}

// MOCK-ONLY: read the bluechip USD price directly from the configured mock
// oracle contract (keyed under "BLUECHIP_USD"). In mock builds, the keeper
// pushes a fresh SetPrice to this contract each tick; the factory then reads
// it here and treats it as the authoritative price. Production builds are
// untouched — they still derive the price from pool TWAPs.
#[cfg(feature = "mock")]
pub fn query_mock_bluechip_usd_price(deps: Deps) -> Result<Uint128, ContractError> {
    use crate::pyth_types::{PriceResponse, PythQueryMsg};
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let resp: PriceResponse = deps
        .querier
        .query_wasm_smart(
            factory_config.pyth_contract_addr_for_conversions.as_str(),
            &PythQueryMsg::GetPrice {
                price_id: "BLUECHIP_USD".to_string(),
            },
        )
        .map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "mock bluechip price query failed: {}",
                e
            )))
        })?;
    if resp.price.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "mock bluechip price is zero",
        )));
    }
    Ok(resp.price)
}

// Append the oracle-update keeper-bounty outcome attributes (and, on success,
// the BankMsg transfer) to `response`. Three branches, deterministic attribute
// shape. Shared between the mock and prod oracle paths so the attribute
// schema can only drift in one place.
fn apply_oracle_bounty(
    mut response: Response,
    bounty_usd: Uint128,
    bounty_bluechip: Uint128,
    factory_balance: Uint128,
    recipient: &Addr,
) -> Response {
    if !bounty_bluechip.is_zero() && factory_balance >= bounty_bluechip {
        response = response
            .add_message(CosmosMsg::Bank(BankMsg::Send {
                to_address: recipient.to_string(),
                amount: vec![Coin {
                    denom: ORACLE_BOUNTY_DENOM.to_string(),
                    amount: bounty_bluechip,
                }],
            }))
            .add_attribute("bounty_paid_bluechip", bounty_bluechip.to_string())
            .add_attribute("bounty_paid_usd", bounty_usd.to_string())
            .add_attribute("bounty_recipient", recipient.to_string());
    } else if bounty_bluechip.is_zero() {
        response = response
            .add_attribute("bounty_skipped", "conversion_returned_zero")
            .add_attribute("bounty_configured_usd", bounty_usd.to_string());
    } else {
        response = response
            .add_attribute("bounty_skipped", "insufficient_factory_balance")
            .add_attribute("bounty_required_bluechip", bounty_bluechip.to_string())
            .add_attribute("bounty_configured_usd", bounty_usd.to_string())
            .add_attribute("factory_balance", factory_balance.to_string());
    }
    response
}

pub fn update_internal_oracle_price(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();
    let next_update = oracle
        .bluechip_price_cache
        .last_update
        .saturating_add(oracle.update_interval);
    if current_time < next_update {
        return Err(ContractError::UpdateTooSoon { next_update });
    }

    // MOCK-ONLY short-circuit. If a mock oracle is configured with a
    // BLUECHIP_USD price feed, read that price and skip pool TWAP math.
    // When the mock oracle query returns no price (not configured, or
    // feed id missing), fall through to the prod pool-TWAP path — this
    // keeps existing factory tests that exercise the prod path under
    // `--features mock` working unchanged.
    #[cfg(feature = "mock")]
    if let Ok(price) = query_mock_bluechip_usd_price(deps.as_ref()) {
        oracle.bluechip_price_cache.last_price = price;
        oracle.bluechip_price_cache.last_update = current_time;
        oracle
            .bluechip_price_cache
            .twap_observations
            .push(PriceObservation {
                timestamp: current_time,
                price,
                atom_pool_price: price,
            });
        INTERNAL_ORACLE.save(deps.storage, &oracle)?;

        let bounty_usd = ORACLE_UPDATE_BOUNTY_USD
            .may_load(deps.storage)?
            .unwrap_or_default();
        let mut response = Response::new()
            .add_attribute("action", "update_oracle")
            .add_attribute("twap_price", price.to_string())
            .add_attribute("mock_mode", "true");

        if !bounty_usd.is_zero() {
            // Convert USD -> bluechip using the price we just fetched from
            // the mock oracle (not via get_bluechip_usd_price, which in mock
            // builds returns the ATOM/USD shortcut used by other paths).
            let bounty_bluechip = bounty_usd
                .checked_mul(Uint128::from(PRICE_PRECISION))
                .map_err(|_| {
                    ContractError::Std(StdError::generic_err("bounty conversion overflow"))
                })?
                .checked_div(price)
                .map_err(|_| {
                    ContractError::Std(StdError::generic_err("bounty conversion div-by-zero"))
                })?;
            let balance = deps
                .querier
                .query_balance(env.contract.address.as_str(), ORACLE_BOUNTY_DENOM)?;
            response = apply_oracle_bounty(
                response,
                bounty_usd,
                bounty_bluechip,
                balance.amount,
                &info.sender,
            );
        }
        return Ok(response);
    }

    let mut pools_to_use = oracle.selected_pools.clone();
    if current_time
        >= oracle
            .last_rotation
            .saturating_add(oracle.rotation_interval)
    {
        pools_to_use =
            select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
        oracle.selected_pools = pools_to_use.clone();
        oracle.last_rotation = current_time;
        // Retain snapshots only for pools that remain in the new selection to preserve TWAP continuity
        oracle
            .pool_cumulative_snapshots
            .retain(|s| pools_to_use.contains(&s.pool_address));
    }
    let (weighted_price, atom_price, new_snapshots) = calculate_weighted_price_with_atom(
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
        .retain(|obs| obs.timestamp >= cutoff_time);

    let twap_price = calculate_twap(&oracle.bluechip_price_cache.twap_observations)?;
    oracle.bluechip_price_cache.last_price = twap_price;
    oracle.bluechip_price_cache.last_update = current_time;

    // Cache the Pyth ATOM/USD price alongside the TWAP update
    if let Ok(pyth_price) = query_pyth_atom_usd_price(deps.as_ref(), &env) {
        oracle.bluechip_price_cache.cached_pyth_price = pyth_price;
        oracle.bluechip_price_cache.cached_pyth_timestamp = current_time;
    }

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;

    // Keeper bounty: pay the caller out of the factory's native balance.
    // Stored in USD (6 decimals) and converted to bluechip at payout time
    // using the just-updated oracle price, so keeper compensation stays
    // roughly stable in USD as bluechip price fluctuates. Skip reasons
    // emit attributes instead of erroring — a Pyth outage shouldn't also
    // halt the keepers that fix it. UPDATE_INTERVAL above gates frequency.
    let bounty_usd = ORACLE_UPDATE_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();
    let mut response = Response::new()
        .add_attribute("action", "update_oracle")
        .add_attribute("twap_price", twap_price.to_string())
        .add_attribute("pools_used", pools_to_use.len().to_string());

    if !bounty_usd.is_zero() {
        // Convert USD -> bluechip via the just-updated TWAP. If the
        // conversion errors (Pyth + cache both unavailable), skip the
        // bounty rather than reverting the whole oracle update.
        match usd_to_bluechip(deps.as_ref(), bounty_usd, env.clone()) {
            Ok(conv) => {
                let balance = deps
                    .querier
                    .query_balance(env.contract.address.as_str(), ORACLE_BOUNTY_DENOM)?;
                response = apply_oracle_bounty(
                    response,
                    bounty_usd,
                    conv.amount,
                    balance.amount,
                    &info.sender,
                );
            }
            Err(_) => {
                response = response
                    .add_attribute("bounty_skipped", "price_unavailable")
                    .add_attribute("bounty_configured_usd", bounty_usd.to_string());
            }
        }
    }

    Ok(response)
}

// Calculates a liquidity-weighted price across sampled pools using cumulative
pub fn calculate_weighted_price_with_atom(
    deps: Deps,
    pool_addresses: &[String],
    prev_snapshots: &[PoolCumulativeSnapshot],
) -> Result<(Uint128, Uint128, Vec<PoolCumulativeSnapshot>), ContractError> {
    let factory_config = FACTORYINSTANTIATEINFO
        .load(deps.storage)
        .map_err(ContractError::Std)?;
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

                // Determine if Bluechip is reserve0 or reserve1 by looking up the
                let is_bluechip_second = {
                    let mut found = false;
                    for (_id, pool_details) in POOLS_BY_ID
                        .range(deps.storage, None, None, Order::Ascending)
                        .flatten()
                    {
                        if pool_details.creator_pool_addr.as_str() == pool_address.as_str() {
                            // asset_infos[0] is CreatorToken => bluechip is second (index 1)
                            found = matches!(
                                pool_details.pool_token_info[0],
                                TokenType::CreatorToken { .. }
                            );
                            break;
                        }
                    }
                    found
                };

                // Resolve reserves once based on token ordering
                let (bluechip_reserve, other_reserve) = if is_bluechip_second {
                    (pool_state.reserve1, pool_state.reserve0)
                } else {
                    (pool_state.reserve0, pool_state.reserve1)
                };

                // Save cumulative snapshot for next update cycle.
                // price0_cumulative tracks reserve1/reserve0 (creator_per_bluechip).
                // For bluechip pricing: we need reserve0(bluechip) / reserve1(other).
                let cumulative_for_price = if is_bluechip_second {
                    pool_state.price0_cumulative_last
                } else {
                    pool_state.price1_cumulative_last
                };

                new_snapshots.push(PoolCumulativeSnapshot {
                    pool_address: pool_address.clone(),
                    price0_cumulative: cumulative_for_price,
                    block_time: pool_state.block_time_last,
                });

                // H3 hardening: distinguish anchor vs. creator pools when
                // the cumulative accumulator hasn't advanced.
                //
                // - Creator pools are the real attack surface here: they vary
                //   wildly in liquidity, an attacker can shop for the quietest
                //   one, and spot price is single-block manipulable. We skip
                //   any creator pool with zero cumulative-delta; it rejoins
                //   the weighted sum on the next update once real trading
                //   activity advances its accumulator.
                //
                // - The anchor (ATOM/bluechip) pool is curated by the
                //   deployment team and expected to stay highly liquid.
                //   Manipulating it takes materially more capital than a
                //   random creator pool, and without it the oracle can't
                //   compute a price at all. We keep the spot-price fallback
                //   here so a temporarily inactive anchor doesn't freeze the
                //   whole oracle — at the cost of leaving a narrow, high-cost
                //   spot-manipulation vector on the anchor specifically.
                let is_anchor = pool_address == &atom_pool_address;
                let price = if let Some(prev) = prev_snapshots
                    .iter()
                    .find(|s| s.pool_address == *pool_address)
                {
                    let time_delta = pool_state.block_time_last.saturating_sub(prev.block_time);
                    let cumulative_delta =
                        cumulative_for_price.saturating_sub(prev.price0_cumulative);

                    if time_delta > 0 && !cumulative_delta.is_zero() {
                        // TWAP = cumulative_delta / time_delta
                        // Scale to PRICE_PRECISION for consistency.
                        cumulative_delta
                            .checked_mul(Uint128::from(PRICE_PRECISION))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err("TWAP scale overflow"))
                            })?
                            .checked_div(Uint128::from(time_delta))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err("TWAP division error"))
                            })?
                    } else if is_anchor {
                        // Anchor-only spot fallback. See comment block above.
                        calculate_price_from_reserves(bluechip_reserve, other_reserve)?
                    } else {
                        // Creator pool with no TWAP evidence this round — skip.
                        continue;
                    }
                } else if prev_snapshots.is_empty() {
                    // Bootstrap case: very first oracle update in the factory's
                    // entire lifetime — no prior snapshots exist for any pool.
                    // After this call prev_snapshots will be non-empty forever,
                    // so the spot price is only ever used once, system-wide,
                    // and before any significant protocol activity.
                    calculate_price_from_reserves(bluechip_reserve, other_reserve)?
                } else if is_anchor {
                    // Anchor was somehow missing from prev_snapshots despite
                    // prev_snapshots being non-empty (e.g. first update after
                    // an admin migration that cleared the snapshot set but
                    // populated it for creator pools). Spot-fallback the
                    // anchor so the update can still produce a price.
                    calculate_price_from_reserves(bluechip_reserve, other_reserve)?
                } else {
                    // Post-rotation: this creator pool is newly selected and
                    // has no prior snapshot. Skip it from price weighting.
                    // The snapshot was already recorded above, so TWAP data
                    // will be available on the next update.
                    continue;
                };

                let liquidity_weight = if pool_address == &atom_pool_address {
                    has_atom_pool = true;
                    atom_pool_price = price;
                    // ATOM pool gets 2x weight
                    bluechip_reserve
                        .checked_mul(Uint128::from(2u128))
                        .map_err(|_| ContractError::Std(StdError::generic_err("Weight overflow")))?
                } else {
                    bluechip_reserve
                };

                weighted_sum = weighted_sum
                    .checked_add(
                        Uint256::from(price)
                            .checked_mul(Uint256::from(liquidity_weight))
                            .map_err(|_| {
                                ContractError::Std(StdError::generic_err("Weighted sum overflow"))
                            })?,
                    )
                    .map_err(|_| ContractError::Std(StdError::generic_err("Sum overflow")))?;

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
        let time_delta = observations[i]
            .timestamp
            .saturating_sub(observations[i - 1].timestamp);
        let avg_price = observations[i]
            .price
            .checked_add(observations[i - 1].price)
            .map_err(|_| ContractError::Std(StdError::generic_err("Price addition overflow")))?
            / Uint128::from(2u128);

        weighted_sum = weighted_sum
            .checked_add(
                Uint256::from(avg_price)
                    .checked_mul(Uint256::from(time_delta))
                    .map_err(|_| {
                        ContractError::Std(StdError::generic_err("TWAP weighted sum overflow"))
                    })?,
            )
            .map_err(|_| ContractError::Std(StdError::generic_err("TWAP accumulator overflow")))?;
        total_time = total_time.saturating_add(time_delta);
    }

    if total_time == 0 {
        return observations
            .last()
            .map(|obs| obs.price)
            .ok_or_else(|| ContractError::Std(StdError::generic_err("No observations available")));
    }

    let weighted_average = Uint128::try_from(
        weighted_sum
            .checked_div(Uint256::from(total_time))
            .map_err(|_| ContractError::Std(StdError::generic_err("TWAP division error")))?,
    )
    .map_err(|_| ContractError::Std(StdError::generic_err("conversion overflow")))?;

    Ok(weighted_average)
}
pub fn query_pyth_atom_usd_price(deps: Deps, env: &Env) -> StdResult<Uint128> {
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

        if current_time - price_data.publish_time
            > crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE as i64
        {
            return Err(StdError::generic_err("ATOM price is stale"));
        }

        // Validate price is positive. We rely on this check for the conf
        // threshold below — moving or removing it would cause `price as u64`
        // to wrap a negative value into a huge number and pass the conf
        // check vacuously. Don't reorder.
        if price_data.price <= 0 {
            return Err(StdError::generic_err("Invalid negative or zero price"));
        }

        // Reject prices with wide confidence intervals (> 5% of price).
        // During low oracle participation or extreme volatility, Pyth may
        // report prices with very wide bands that are unreliable.
        //
        // Use try_into() rather than `as u64` so a future edit that drops
        // or reorders the negative-price check above produces an explicit
        // runtime error rather than a silent wrap to u64::MAX-ish that
        // would let a wide-conf price pass.
        let price_u64: u64 = price_data.price.try_into().map_err(|_| {
            StdError::generic_err("Price overflow when computing conf threshold")
        })?;
        let conf_threshold = price_u64 / 20; // 5%
        if price_data.conf > conf_threshold {
            return Err(StdError::generic_err(format!(
                "Pyth confidence interval too wide: conf={} exceeds 5% of price={}",
                price_data.conf, price_data.price
            )));
        }

        let price_u128 = price_data.price as u128;
        let expo = price_data.expo;

        // Validate expo is within reasonable range for price feeds
        if !(-12..=-4).contains(&expo) {
            return Err(StdError::generic_err(format!(
                "Unexpected Pyth expo: {}. Expected between -12 and -4",
                expo
            )));
        }

        // Normalize to 6 decimals (system standard)
        let normalized_price = match expo.cmp(&-6) {
            std::cmp::Ordering::Equal => Uint128::from(price_u128),
            std::cmp::Ordering::Less => {
                let divisor = 10u128.pow((expo.abs() - 6) as u32);
                Uint128::from(price_u128 / divisor)
            }
            std::cmp::Ordering::Greater => {
                let multiplier = 10u128.pow((6 - expo.abs()) as u32);
                Uint128::from(price_u128 * multiplier)
            }
        };

        Ok(normalized_price)
    }
    #[cfg(test)]
    {
        let _ = env;
        // Simulate a Pyth outage so tests can exercise the cache-fallback
        // path of get_bluechip_usd_price. Tests set this flag then clear it.
        if MOCK_PYTH_SHOULD_FAIL
            .may_load(deps.storage)?
            .unwrap_or(false)
        {
            return Err(StdError::generic_err("mock: pyth query failed"));
        }
        let mock_price = MOCK_PYTH_PRICE
            .may_load(deps.storage)?
            .unwrap_or(Uint128::new(10_000_000)); // Default $10
        Ok(mock_price)
    }
}

pub fn get_bluechip_usd_price(deps: Deps, env: &Env) -> StdResult<Uint128> {
    // Try live Pyth price first; fall back to cached price if Pyth is stale.
    let atom_usd_price = match query_pyth_atom_usd_price(deps, env) {
        Ok(price) => price,
        Err(_) => {
            // Pyth query failed (likely stale). The cache only bridges very
            // short Pyth outages — we use the same staleness threshold as the
            // live query (MAX_PRICE_AGE_SECONDS_BEFORE_STALE, currently 300s).
            // If Pyth has been unavailable longer than that, refuse to price
            // rather than letting a volatile old value leak into commit USD
            // valuations. This converts a prolonged Pyth outage into a
            // temporary commit freeze, which is safer than mispricing.
            let oracle = INTERNAL_ORACLE
                .load(deps.storage)
                .map_err(|_| StdError::generic_err("Internal oracle not initialized"))?;
            let cache = &oracle.bluechip_price_cache;
            let current_time = env.block.time.seconds();
            let max_cache_age = crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE;
            if cache.cached_pyth_price.is_zero()
                || current_time.saturating_sub(cache.cached_pyth_timestamp) > max_cache_age
            {
                return Err(StdError::generic_err(
                    "Pyth price stale and no valid cached price available",
                ));
            }
            cache.cached_pyth_price
        }
    };

    #[cfg(feature = "mock")]
    {
        return Ok(atom_usd_price);
    }

    // Load the internal oracle to get the TWAP of Bluechip/ATOM
    let oracle = INTERNAL_ORACLE
        .load(deps.storage)
        .map_err(|_| StdError::generic_err("Internal oracle not initialized"))?;

    // Bootstrap guard: if fewer than MIN_ELIGIBLE_POOLS_FOR_TWAP creator
    // pools have crossed threshold, the TWAP is effectively a single-pool
    // price (the anchor). Refuse to serve it rather than letting an
    // attacker who can move the anchor pool dictate the bluechip USD price
    // to every downstream consumer. Eligibility already requires
    // POOL_THRESHOLD_MINTED == true (see get_eligible_creator_pools), so
    // we can reuse the same query.
    //
    // Gated on cfg(not(test)) so unit tests can exercise oracle math
    // in isolation without being forced to stand up three threshold-
    // crossed pools. Tests that specifically want to verify the
    // bootstrap behavior can set it up explicitly.
    #[cfg(not(test))]
    {
        let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
        let anchor_addr = factory_config.atom_bluechip_anchor_pool_address.to_string();
        let eligible = get_eligible_creator_pools(deps, &anchor_addr)?;
        if eligible.len() < MIN_ELIGIBLE_POOLS_FOR_TWAP {
            return Err(StdError::generic_err(format!(
                "Oracle bootstrap: at least {} threshold-crossed creator pools are required before TWAP prices are served (currently {}).",
                MIN_ELIGIBLE_POOLS_FOR_TWAP,
                eligible.len()
            )));
        }
    }

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

/// Core conversion: when `to_usd` is true, converts bluechip→USD; otherwise USD→bluechip.
fn convert_with_oracle(
    deps: Deps,
    env: &Env,
    amount: Uint128,
    to_usd: bool,
) -> StdResult<ConversionResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let cached_price = get_bluechip_usd_price(deps, env)?;

    if cached_price.is_zero() {
        return Err(StdError::generic_err("Invalid zero price"));
    }

    let (numerator, denominator) = if to_usd {
        (cached_price, Uint128::from(PRICE_PRECISION))
    } else {
        (Uint128::from(PRICE_PRECISION), cached_price)
    };
    let direction = if to_usd {
        "bluechip to USD"
    } else {
        "USD to bluechip"
    };

    let converted = amount
        .checked_mul(numerator)
        .map_err(|e| StdError::generic_err(format!("Overflow in {} conversion: {}", direction, e)))?
        .checked_div(denominator)
        .map_err(|e| {
            StdError::generic_err(format!("Division error in {} conversion: {}", direction, e))
        })?;

    Ok(ConversionResponse {
        amount: converted,
        rate_used: cached_price,
        timestamp: oracle.bluechip_price_cache.last_update,
    })
}

pub fn bluechip_to_usd(
    deps: Deps,
    bluechip_amount: Uint128,
    env: Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, &env, bluechip_amount, true)
}

pub fn usd_to_bluechip(deps: Deps, usd_amount: Uint128, env: Env) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, &env, usd_amount, false)
}

pub fn get_price_with_staleness_check(
    deps: Deps,
    env: Env,
    max_staleness: u64,
) -> StdResult<Uint128> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();

    if current_time
        > oracle
            .bluechip_price_cache
            .last_update
            .saturating_add(max_staleness)
    {
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

// Force-rotate uses the same 48h timelock as every other admin-initiated
// state change. Re-exported here for backward compatibility with callers
// that referenced the old name; new code should use
// `crate::state::ADMIN_TIMELOCK_SECONDS` directly.
pub use crate::state::ADMIN_TIMELOCK_SECONDS as FORCE_ROTATE_TIMELOCK_SECONDS;

pub fn execute_propose_force_rotate_pools(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if info.sender != config.factory_admin_address {
        return Err(ContractError::Unauthorized {});
    }

    if crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .is_some()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "A force-rotate is already pending. Cancel it first.",
        )));
    }

    let effective_after = env.block.time.plus_seconds(FORCE_ROTATE_TIMELOCK_SECONDS);
    crate::state::PENDING_ORACLE_ROTATION.save(deps.storage, &effective_after)?;

    Ok(Response::new()
        .add_attribute("action", "propose_force_rotate_pools")
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_cancel_force_rotate_pools(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if info.sender != config.factory_admin_address {
        return Err(ContractError::Unauthorized {});
    }

    if crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .is_none()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending force-rotate to cancel",
        )));
    }

    crate::state::PENDING_ORACLE_ROTATION.remove(deps.storage);

    Ok(Response::new().add_attribute("action", "cancel_force_rotate_pools"))
}

pub fn execute_force_rotate_pools(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if info.sender != config.factory_admin_address {
        return Err(ContractError::Unauthorized {});
    }

    // Must have gone through the 48h propose/wait flow.
    let effective_after = crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .ok_or_else(|| {
            ContractError::Std(StdError::generic_err(
                "No pending force-rotate; call ProposeForceRotateOraclePools first",
            ))
        })?;

    if env.block.time < effective_after {
        return Err(ContractError::TimelockNotExpired { effective_after });
    }

    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let new_pools = select_random_pools_with_atom(deps.as_ref(), env.clone(), ORACLE_POOL_COUNT)?;
    oracle.selected_pools = new_pools.clone();
    oracle.last_rotation = env.block.time.seconds();

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    crate::state::PENDING_ORACLE_ROTATION.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "force_rotate_pools")
        .add_attribute("pools_count", new_pools.len().to_string()))
}
