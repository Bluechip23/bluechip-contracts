#[cfg(not(test))]
use crate::pyth_types::{PriceFeedResponse, PythQueryMsg};

use crate::state::{
    EligiblePoolSnapshot, ELIGIBLE_POOL_REFRESH_BLOCKS, ELIGIBLE_POOL_SNAPSHOT,
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
use pool_factory_interfaces::{ConversionResponse, PoolKind, PoolQueryMsg, PoolStateResponseForFactory};
use sha2::{Digest, Sha256};
#[cfg(test)]
pub const MOCK_PYTH_PRICE: Item<Uint128> = Item::new("mock_pyth_price");
// When set to true in tests, query_pyth_atom_usd_price returns Err,
// letting tests exercise the cache-fallback branch of get_bluechip_usd_price.
#[cfg(test)]
pub const MOCK_PYTH_SHOULD_FAIL: Item<bool> = Item::new("mock_pyth_should_fail");

/// Target number of pools sampled per oracle rotation (plus the anchor
/// ATOM/bluechip pool, for a total of `ORACLE_POOL_COUNT + 1` pools).
/// Sampling draws from the cached eligible-pool snapshot, so this count
/// no longer scales cost linearly with the full pool set — only with the
/// per-sample cross-contract queries inside `calculate_weighted_price_with_atom`.
pub const ORACLE_POOL_COUNT: usize = 75;
pub const MIN_POOL_LIQUIDITY: Uint128 = Uint128::new(10_000_000_000);
pub const TWAP_WINDOW: u64 = 3600;
pub const UPDATE_INTERVAL: u64 = 300;
pub const ROTATION_INTERVAL: u64 = 3600;

/// TWAP circuit breaker. Maximum allowed drift between the previously-cached
/// `bluechip_price_cache.last_price` and the freshly-computed TWAP, in basis
/// points. Rejects any oracle update where the new TWAP differs from the
/// prior by more than this threshold.
///
/// 3000 bps = 30%. Sized for early-ecosystem volatility on a low-liquidity
/// token: a real ±30% move per 300s `UPDATE_INTERVAL` is extreme but not
/// unheard-of around exchange listings, large-cap announcements, or genuine
/// market shocks. Anything above 30% per 5 minutes is overwhelmingly more
/// likely to be a manipulation attempt or upstream feed glitch than a real
/// market move; we'd rather freeze the oracle and force a human to look
/// than let an obviously-wrong price flow into commit USD valuations.
///
/// Skipped on the first update (when prior == 0) so genuine bootstrap
/// values can land. Recovery from a tripped breaker: wait for the
/// underlying spot pools to arb back to a sane range, or admin can
/// `ProposeForceRotateOraclePools` to swap out a manipulated pool from
/// the sample set.
pub const MAX_TWAP_DRIFT_BPS: u64 = 3000;

// Aspirational floor: the number of threshold-crossed creator pools the
// design intends to require before the TWAP is meaningfully diversified
// across multiple pools (in addition to the anchor ATOM/bluechip pool).
//
// IMPORTANT — NOT ENFORCED: this constant is referenced only by the
// bootstrap-acceptance comment block in `get_bluechip_usd_price_with_meta`
// (see lines ~1075-1100). The oracle does NOT currently refuse to serve
// a price when fewer than this many creator pools are eligible. We
// explicitly accept a single-pool-dominated price during the bootstrap
// window because every commit needs an oracle price to compute its USD
// value, but no creator pool can cross its threshold until commits
// succeed — enforcing the floor would deadlock the protocol on day one.
//
// Defense-in-depth that bounds the bootstrap manipulation risk:
//   - `MIN_POOL_LIQUIDITY` (line 31) raises the cost of moving the anchor.
//   - The anchor pool is curated and seeded by the deployment team.
//   - The H11 TWAP circuit breaker caps per-update drift to
//     `MAX_TWAP_DRIFT_BPS` (30%) on every update *after* the first.
//   - Downstream consumers (commit, swap) layer their own slippage and
//     spread protections.
//
// The risk window is "first oracle update plus the few updates until
// MIN_ELIGIBLE_POOLS_FOR_TWAP creator pools have crossed threshold." If
// you ever do want this to be a hard floor, the place to enforce it is
// in `calculate_weighted_price_with_atom` (return `InsufficientData` when
// `successful_pools < MIN_ELIGIBLE_POOLS_FOR_TWAP`) plus a bootstrap-mode
// switch on the price reader so the protocol can still launch.
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
    /// Warm-up gate (H-2). Number of additional successful TWAP observations
    /// the oracle still needs before downstream price queries
    /// (`get_bluechip_usd_price_with_meta`) will serve a price. Set to a
    /// non-zero value whenever the price cache is reset (anchor change,
    /// timelocked config update that swaps the anchor) so the very-first
    /// post-reset observation cannot be locked in as the canonical price
    /// by an attacker who briefly perturbed the new anchor's reserves.
    /// Decremented by one per successful price-publishing oracle update;
    /// failed (snapshot-only) updates do NOT decrement it because they
    /// don't advance the TWAP. While `> 0`, downstream conversions
    /// return `Err(InsufficientData)`. `#[serde(default)]` keeps oracle
    /// records written before this field existed deserializing as zero
    /// (no warm-up active), preserving the post-migration invariant
    /// that running pre-warm-up oracles continue to serve prices.
    #[serde(default)]
    pub warmup_remaining: u32,
}

/// Number of successful price-publishing oracle updates required after the
/// price cache is reset (anchor change) before downstream conversions
/// resume. With UPDATE_INTERVAL = 300s, this is 6 × 5min = 30 min of
/// real cumulative-delta evidence before any commit/swap can be priced
/// against the new anchor. Sized so a sustained ~30-min spot perturbation
/// would be required to bias the warm-up TWAP — a much larger commitment
/// than the prior single-block manipulation window.
pub const ANCHOR_CHANGE_WARMUP_OBSERVATIONS: u32 = 6;
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

/// Rebuild `ELIGIBLE_POOL_SNAPSHOT` if the current snapshot is missing or
/// older than `ELIGIBLE_POOL_REFRESH_BLOCKS`. No-op otherwise. Called once
/// from inside `select_random_pools_with_atom` at the top of each sample
/// attempt; amortizes the O(N) `POOLS_BY_ID` scan to once per ≈5 days even
/// if the oracle rotates hourly.
///
/// Captures `(address, bluechip_index)` pairs so the per-sample lookup at
/// `calculate_weighted_price_with_atom` time is O(1) instead of O(N) — see
/// `EligiblePoolSnapshot` doc.
fn refresh_eligible_pool_snapshot_if_stale(
    deps: &mut DepsMut,
    env: &Env,
    atom_pool_contract_address: &str,
) -> StdResult<()> {
    let current_block = env.block.height;
    let is_stale = match ELIGIBLE_POOL_SNAPSHOT.may_load(deps.storage)? {
        Some(snap) => current_block.saturating_sub(snap.captured_at_block)
            >= ELIGIBLE_POOL_REFRESH_BLOCKS,
        None => true,
    };
    if !is_stale {
        return Ok(());
    }
    let (pool_addresses, bluechip_indices) =
        get_eligible_creator_pools(deps.as_ref(), atom_pool_contract_address)?;
    ELIGIBLE_POOL_SNAPSHOT.save(
        deps.storage,
        &EligiblePoolSnapshot {
            pool_addresses,
            bluechip_indices,
            captured_at_block: current_block,
        },
    )?;
    Ok(())
}

pub fn select_random_pools_with_atom(
    mut deps: DepsMut,
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

    // Real Network Logic. Rebuild the eligible-pool snapshot at most once
    // per ELIGIBLE_POOL_REFRESH_BLOCKS (≈5 days); between refreshes the
    // sampler reads from the snapshot instead of scanning POOLS_BY_ID.
    refresh_eligible_pool_snapshot_if_stale(
        &mut deps,
        &env,
        &atom_pool_contract_contract_address,
    )?;
    let eligible_pools = ELIGIBLE_POOL_SNAPSHOT
        .load(deps.storage)?
        .pool_addresses;
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
                warmup_remaining: 0,
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
    mut deps: DepsMut,
    env: Env,
) -> Result<Response, ContractError> {
    let selected_pools =
        select_random_pools_with_atom(deps.branch(), env.clone(), ORACLE_POOL_COUNT)?;
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
        // Initial bootstrap warm-up. Treated identically to an anchor
        // change: the very first observations carry no historical TWAP
        // weight, so we refuse to serve a price downstream until enough
        // real cumulative-delta evidence has accumulated.
        warmup_remaining: ANCHOR_CHANGE_WARMUP_OBSERVATIONS,
    };

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    Ok(Response::new())
}

/// Returns (eligible_addresses, bluechip_indices). Each address at index
/// `i` has bluechip on reserve-side `bluechip_indices[i]` (0 or 1).
/// Hoisting this into the snapshot is what makes oracle updates O(1) per
/// sampled pool instead of O(N).
pub fn get_eligible_creator_pools(
    deps: Deps,
    atom_pool_contract_address: &str,
) -> StdResult<(Vec<String>, Vec<u8>)> {
    // Return every pool eligible for oracle sampling. A pool is eligible iff:
    //   1. It is a commit pool (NOT a standard pool — standard pools can
    //      hold arbitrary pairs including non-bluechip ones and their price
    //      isn't meaningful for bluechip/USD derivation).
    //   2. It contains a bluechip token (so we can price it against ATOM).
    //   3. It has crossed its commit threshold (POOL_THRESHOLD_MINTED == true).
    //   4. Its current reserves sum to >= MIN_POOL_LIQUIDITY.
    //
    // The threshold-crossed gate is the important one: pool creation is
    // permissionless, so without this check a spammer could bloat the oracle
    // sample set with pre-threshold pools. The MIN_POOL_LIQUIDITY check is
    // defense-in-depth for pools that crossed threshold but later drained.
    //
    // Single pass over POOLS_BY_ID: for each candidate we check the cheap
    // in-storage gates first and only incur the cross-contract
    // PoolStateResponseForFactory query when they all pass. The older
    // implementation did two full range scans plus a HashSet build, which
    // dominated oracle-update gas at scale.
    let mut eligible = Vec::new();
    let mut indices = Vec::new();
    for row in POOLS_BY_ID.range(deps.storage, None, None, Order::Ascending) {
        let (pool_id, pool_details) = row?;

        if pool_details.creator_pool_addr.as_str() == atom_pool_contract_address {
            continue;
        }
        // Standard pools are never eligible for TWAP sampling — see gate (1) above.
        if pool_details.pool_kind == PoolKind::Standard {
            continue;
        }
        // Resolve bluechip side once at snapshot capture time. Commit pools
        // are validated at instantiate to contain exactly one Bluechip and
        // one CreatorToken, so this find always succeeds for eligible pools.
        let bluechip_idx = match pool_details
            .pool_token_info
            .iter()
            .position(|t| matches!(t, TokenType::Native { .. }))
        {
            Some(i) => i as u8,
            None => continue, // No bluechip side — gate (2) fails.
        };
        if !POOL_THRESHOLD_MINTED
            .may_load(deps.storage, pool_id)?
            .unwrap_or(false)
        {
            continue;
        }

        let pool_state: PoolStateResponseForFactory = deps.querier.query_wasm_smart(
            pool_details.creator_pool_addr.to_string(),
            &PoolQueryMsg::GetPoolState {},
        )?;

        let total_liquidity = pool_state.reserve0.saturating_add(pool_state.reserve1);
        if total_liquidity >= MIN_POOL_LIQUIDITY {
            eligible.push(pool_details.creator_pool_addr.to_string());
            indices.push(bluechip_idx);
        }
    }
    Ok((eligible, indices))
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
    mut deps: DepsMut,
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
            select_random_pools_with_atom(deps.branch(), env.clone(), ORACLE_POOL_COUNT)?;
        oracle.selected_pools = pools_to_use.clone();
        oracle.last_rotation = current_time;
        // Retain snapshots only for pools that remain in the new selection to preserve TWAP continuity
        oracle
            .pool_cumulative_snapshots
            .retain(|s| pools_to_use.contains(&s.pool_address));
    }
    let (maybe_weighted_price, maybe_atom_price, new_snapshots) =
        calculate_weighted_price_with_atom(
            deps.as_ref(),
            &pools_to_use,
            &oracle.pool_cumulative_snapshots,
        )?;
    // Always persist the new snapshots so the next round has prior data
    // to compute a TWAP from, even when this round couldn't produce a
    // price (bootstrap, anchor inactive, etc.). Pre-H-3 code returned
    // Err on those paths and reverted the snapshot save, leaving the
    // oracle stuck.
    oracle.pool_cumulative_snapshots = new_snapshots;

    let (weighted_price, atom_price) = match (maybe_weighted_price, maybe_atom_price) {
        (Some(w), Some(a)) => (w, a),
        _ => {
            // No TWAP could be produced this round. Snapshots have already
            // been recorded; persist them and return success so the next
            // oracle update has prior data. Don't push an observation,
            // don't decrement warmup_remaining, don't pay the keeper
            // bounty (snapshot-only updates aren't price-publishing
            // work). The pool-side staleness gate
            // (MAX_ORACLE_STALENESS_SECONDS) will eventually start
            // rejecting commits if the no-TWAP condition persists; that
            // fail-closed behaviour is the intended pressure on
            // operators to investigate.
            INTERNAL_ORACLE.save(deps.storage, &oracle)?;
            return Ok(Response::new()
                .add_attribute("action", "update_oracle")
                .add_attribute("price_published", "false")
                .add_attribute(
                    "reason",
                    "insufficient_twap_data_snapshots_recorded_for_next_round",
                )
                .add_attribute("pools_used", pools_to_use.len().to_string()));
        }
    };

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

    // Circuit breaker. If the freshly-computed TWAP has drifted more than
    // MAX_TWAP_DRIFT_BPS from the previously-cached `last_price`, reject
    // this update entirely. The full tx reverts (push to twap_observations,
    // pyth cache update, etc.) so the next caller sees the prior cached
    // state and gets to retry against fresh observations. See
    // `MAX_TWAP_DRIFT_BPS` doc for sizing rationale and recovery path.
    //
    // First-update bootstrap (`prior == 0`) bypasses the check so the
    // very first observation can land. After that, every subsequent
    // update is rate-limited to the bps threshold per UPDATE_INTERVAL.
    let prior = oracle.bluechip_price_cache.last_price;
    if !prior.is_zero() {
        let (smaller, larger) = if twap_price > prior {
            (prior, twap_price)
        } else {
            (twap_price, prior)
        };
        let diff = larger.checked_sub(smaller)?;
        // Saturate any overflow in the bps ratio to "definitely tripped"
        // — if `diff * 10_000` overflows u128, the drift is astronomically
        // larger than MAX_TWAP_DRIFT_BPS and the breaker should fire
        // unconditionally.
        let drift_bps_u128 = match diff.checked_mul(Uint128::from(10_000u128)) {
            Ok(scaled) => scaled
                .checked_div(smaller)
                .map(|v| v.u128())
                .unwrap_or(u128::MAX),
            Err(_) => u128::MAX,
        };
        if drift_bps_u128 > MAX_TWAP_DRIFT_BPS as u128 {
            return Err(ContractError::TwapCircuitBreaker {
                prior,
                new: twap_price,
                drift_bps: drift_bps_u128,
                max_bps: MAX_TWAP_DRIFT_BPS,
            });
        }
    }

    oracle.bluechip_price_cache.last_price = twap_price;
    oracle.bluechip_price_cache.last_update = current_time;

    // Cache the Pyth ATOM/USD price alongside the TWAP update
    if let Ok(pyth_price) = query_pyth_atom_usd_price(deps.as_ref(), &env) {
        oracle.bluechip_price_cache.cached_pyth_price = pyth_price;
        oracle.bluechip_price_cache.cached_pyth_timestamp = current_time;
    }

    // Decrement the H-2 warm-up counter. Only price-publishing updates
    // count — snapshot-only updates returned earlier and don't tick
    // this down, otherwise an attacker could exhaust the warm-up by
    // triggering empty rounds.
    let warmup_remaining_before = oracle.warmup_remaining;
    oracle.warmup_remaining = oracle.warmup_remaining.saturating_sub(1);

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
        .add_attribute("pools_used", pools_to_use.len().to_string())
        .add_attribute(
            "warmup_remaining_before",
            warmup_remaining_before.to_string(),
        )
        .add_attribute(
            "warmup_remaining_after",
            oracle.warmup_remaining.to_string(),
        );

    if !bounty_usd.is_zero() {
        // Convert USD -> bluechip via the just-updated TWAP. If the
        // conversion errors (Pyth + cache both unavailable), skip the
        // bounty rather than reverting the whole oracle update.
        match usd_to_bluechip(deps.as_ref(), bounty_usd, &env) {
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

/// O(M) lookup of the bluechip-side index for `pool_address` in the
/// eligible-pool snapshot, where M is the snapshot size (≤ a few thousand).
/// Returns `None` if the snapshot is missing, the address isn't in the
/// snapshot (e.g., it's the anchor), or the indices array is shorter than
/// the addresses array (a snapshot written by pre-cache code).
///
/// The linear search is fine here even at large M: it runs once per sampled
/// pool, vs the prior O(N) full POOLS_BY_ID range scan which deserialized
/// every pool record. Snapshot entries are just `String + u8`, so a 1000-pool
/// snapshot scan is ~16 KB of memory comparison vs the storage-deserializing
/// scan it replaced.
fn bluechip_index_lookup(deps: Deps, pool_address: &str) -> StdResult<Option<u8>> {
    let snap = match ELIGIBLE_POOL_SNAPSHOT.may_load(deps.storage)? {
        Some(s) => s,
        None => return Ok(None),
    };
    if snap.bluechip_indices.len() != snap.pool_addresses.len() {
        // Pre-cache snapshot — caller should fall back to the scan.
        return Ok(None);
    }
    Ok(snap
        .pool_addresses
        .iter()
        .position(|addr| addr == pool_address)
        .map(|i| snap.bluechip_indices[i]))
}

// Calculates a liquidity-weighted price across sampled pools using cumulative
// TWAPs. Returns `(maybe_weighted_price, maybe_atom_price, new_snapshots)`:
//
//   - `maybe_*` are `None` whenever this round can't produce a real TWAP
//     (bootstrap / no-anchor-activity / no successful creator pools). In
//     that case the oracle update handler must SAVE `new_snapshots` and
//     skip the observation push so the next round has fresh prior data
//     to compute a TWAP from. The previous Err-on-insufficient-data
//     behaviour reverted the whole tx and discarded snapshots, leaving
//     the oracle permanently unable to bootstrap once spot fallbacks
//     were removed.
//
//   - `new_snapshots` is always populated for every sampled pool that
//     answered a `GetPoolState` query and met `MIN_POOL_LIQUIDITY`,
//     regardless of whether its price contributed to the weighted sum
//     this round.
//
// SPOT PRICE IS NEVER USED. All three former spot-fallback branches
// (anchor-stale-cumulative, bootstrap, anchor-missing-from-prev) now
// `continue` instead. A single-block `reserve0/reserve1` read is trivially
// manipulable by a sufficiently-funded attacker; rather than mixing it
// into the TWAP and contaminating downstream USD conversions for the
// next ~1h TWAP_WINDOW, we refuse to publish until the AMM has produced
// real cumulative-delta evidence over a real time window.
pub fn calculate_weighted_price_with_atom(
    deps: Deps,
    pool_addresses: &[String],
    prev_snapshots: &[PoolCumulativeSnapshot],
) -> Result<(Option<Uint128>, Option<Uint128>, Vec<PoolCumulativeSnapshot>), ContractError> {
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

                // Determine if Bluechip is reserve0 or reserve1. Hoisted out
                // of an O(N) scan of `POOLS_BY_ID` (formerly run per-sample;
                // amounted to O(N²) per oracle update at scale). The
                // eligible-pool snapshot now caches the bluechip-side index
                // alongside each address, populated at capture time in
                // `get_eligible_creator_pools`.
                //
                // Falls back to the old linear scan only on:
                //   - the anchor pool (which isn't in the snapshot — it's
                //     always added separately by `select_random_pools_with_atom`)
                //   - a snapshot written by pre-cache code that had no
                //     `bluechip_indices` populated (`#[serde(default)]`
                //     produces an empty Vec; one-time, until next refresh)
                let is_bluechip_second = if let Some(idx) =
                    bluechip_index_lookup(deps, pool_address)?
                {
                    idx == 1
                } else {
                    // Anchor pool or stale snapshot — fall back to scan.
                    //
                    // Two pool shapes can land here:
                    //   (a) Creator pool: `[Native(bluechip), CreatorToken(...)]`.
                    //       `pool_token_info[0]` is `Native`, so the `Native` arm
                    //       fires and returns `false` (bluechip is at index 0).
                    //   (b) Anchor pool: `[Native(bluechip), Native(atom)]` OR
                    //       `[Native(atom), Native(bluechip)]` — `execute_set_anchor_pool`
                    //       accepts either order. Match the index-0 denom against
                    //       the canonical bluechip denom; if it matches, bluechip
                    //       is first; otherwise (i.e. atom is first), bluechip
                    //       is second.
                    //
                    // The pre-fix `matches!(... CreatorToken { .. })` pattern was
                    // structurally `false` for any Native/Native pair, which silently
                    // inverted the anchor's reserve selection whenever the operator
                    // created the anchor with `[Native(atom), Native(bluechip)]`.
                    let canonical_bluechip = factory_config.bluechip_denom.as_str();
                    let mut found = false;
                    for (_id, pool_details) in POOLS_BY_ID
                        .range(deps.storage, None, None, Order::Ascending)
                        .flatten()
                    {
                        if pool_details.creator_pool_addr.as_str() == pool_address.as_str() {
                            found = match &pool_details.pool_token_info[0] {
                                TokenType::CreatorToken { .. } => true,
                                TokenType::Native { denom } => denom != canonical_bluechip,
                            };
                            break;
                        }
                    }
                    found
                };

                // Resolve bluechip reserve based on token ordering. The
                // other-side reserve is no longer needed (former spot
                // fallbacks consumed it; H-3 removed all of them).
                let bluechip_reserve = if is_bluechip_second {
                    pool_state.reserve1
                } else {
                    pool_state.reserve0
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

                // No spot fallback anywhere — every branch that previously
                // fell back to `calculate_price_from_reserves` now `continue`s
                // and lets this round produce no price for that pool. The
                // snapshot above still lands so the next round has prior
                // data even though we don't publish today. `is_anchor`
                // distinction was previously needed to gate the spot
                // fallback; with all spot paths removed it's no longer
                // needed inside the price-derivation block.
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
                    } else {
                        // No cumulative-delta evidence this round (no swap
                        // since the last sample). Skip — including the
                        // anchor. The previous spot-fallback branch let an
                        // attacker who could move anchor reserves for one
                        // block dictate the published price; we'd rather
                        // refuse to publish than serve a manipulable read.
                        continue;
                    }
                } else {
                    // No prior snapshot for this pool — either the very
                    // first oracle update or a freshly-rotated pool. Skip
                    // (snapshot was just recorded above, so the next round
                    // can compute a real TWAP). The prior bootstrap and
                    // anchor-missing-from-prev branches both used spot
                    // here; both removed.
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

    // No anchor TWAP this round — return None for prices but KEEP the
    // populated `new_snapshots` so the caller can persist them and the
    // next round has prior data to compute a TWAP from. Returning Err
    // here would revert the snapshots and leave the oracle permanently
    // stuck at bootstrap.
    if !has_atom_pool || successful_pools == 0 || total_weight.is_zero() {
        return Ok((None, None, new_snapshots));
    }
    let weighted_average = weighted_sum
        .checked_div(total_weight)
        .map_err(|_| ContractError::Std(StdError::generic_err("Division by zero")))?;

    let final_price = Uint128::try_from(weighted_average)
        .map_err(|_| ContractError::Std(StdError::generic_err("Price conversion overflow")))?;

    Ok((Some(final_price), Some(atom_pool_price), new_snapshots))
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

        // Partial-move feed id and pyth contract address out of factory:
        // both are used at most twice (once for the standard query, once
        // again only on the mock-oracle fallback path) and both consumers
        // need owned `String`. Owning them locally lets the fallback
        // branch reuse them by move instead of cloning a second time.
        let feed_id = factory.pyth_atom_usd_price_feed_id;
        let pyth_addr = factory.pyth_contract_addr_for_conversions;

        let query_msg = PythQueryMsg::PythConversionPriceFeed {
            id: feed_id.clone(),
        };

        // M-7: the `GetPrice` fallback is only meaningful for the mock
        // oracle (which is selected via the `mock` cargo feature). In
        // production a Pyth query failure must surface as `Err` so the
        // cache-fallback path inside `get_bluechip_usd_price_with_meta`
        // can decide whether to bridge the outage from the cached price
        // or refuse to serve. Silently falling back to a different RPC
        // shape on the same contract previously meant that an operator
        // who accidentally pointed `pyth_contract_addr_for_conversions`
        // at a mock-flavoured oracle in production would silently
        // receive mock prices.
        //
        // Behaviour by build flavour:
        //   - prod (default): error propagates → caller's cache
        //     fallback fires.
        //   - `mock` feature: keep the GetPrice fallback so the test
        //     mockoracle keeps working.
        #[cfg(not(feature = "mock"))]
        let response: PriceFeedResponse = {
            let _ = feed_id; // silence unused-variable in prod build
            deps.querier.query_wasm_smart(pyth_addr, &query_msg)?
        };
        #[cfg(feature = "mock")]
        let response: PriceFeedResponse =
            match deps.querier.query_wasm_smart(pyth_addr.clone(), &query_msg) {
                Ok(res) => res,
                Err(_) => {
                    let fallback_msg = PythQueryMsg::GetPrice { price_id: feed_id };
                    deps.querier.query_wasm_smart(pyth_addr, &fallback_msg)?
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

        // Derive `price_u128` from the already-validated `price_u64` rather
        // than re-casting `price_data.price` (i64) directly. If a future edit
        // ever drops or reorders the negative-price guard above, this chain
        // would still produce a typed runtime error from `try_into::<u64>`
        // rather than silently sign-extending a negative i64 into the high
        // bits of u128 and passing every later check vacuously.
        let price_u128: u128 = price_u64.into();
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

/// Internal: returns the bluechip USD price together with the oracle's
/// `last_update` timestamp from a single load of INTERNAL_ORACLE. The
/// conversion wrappers (`bluechip_to_usd` / `usd_to_bluechip`) need both
/// values to populate `ConversionResponse.timestamp`, and the cache
/// fallback path needs the cache to authorize the stale-pyth bridge —
/// so loading the oracle once and reusing it both for the cache check
/// and for the TWAP read avoids the prior 2× / 3× re-deserialization.
fn get_bluechip_usd_price_with_meta(deps: Deps, env: &Env) -> StdResult<(Uint128, u64)> {
    // Single load of INTERNAL_ORACLE shared by both the Pyth-fallback
    // branch (which reads `bluechip_price_cache`) and the post-Pyth TWAP
    // computation (which reads `bluechip_price_cache.last_price`).
    let oracle = INTERNAL_ORACLE
        .load(deps.storage)
        .map_err(|_| StdError::generic_err("Internal oracle not initialized"))?;

    // H-2 warm-up gate. After bootstrap or any anchor change the oracle
    // cache is reset, and the very-first post-reset observation is
    // single-block-manipulable. Refuse to serve a price downstream
    // until ANCHOR_CHANGE_WARMUP_OBSERVATIONS price-publishing updates
    // have accumulated. Commits/swaps revert during this window. Once
    // warmup_remaining hits zero, normal pricing resumes.
    if oracle.warmup_remaining > 0 {
        return Err(StdError::generic_err(format!(
            "Oracle warm-up in progress after anchor reset: {} more successful TWAP \
             updates required before pricing resumes (H-2)",
            oracle.warmup_remaining
        )));
    }

    let cache = &oracle.bluechip_price_cache;
    let last_update = cache.last_update;

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
        return Ok((atom_usd_price, last_update));
    }

    // Bootstrap note: when fewer than MIN_ELIGIBLE_POOLS_FOR_TWAP creator
    // pools have crossed threshold, `oracle.bluechip_price_cache.last_price`
    // is derived from the anchor ATOM/bluechip pool alone (plus whichever
    // creators have crossed, of which there are < 3). We accept that
    // single-pool-dominated price during bootstrap rather than bricking
    // every commit on day one — without this fallback the protocol
    // deadlocks on launch, because each commit requires an oracle price
    // to compute its USD value, but no pool can cross its threshold
    // until commits succeed.
    //
    // The trade-off: during bootstrap, a sophisticated attacker who can
    // move the anchor pool's price for a block (see the spot-fallback
    // in calculate_weighted_price_with_atom) can also move the derived
    // bluechip/USD rate. This risk is bounded by the anchor's curated
    // liquidity and ends as soon as MIN_ELIGIBLE_POOLS_FOR_TWAP creator
    // pools have crossed threshold. Callers downstream of this function
    // (commits, swaps) layer their own slippage / spread protections,
    // so the worst-case is a temporarily mispriced commit rather than
    // direct theft.
    //
    // The staleness check still applies via `last_update` — the pool's
    // get_oracle_conversion_with_staleness rejects commits if the cached
    // price is older than MAX_ORACLE_STALENESS_SECONDS. And the zero
    // guard below catches the pre-first-update case where UpdateOraclePrice
    // has never been called.
    #[cfg(not(feature = "mock"))]
    {
        let bluechip_per_atom_twap = cache.last_price;

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

        Ok((bluechip_usd_price, last_update))
    }
}

pub fn get_bluechip_usd_price(deps: Deps, env: &Env) -> StdResult<Uint128> {
    get_bluechip_usd_price_with_meta(deps, env).map(|(price, _)| price)
}

/// Core conversion: when `to_usd` is true, converts bluechip→USD; otherwise USD→bluechip.
fn convert_with_oracle(
    deps: Deps,
    env: &Env,
    amount: Uint128,
    to_usd: bool,
) -> StdResult<ConversionResponse> {
    // Single oracle load — `get_bluechip_usd_price_with_meta` returns both
    // the price and the cache's `last_update`, so we no longer need a
    // separate `INTERNAL_ORACLE.load(...)` here just to populate the
    // response timestamp.
    let (cached_price, last_update) = get_bluechip_usd_price_with_meta(deps, env)?;

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
        timestamp: last_update,
    })
}

pub fn bluechip_to_usd(
    deps: Deps,
    bluechip_amount: Uint128,
    env: &Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, env, bluechip_amount, true)
}

pub fn usd_to_bluechip(
    deps: Deps,
    usd_amount: Uint128,
    env: &Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, env, usd_amount, false)
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

fn query_pool_safe(
    deps: Deps,
    pool_address: &str,
) -> Result<PoolStateResponseForFactory, ContractError> {
    #[cfg(not(test))]
    {
        deps.querier
            .query_wasm_smart(pool_address.to_string(), &PoolQueryMsg::GetPoolState {})
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
    mut deps: DepsMut,
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
    let new_pools =
        select_random_pools_with_atom(deps.branch(), env.clone(), ORACLE_POOL_COUNT)?;
    oracle.selected_pools = new_pools.clone();
    oracle.last_rotation = env.block.time.seconds();
    // M-8: clear cumulative snapshots and the price cache so the new
    // sample set starts from a clean slate. Pre-fix, force-rotate left
    // both intact:
    //   - Stale `pool_cumulative_snapshots` for pools no longer in
    //     `selected_pools` lingered in storage until the next periodic
    //     rotation.
    //   - The very next `update_internal_oracle_price` saw most newly-
    //     selected creator pools as having no prior snapshot and
    //     skipped them, leaving the anchor to dominate the TWAP for
    //     one cycle.
    //   - The retained `last_price` from the pre-rotation set (which
    //     may have been the very thing the operator was force-rotating
    //     to escape) anchored the circuit breaker on the next update.
    //
    // Treat force-rotate as a full oracle reset, identical to the
    // anchor-change path: clear snapshots + observations, zero
    // `last_price`/`last_update`, re-arm the H-2 warm-up gate so
    // downstream consumers refuse to serve a price until enough new
    // observations have accumulated.
    oracle.pool_cumulative_snapshots.clear();
    oracle.bluechip_price_cache.last_price = Uint128::zero();
    oracle.bluechip_price_cache.last_update = 0;
    oracle.bluechip_price_cache.twap_observations.clear();
    oracle.warmup_remaining = ANCHOR_CHANGE_WARMUP_OBSERVATIONS;

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    crate::state::PENDING_ORACLE_ROTATION.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "force_rotate_pools")
        .add_attribute("pools_count", new_pools.len().to_string())
        .add_attribute(
            "warmup_remaining",
            ANCHOR_CHANGE_WARMUP_OBSERVATIONS.to_string(),
        ))
}
