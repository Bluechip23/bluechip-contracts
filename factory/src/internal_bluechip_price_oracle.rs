#[cfg(not(test))]
use crate::pyth_types::{PriceFeedResponse, PythQueryMsg};

use crate::state::{
    EligiblePoolSnapshot, ELIGIBLE_POOL_REFRESH_BLOCKS, ELIGIBLE_POOL_SNAPSHOT,
    FACTORYINSTANTIATEINFO, ORACLE_UPDATE_BOUNTY_USD,
    POOLS_BY_ID, POOL_THRESHOLD_MINTED,
};
// `POOLS_BY_CONTRACT_ADDRESS` is read only by the `#[cfg(test)]` branch
// of `query_pool_safe` (the prod path goes through `deps.querier`).
// Gating the import on the same cfg avoids the unused-import warning in
// release builds while keeping the test path compiling unchanged.
#[cfg(test)]
use crate::state::POOLS_BY_CONTRACT_ADDRESS;
use crate::execute::ensure_admin;
use crate::{asset::TokenType, error::ContractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError,
    StdResult, Storage, Uint128, Uint256,
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

/// Cross-pool basket aggregation gate (audit C-1).
///
/// Set `false` for v1. Each AMM pool's TWAP yields a raw
/// `bluechip-per-non-bluechip-side` exchange rate (see
/// `packages/pool-core/src/swap.rs::update_price_accumulator`).
/// Averaging those rates across heterogeneous non-bluechip sides
/// (ATOM vs USDC vs OSMO vs creator token) without first normalizing
/// each pool's contribution to a shared unit (e.g. USD-per-bluechip)
/// produces a result with no economic interpretation. The downstream
/// consumer at `get_bluechip_usd_price_with_meta` reads `last_price`
/// as strictly `bluechip-per-ATOM`, so the only safe aggregation today
/// is "anchor only."
///
/// When this is set `true`, the eligible-pool sampling path turns on
/// and `calculate_weighted_price_with_atom` blends additional pools
/// into the weighted average. Re-enabling requires:
///   1. Each `AllowlistedOraclePool` carries a per-pool Pyth feed id
///      for the non-bluechip side.
///   2. `calculate_weighted_price_with_atom` converts every pool's
///      contribution to a USD-per-bluechip estimate via that pool's
///      Pyth feed before summing.
///   3. `last_price` semantics + the consumer in
///      `get_bluechip_usd_price_with_meta` align on whichever
///      representation the new aggregation produces (USD-per-bluechip
///      direct, or bluechip-per-ATOM via per-pool normalization).
///
/// Until those three are wired, every non-anchor pool added to the
/// allowlist would drag `last_price` away from the correct value, so
/// the anchor pool is the sole price source.
pub const ORACLE_BASKET_ENABLED: bool = false;

/// Hardcoded fallback bluechip-side floor used by `pool_meets_liquidity_floor`
/// when the oracle has no usable price (bootstrap window, breaker tripped,
/// post-anchor-change warm-up). Denominated in ubluechip — the bluechip-side
/// reserve must meet this on its own (per-side, not summed). 5_000_000_000
/// ubluechip = 5_000 BC, half of the legacy `MIN_POOL_LIQUIDITY` constant
/// which was applied to the SUM of both sides; the per-side equivalent that
/// a balanced pool must have met under the old code is therefore identical.
///
/// Mirrors the `STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP` pattern: a
/// known-conservative bluechip-denominated value used when the
/// USD-denominated source of truth can't be resolved.
pub const MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE: Uint128 =
    Uint128::new(5_000_000_000);
/// Legacy constant retained for one cycle of git-grep continuity. Callers
/// MUST use `pool_meets_liquidity_floor` instead — this constant is
/// intentionally inaccessible at runtime.
#[allow(dead_code)]
pub(crate) const MIN_POOL_LIQUIDITY: Uint128 = Uint128::new(10_000_000_000);

/// USD-denominated floor for total pool liquidity, enforced at oracle
/// sampling time. Scale matches `standard_pool_creation_fee_usd` and the
/// oracle's `bluechip_price_cache.last_price` numerator: 6-decimal USD
/// (so `5_000_000_000` = $5_000.00).
///
/// The total-USD floor is converted to a bluechip-side floor (= total/2,
/// since xyk pools have equal-USD sides at the spot-implied price) and
/// compared against the bluechip-side reserve. The bluechip side carrying
/// half the total USD acts as both the total liquidity gate AND the
/// per-side floor — a pool whose bluechip side meets `floor/2` cannot
/// be lopsided away from the spot equilibrium without arb pressure
/// closing the gap on the next block, which is the whole reason xyk
/// reserves stay near parity in USD terms.
///
/// Default sized for early-ecosystem standard pools (bluechip/USDC,
/// bluechip/OSMO, etc.) where ~$5k each side ≈ $10k total is the
/// minimum where a single-block reserve manipulation costs more than
/// the would-be attacker can recover even from a 30% TWAP move
/// (capped by `MAX_TWAP_DRIFT_BPS`).
pub const MIN_POOL_LIQUIDITY_USD: Uint128 = Uint128::new(5_000_000_000);

pub const TWAP_WINDOW: u64 = 3600;
pub const UPDATE_INTERVAL: u64 = 300;
pub const ROTATION_INTERVAL: u64 = 3600;

/// Liquidity-floor gate used by every eligible-pool path
/// (`get_eligible_creator_pools` for both sources, plus the per-sample
/// check inside `calculate_weighted_price_with_atom`).
///
/// Two-tier evaluation:
///
///   1. **USD-denominated path** (preferred). When
///      `INTERNAL_ORACLE.bluechip_price_cache.last_price` is non-zero,
///      converts `MIN_POOL_LIQUIDITY_USD` to bluechip via the cached
///      price and requires `bluechip_reserve >= floor_bluechip / 2`
///      (the per-side share of the total floor for an xyk pool at
///      spot equilibrium).
///
///   2. **Hardcoded fallback** (bootstrap / breaker / warm-up). When
///      the cached price is zero, falls back to
///      `MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE` so the gate
///      stays meaningful before the oracle has produced a usable USD
///      price. Sized to be conservative-equivalent to the legacy
///      "summed reserves >= MIN_POOL_LIQUIDITY" check on a balanced
///      pool, so existing deployments with no oracle data yet behave
///      no more permissively than before.
///
/// Reads cache directly rather than going through `usd_to_bluechip` so:
///   - No `Env` dependency at the callsite (avoids plumbing it
///     through `calculate_weighted_price_with_atom`).
///   - No warm-up gate (the floor check is informational — at worst
///     we admit a borderline pool for one round; the TWAP / breaker
///     handle the actual price math).
///   - No Pyth dependency (the floor is a function of the bluechip
///     side only; ATOM/USD doesn't enter).
pub fn pool_meets_liquidity_floor(
    storage: &dyn Storage,
    pool_state: &PoolStateResponseForFactory,
    bluechip_index: u8,
) -> StdResult<bool> {
    let bluechip_reserve = if bluechip_index == 0 {
        pool_state.reserve0
    } else {
        pool_state.reserve1
    };

    // `may_load` rather than `load`: this helper runs during the
    // instantiate path (initialize_internal_bluechip_oracle ->
    // select_random_pools_with_atom -> refresh_eligible_pool_snapshot ->
    // get_eligible_creator_pools), before INTERNAL_ORACLE has been
    // saved. A missing oracle is the same outcome as a zero `last_price`:
    // no usable USD reading, fall back to the hardcoded bluechip-side
    // floor.
    let last_price = INTERNAL_ORACLE
        .may_load(storage)?
        .map(|o| o.bluechip_price_cache.last_price)
        .unwrap_or_default();

    let floor_per_side = if last_price.is_zero() {
        // No usable USD price yet — fall back to the legacy bluechip-
        // denominated per-side floor.
        MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE
    } else {
        // floor_bluechip = MIN_POOL_LIQUIDITY_USD * PRICE_PRECISION / last_price
        // floor_per_side = floor_bluechip / 2
        // checked_mul/div all return StdError on overflow/division by zero;
        // last_price is non-zero by branch guard, so the div can't fault.
        let total_bluechip = MIN_POOL_LIQUIDITY_USD
            .checked_mul(Uint128::from(PRICE_PRECISION))
            .map_err(|e| {
                StdError::generic_err(format!(
                    "pool_meets_liquidity_floor: overflow scaling USD floor: {e}"
                ))
            })?
            .checked_div(last_price)
            .map_err(|e| {
                StdError::generic_err(format!(
                    "pool_meets_liquidity_floor: divide-by-zero converting USD floor: {e}"
                ))
            })?;
        total_bluechip
            .checked_div(Uint128::from(2u128))
            .map_err(|e| {
                StdError::generic_err(format!(
                    "pool_meets_liquidity_floor: halving total floor: {e}"
                ))
            })?
    };

    Ok(bluechip_reserve >= floor_per_side)
}

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
/// IMPORTANT — TWAP smoothing dilutes single-observation drift.
/// The branch-(a) check compares `last_price` against `twap_price`,
/// which is the time-weighted average over `TWAP_WINDOW` (1h). When
/// the observation window already holds enough history to be near
/// steady state, a single raw observation that exceeds ±30% is
/// flattened by the average and the breaker fires only on aggregate
/// drift > 30%. But immediately after a reset
/// (post-`ConfirmBootstrapPrice`, post-`SetAnchorPool`,
/// post-`ForceRotateOraclePools`) the window holds only the one
/// confirmed/published observation, so the next round's TWAP is
/// effectively `(confirmed + new_observation) / 2`. In that
/// configuration:
///
///   - drift_bps_saturating(twap_price, last_price)
///   - == |((confirmed + new)/2) - confirmed| / confirmed * 10_000
///   - == |new - confirmed| / 2 / confirmed * 10_000
///
/// so a single raw observation up to ±~60% of the confirmed price
/// produces an effective TWAP drift of ±~30% and is admitted. The
/// breaker therefore behaves as a ~30% **per-round-aggregate** cap, not
/// a ~30% **per-raw-observation** cap. Two consecutive rounds at the
/// margin can compound to a larger total move than the per-round
/// nominal figure suggests.
///
/// Defense in depth that makes this acceptable:
///   - Each "observation" is a TWAP-from-cumulative-deltas read of
///     the anchor pool, NOT a single-block spot read. Moving it
///     requires real swap flow that the pool accumulator records —
///     capital committed over the full `UPDATE_INTERVAL` (300s)
///     rather than a single-block reserve perturbation.
///   - `ANCHOR_CHANGE_WARMUP_OBSERVATIONS = 6` rounds (~30 min) of
///     successful publishes are required before strict downstream
///     callers (commit valuation) will serve a price after any reset.
///     During this window the post-reset buffer (branch b/c) requires
///     two consecutive observations to drift-check against each
///     other and publish the median; the dilution analysis above
///     only applies once branch (a) is live again.
///   - Subsequent branch-(a) rounds accumulate more observations in
///     the window, increasing the dilution and tightening the
///     effective raw-observation tolerance back toward the nominal
///     30%.
///
/// Skipped on the first update (when prior == 0) so genuine bootstrap
/// values can land. Recovery from a tripped breaker: wait for the
/// underlying spot pools to arb back to a sane range, or admin can
/// `ProposeForceRotateOraclePools` to swap out a manipulated pool from
/// the sample set.
pub const MAX_TWAP_DRIFT_BPS: u64 = 3000;

/// Basis-points scale for the drift ratio (10_000 = 100%).
pub const BPS_SCALE: u128 = 10_000;

/// Saturating drift-in-bps between two prices.
///
/// Returns `|a - b| * 10_000 / min(a, b)` clamped to `u128::MAX` on any
/// overflow OR division-by-zero. Used by the TWAP circuit-breaker
/// branches; saturating semantics map "math overflowed" to "definitely
/// tripped" so the breaker fires unconditionally rather than silently
/// wrapping. The (0, 0) case lands in the saturating div-by-zero branch
/// and also returns `u128::MAX` — production callers gate the drift
/// check on `prior != 0` separately so this is unreachable in practice,
/// but the fail-safe direction is preserved here as belt-and-braces.
pub fn drift_bps_saturating(a: Uint128, b: Uint128) -> u128 {
    let (smaller, larger) = if a > b { (b, a) } else { (a, b) };
    let diff = larger.saturating_sub(smaller);
    match diff.checked_mul(Uint128::from(BPS_SCALE)) {
        Ok(scaled) => scaled
            .checked_div(smaller)
            .map(|v| v.u128())
            .unwrap_or(u128::MAX),
        Err(_) => u128::MAX,
    }
}

// Bootstrap pool-count policy — INTENTIONALLY NOT ENFORCED.
//
// We explicitly accept a single-pool-dominated price during the
// bootstrap window because every commit needs an oracle price, but no
// creator pool can cross its threshold until commits succeed —
// enforcing a floor would deadlock the protocol on day one.
//
// Defense-in-depth that bounds the bootstrap manipulation risk:
//   - `pool_meets_liquidity_floor` raises the cost of moving the anchor
//     (USD-denominated total floor; per-side check via xyk symmetry).
//   - The anchor pool is curated and seeded by the deployment team.
//   - The TWAP circuit breaker caps per-update drift to
//     `MAX_TWAP_DRIFT_BPS` (30%) on every update *after* the first.
//   - Downstream consumers (commit, swap) layer their own slippage
//     and spread protections.
//
// If a future deployment ever wants a hard floor, enforce it in
// `calculate_weighted_price_with_atom` (return `InsufficientData` when
// the eligible-pool count is below the desired floor) plus a
// bootstrap-mode switch on the price reader so the protocol can still
// launch. A previous `pub const MIN_ELIGIBLE_POOLS_FOR_TWAP` was kept
// at module scope as a bookmark, but the value was never read by code
// — the comment above is the policy.
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
    /// Warm-up gate. Number of additional successful TWAP observations
    /// required before downstream price queries
    /// (`get_bluechip_usd_price_with_meta`) will serve a price. Set
    /// non-zero whenever the price cache is reset (anchor change,
    /// timelocked config update that swaps the anchor) so the
    /// very-first post-reset observation can't be locked in as the
    /// canonical price by an attacker who briefly perturbed the new
    /// anchor's reserves. Decremented per successful price-publishing
    /// update; failed (snapshot-only) updates do NOT decrement
    /// because they don't advance the TWAP. While `> 0`, downstream
    /// conversions return `Err(InsufficientData)` for strict callers
    /// (commit path), but best-effort callers
    /// (`usd_to_bluechip_best_effort`)
    /// fall back to `pre_reset_last_price`. `#[serde(default)]` keeps
    /// records written before this field existed deserializing as
    /// zero (no warm-up active).
    #[serde(default)]
    pub warmup_remaining: u32,
    /// Cached bluechip-side index of the anchor pool (0 = bluechip is
    /// reserve0, 1 = bluechip is reserve1). Pinned at every anchor
    /// reset (`SetAnchorPool`, timelocked anchor change in
    /// `UpdateConfig`, force-rotate) so the per-update sample loop
    /// never has to scan `POOLS_BY_ID` to figure out which side is
    /// bluechip on the anchor. Without this cache the prior code
    /// path fell back to an O(N) scan over the full pool registry,
    /// which scales poorly under permissionless `CreateStandardPool`
    /// spam. `#[serde(default)]` lets pre-cache records round-trip
    /// as zero; the next anchor reset repopulates it with the real
    /// value (and, since on every chain we ship today the canonical
    /// anchor has bluechip at index 0, the default of 0 also happens
    /// to be the right value during the brief pre-reset interval).
    #[serde(default)]
    pub anchor_bluechip_index: u8,
    /// Buffered candidate first observation after a price-cache reset.
    /// On every reset (anchor change / force-rotate / bootstrap)
    /// `last_price` is zeroed AND this is set to None. The first
    /// post-reset successful TWAP is held here rather than committed
    /// to `last_price` directly — the second observation drift-checks
    /// against this candidate; on success the median of the two
    /// becomes the new `last_price`, on drift-failure the second
    /// observation replaces the candidate (start-over) up to
    /// `MAX_POST_RESET_CONSECUTIVE_FAILURES` consecutive rounds, after
    /// which the median is force-accepted as a liveness valve (see
    /// `post_reset_consecutive_failures`). The previous behaviour
    /// committed the first observation directly with no drift check
    /// (the breaker bypassed because `prior == 0`), letting a
    /// single-block manipulation of the anchor reserves anchor the
    /// breaker to a bad starting point. With this buffer, an attacker
    /// has to manipulate two consecutive observations within
    /// `MAX_TWAP_DRIFT_BPS` of each other for the bad value to land,
    /// AND can stretch the freeze only up to the cap above before the
    /// median lands anyway. `#[serde(default)]` is `None`.
    #[serde(default)]
    pub pending_first_price: Option<Uint128>,
    /// Snapshot of `last_price` immediately before a reset (anchor
    /// change / force-rotate). Used by the best-effort conversion
    /// path so non-critical USD-denominated callers
    /// (CreateStandardPool fee, PayDistributionBounty) can keep
    /// running during the warm-up window. The strict path used by
    /// commit valuation never consults this — a wrong USD valuation
    /// directly translates to wrong threshold-cross arithmetic, so
    /// commits remain hard-failed during warm-up. `#[serde(default)]`
    /// is `Uint128::zero()`; both internal callers gracefully skip
    /// when zero.
    #[serde(default)]
    pub pre_reset_last_price: Uint128,
    /// Liveness escape valve for the post-reset buffer in branch (c).
    /// Each consecutive (c)-failure (drift between candidate and the
    /// new observation exceeds `MAX_TWAP_DRIFT_BPS`) increments this
    /// counter. (c)-success resets it to zero. Branch (b) is the
    /// "first observation after reset" case; it does NOT touch this
    /// counter (the failure semantics start at (c) once a candidate
    /// exists). Once the counter reaches
    /// `MAX_POST_RESET_CONSECUTIVE_FAILURES` we forcibly accept the
    /// median of the buffered candidate and the current observation
    /// as `last_price`, log the force-accept reason, reset the
    /// counter, and resume the steady-state breaker on the next
    /// round. This prevents an attacker who can keep manipulating
    /// the new anchor's reserves for consecutive rounds from
    /// indefinitely freezing the strict commit path.
    /// `#[serde(default)]` is zero on bootstrap and after every
    /// reset.
    #[serde(default)]
    pub post_reset_consecutive_failures: u32,
}

/// Number of successful price-publishing oracle updates required after the
/// price cache is reset (anchor change) before downstream conversions
/// resume. With UPDATE_INTERVAL = 300s, this is 6 × 5min = 30 min of
/// real cumulative-delta evidence before any commit/swap can be priced
/// against the new anchor. Sized so a sustained ~30-min spot perturbation
/// would be required to bias the warm-up TWAP — a much larger commitment
/// than the prior single-block manipulation window.
pub const ANCHOR_CHANGE_WARMUP_OBSERVATIONS: u32 = 6;

/// Maximum consecutive post-reset (c)-failure rounds — i.e. rounds where
/// the new observation drifts more than `MAX_TWAP_DRIFT_BPS` from the
/// buffered candidate — before the breaker forcibly accepts the median.
/// 12 rounds × `UPDATE_INTERVAL = 300s` ≈ 1 hour. Sized as a liveness
/// escape valve: an attacker who can keep manipulating the new anchor
/// across this many consecutive observations is sophisticated enough
/// that the buffer alone cannot serve as the only defense; the
/// `warmup_remaining` counter (held off downstream consumers for the
/// full warm-up window from the moment of force-accept) and the
/// 30%-per-round circuit breaker that resumes on subsequent rounds
/// remain in place. Without this cap an attacker could indefinitely
/// freeze every strict-commit caller, which is a worse failure mode
/// than letting a slightly-influenced median land. The wider trade-off
/// — strict callers still freeze for the warm-up window after a
/// force-accept — preserves observability and gives operators time
/// to investigate even in the force-accept path.
pub const MAX_POST_RESET_CONSECUTIVE_FAILURES: u32 = 12;
#[cw_serde]
pub struct PriceCache {
    pub last_price: Uint128,
    pub last_update: u64,
    pub twap_observations: Vec<PriceObservation>,

    #[serde(default)]
    pub cached_pyth_price: Uint128,
    #[serde(default)]
    pub cached_pyth_timestamp: u64,
    /// Pyth confidence interval (in price units, normalized to 6
    /// decimals like `cached_pyth_price`) captured at the same moment
    /// the cached price was sampled. Re-validated on the cache-fallback
    /// path so a wide-band publish-at-the-edge can't be used to serve
    /// every commit through a Pyth outage. `#[serde(default)]` lets
    /// pre-upgrade records deserialize as zero, which the fallback
    /// path treats as "conf unknown" and refuses to serve from cache
    /// (fail-closed).
    #[serde(default)]
    pub cached_pyth_conf: u64,
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
        get_eligible_creator_pools(deps.as_ref(), env, atom_pool_contract_address)?;
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
    let atom_pool_addr =
        factory_config.atom_bluechip_anchor_pool_address.to_string();

    #[cfg(feature = "mock")]
    {
        return Ok(vec![atom_pool_addr]);
    }

    // Anchor-only short-circuit (audit C-1). Until every non-anchor pool
    // in the allowlist carries a per-pool Pyth feed for its non-bluechip
    // side, the weighted-average math at
    // `calculate_weighted_price_with_atom` cannot meaningfully blend
    // their contributions — see the doc on `ORACLE_BASKET_ENABLED`.
    // Returning `[anchor]` here also skips the eligible-pool snapshot
    // refresh entirely (the snapshot is only used to seed sampling, and
    // we're not sampling), which keeps oracle update cost bounded
    // regardless of registry size.
    if !ORACLE_BASKET_ENABLED {
        return Ok(vec![atom_pool_addr]);
    }

    // Rebuild the eligible-pool snapshot at most once per
    // ELIGIBLE_POOL_REFRESH_BLOCKS (≈5 days); between refreshes the
    // sampler reads from the snapshot instead of scanning POOLS_BY_ID.
    refresh_eligible_pool_snapshot_if_stale(
        &mut deps,
        &env,
        &atom_pool_addr,
    )?;
    let eligible_pools = ELIGIBLE_POOL_SNAPSHOT
        .load(deps.storage)?
        .pool_addresses;
    let random_pools_needed = num_pools.saturating_sub(1);

    if eligible_pools.len() <= random_pools_needed {
        let mut all_pools = eligible_pools;
        all_pools.push(atom_pool_addr);
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
                    cached_pyth_conf: 0,
                },
                update_interval: UPDATE_INTERVAL,
                warmup_remaining: 0,
                anchor_bluechip_index: 0,
                pending_first_price: None,
                pre_reset_last_price: Uint128::zero(),
                post_reset_consecutive_failures: 0,
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
    selected.push(atom_pool_addr);
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
        return Err(ContractError::MissingAtomPool {});
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
            cached_pyth_conf: 0,
        },
        update_interval: UPDATE_INTERVAL,
        // Initial bootstrap warm-up. Treated identically to an anchor
        // change: the very first observations carry no historical TWAP
        // weight, so we refuse to serve a price downstream until enough
        // real cumulative-delta evidence has accumulated.
        warmup_remaining: ANCHOR_CHANGE_WARMUP_OBSERVATIONS,
        // Anchor isn't actually set yet at factory instantiate (the
        // address in factory_config is the deploy-time placeholder
        // until `SetAnchorPool` fires). The real value is populated
        // by `execute_set_anchor_pool` and every subsequent anchor-reset
        // path. Default to 0 here; the next anchor reset repopulates.
        anchor_bluechip_index: 0,
        // No pending observation at bootstrap; the warm-up loop fills
        // this in starting on the very first post-bootstrap update.
        pending_first_price: None,
        // No pre-reset price exists at bootstrap.
        pre_reset_last_price: Uint128::zero(),
        // No prior failures at bootstrap.
        post_reset_consecutive_failures: 0,
    };

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    Ok(Response::new())
}

/// Returns (eligible_addresses, bluechip_indices). Each address at index
/// `i` has bluechip on reserve-side `bluechip_indices[i]` (0 or 1).
/// Hoisting this into the snapshot is what makes oracle updates O(1) per
/// sampled pool instead of O(N).
///
/// Two parallel inputs feed the result (M-3 audit fix):
///
///   1. **Admin-curated allowlist** (`ORACLE_ELIGIBLE_POOLS`). Any pool
///      kind. Bluechip-side index pre-resolved at allowlist-add time so
///      no per-call scan is needed. Required for the early-stage
///      roadmap where bluechip/IBC standard pools are the only
///      externally-priced sources.
///
///   2. **Threshold-crossed commit pools** — included ONLY when
///      `COMMIT_POOLS_AUTO_ELIGIBLE` is true. Default false on fresh
///      deployments; set true by migrate to preserve legacy behaviour
///      for existing deployments. When false, commit pools enter the
///      oracle only via the allowlist (manual override during stages
///      3–4 of the roadmap).
///
/// Both sources independently apply:
///   - skip-anchor (the anchor is added separately by
///     `select_random_pools_with_atom`),
///   - bluechip-side resolution (skip pools without one),
///   - cross-contract `pool_meets_liquidity_floor` check (skip drained
///     and lopsided pools; USD-denominated with a hardcoded fallback
///     when the oracle has no price yet).
///
/// Dedup by pool address: a pool that's both allowlisted AND
/// threshold-crossed-commit shows up once. Allowlist takes precedence
/// for the bluechip-index value (it was admin-blessed at add time).
pub fn get_eligible_creator_pools(
    deps: Deps,
    env: &Env,
    atom_pool_contract_address: &str,
) -> StdResult<(Vec<String>, Vec<u8>)> {
    use std::collections::HashSet;
    let mut entries: Vec<EligiblePoolEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // ---- Source 1: admin allowlist (any pool kind) ----
    //
    // Full scan — the allowlist is admin-curated and bounded by
    // propose/apply timelocked actions, so its size stays small
    // (typically <20 entries). Each entry's `bluechip_index` was
    // pre-resolved at allowlist-add time so no per-entry registry
    // scan is needed here.
    for entry in crate::state::ORACLE_ELIGIBLE_POOLS.range(
        deps.storage,
        None,
        None,
        Order::Ascending,
    ) {
        let (pool_addr, allowlist_entry) = entry?;
        let pool_addr_str = pool_addr.to_string();
        if pool_addr_str == atom_pool_contract_address {
            continue;
        }

        // Cross-contract liquidity gate (M-4 audit fix). The allowlist is
        // curated by admin at propose/apply time, but a pool that met the
        // floor at allowlist-add time can drain afterwards — drop those
        // without an admin RemoveOracleEligiblePool, so the snapshot stays
        // honest until the next admin action.
        let pool_state: PoolStateResponseForFactory = match deps
            .querier
            .query_wasm_smart(pool_addr_str.clone(), &PoolQueryMsg::GetPoolState {})
        {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !pool_meets_liquidity_floor(
            deps.storage,
            &pool_state,
            allowlist_entry.bluechip_index,
        )? {
            continue;
        }

        seen.insert(pool_addr_str.clone());
        entries.push(EligiblePoolEntry {
            address: pool_addr_str,
            bluechip_index: allowlist_entry.bluechip_index,
        });
    }

    // ---- Source 2: threshold-crossed commit pools (gated by global flag) ----
    //
    // M-5 audit fix. Previously this iterated every entry in
    // `POOLS_BY_ID` and ran a cross-contract `GetPoolState` query per
    // candidate; at any meaningful pool count that blew through block
    // gas for the snapshot refresh and bricked oracle updates whenever
    // a rotation interval coincided with snapshot staleness.
    //
    // New approach: random-pull-with-reject. Pick a random pool id in
    // `[1, POOL_COUNTER]`, validate (kind == Commit, threshold-minted,
    // bluechip side present, liquidity floor), accept on success or
    // toss and re-pick on any failure. Capped at
    // `MAX_AUTO_ELIGIBLE_SAMPLE_ATTEMPTS` total attempts so a registry
    // dominated by pre-threshold or drained pools cannot brick the
    // refresh by exhausting the loop. Target sample size is
    // `ORACLE_POOL_COUNT`, matching the downstream sampler's target.
    //
    // Sample composition differs from the prior "exhaustive eligible
    // set" — a pool that crosses threshold may not appear in the next
    // refresh's sample by random chance, but will be a candidate on
    // every subsequent refresh (the seed is block-dependent, so the
    // sample rotates across refreshes). Over time eligible pools get
    // their fair share of inclusions; in any single round, sample bias
    // is bounded by the snapshot's broader purpose (sampling, not
    // exhaustive enumeration).
    let auto_commit = crate::state::load_commit_pools_auto_eligible(deps.storage);
    if auto_commit {
        let pool_count_max = crate::state::POOL_COUNTER
            .may_load(deps.storage)?
            .unwrap_or(0);
        if pool_count_max > 0 {
            const TARGET_SAMPLE_SIZE: usize = ORACLE_POOL_COUNT;
            // Cap = 4× target. Sized so a registry where ~75% of pools
            // fail validation (pre-threshold, paused, drained, or wrong
            // kind) still yields close to a full sample; tighter and
            // we'd under-sample healthy registries, looser and a
            // hostile-pool registry could burn meaningful gas before
            // giving up.
            const MAX_AUTO_ELIGIBLE_SAMPLE_ATTEMPTS: usize =
                TARGET_SAMPLE_SIZE * 4;

            let mut hasher = Sha256::new();
            hasher.update(env.block.time.seconds().to_be_bytes());
            hasher.update(env.block.height.to_be_bytes());
            hasher.update(env.block.chain_id.as_bytes());
            hasher.update((pool_count_max).to_be_bytes());
            let hash = hasher.finalize();

            let mut tried: HashSet<u64> = HashSet::new();
            let mut attempts = 0usize;
            while entries.len() < TARGET_SAMPLE_SIZE
                && attempts < MAX_AUTO_ELIGIBLE_SAMPLE_ATTEMPTS
            {
                // Take 8 bytes from a rotating window of the hash, mixed
                // with the attempt index so we visit distinct slots.
                let i = attempts % 32;
                let seed_bytes = [
                    hash[i],
                    hash[(i + 1) % 32],
                    hash[(i + 2) % 32],
                    hash[(i + 3) % 32],
                    hash[(i + 4) % 32],
                    hash[(i + 5) % 32],
                    hash[(i + 6) % 32],
                    hash[(i + 7) % 32],
                ];
                let seed_u64 = u64::from_be_bytes(seed_bytes)
                    .wrapping_add((attempts as u64).wrapping_mul(0x9e3779b97f4a7c15));
                attempts += 1;
                let candidate_id = (seed_u64 % pool_count_max) + 1;
                if !tried.insert(candidate_id) {
                    // Already tried this id in an earlier iteration of
                    // the current loop; skip without burning a query.
                    continue;
                }

                let pool_details = match POOLS_BY_ID.may_load(deps.storage, candidate_id)? {
                    Some(d) => d,
                    None => continue,
                };

                let pool_addr_str = pool_details.creator_pool_addr.to_string();
                if pool_addr_str == atom_pool_contract_address {
                    continue;
                }
                if seen.contains(&pool_addr_str) {
                    continue;
                }
                // Auto-eligible source covers commit pools only.
                if pool_details.pool_kind == PoolKind::Standard {
                    continue;
                }
                let bluechip_idx = match pool_details
                    .pool_token_info
                    .iter()
                    .position(|t| matches!(t, TokenType::Native { .. }))
                {
                    Some(i) => i as u8,
                    None => continue,
                };
                if !POOL_THRESHOLD_MINTED
                    .may_load(deps.storage, candidate_id)?
                    .unwrap_or(false)
                {
                    continue;
                }

                let pool_state: PoolStateResponseForFactory = match deps
                    .querier
                    .query_wasm_smart(pool_addr_str.clone(), &PoolQueryMsg::GetPoolState {})
                {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if !pool_meets_liquidity_floor(deps.storage, &pool_state, bluechip_idx)? {
                    continue;
                }

                seen.insert(pool_addr_str.clone());
                entries.push(EligiblePoolEntry {
                    address: pool_addr_str,
                    bluechip_index: bluechip_idx,
                });
            }
        }
    }

    let (eligible, indices) = entries
        .into_iter()
        .map(|e| (e.address, e.bluechip_index))
        .unzip();
    Ok((eligible, indices))
}

/// One entry in the eligible-pool snapshot, kept as an atomic
/// (address, bluechip_index) pair so the by-position coupling in
/// [`crate::state::EligiblePoolSnapshot`] cannot be violated by a
/// caller that pushes to one half but not the other.
struct EligiblePoolEntry {
    address: String,
    bluechip_index: u8,
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
/// Convert a USD-denominated bounty (6-decimal microUSD) to bluechip
/// using a single price ratio (`PRICE_PRECISION` units of bluechip per
/// USD). Returns `BluechipPriceZero` (price == 0) — typed error so the
/// mock and prod paths can't drift on generic_err strings.
///
/// Used by the mock oracle's USD→bluechip conversion, which sees the
/// just-fetched mock price directly. The prod path goes through the
/// richer `usd_to_bluechip` helper (TWAP + Pyth fallback); this helper
/// is purposely simple — a single multiply-then-divide. Gated on the
/// mock feature to keep the dead-code warning quiet in non-mock builds.
#[cfg(feature = "mock")]
fn compute_bounty_bluechip(bounty_usd: Uint128, price: Uint128) -> Result<Uint128, ContractError> {
    if price.is_zero() {
        return Err(ContractError::BluechipPriceZero);
    }
    bounty_usd
        .checked_mul(Uint128::from(PRICE_PRECISION))
        .map_err(|_| {
            ContractError::Std(StdError::generic_err("bounty conversion overflow"))
        })?
        .checked_div(price)
        .map_err(|_| {
            // price was non-zero above; checked_div errors here would be
            // an internal cosmwasm_std bug, but we propagate cleanly
            // rather than panicking.
            ContractError::Std(StdError::generic_err("bounty conversion div error"))
        })
}

// the BankMsg transfer) to `response`. Three branches, deterministic attribute
// shape. Shared between the mock and prod oracle paths so the attribute
// schema can only drift in one place.
fn apply_oracle_bounty(
    mut response: Response,
    bounty_usd: Uint128,
    bounty_bluechip: Uint128,
    factory_balance: Uint128,
    recipient: &Addr,
    bluechip_denom: &str,
) -> Response {
    if !bounty_bluechip.is_zero() && factory_balance >= bounty_bluechip {
        response = response
            .add_message(CosmosMsg::Bank(BankMsg::Send {
                to_address: recipient.to_string(),
                amount: vec![Coin {
                    denom: bluechip_denom.to_string(),
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
            let bounty_bluechip = compute_bounty_bluechip(bounty_usd, price)?;
            let bounty_cfg = FACTORYINSTANTIATEINFO.load(deps.storage)?;
            let balance = deps
                .querier
                .query_balance(env.contract.address.as_str(), &bounty_cfg.bluechip_denom)?;
            response = apply_oracle_bounty(
                response,
                bounty_usd,
                bounty_bluechip,
                balance.amount,
                &info.sender,
                &bounty_cfg.bluechip_denom,
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
            oracle.anchor_bluechip_index,
        )?;
    // Always persist the new snapshots so the next round has prior data
    // to compute a TWAP from, even when this round couldn't produce a
    // price (bootstrap, anchor inactive, etc.). Returning Err on those
    // paths would revert the snapshot save and leave the oracle stuck.
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

    // Circuit breaker.
    //
    // Branch selection depends on three flags:
    //   - `prior` = oracle.bluechip_price_cache.last_price
    //   - `pre_reset` = oracle.pre_reset_last_price
    //   - `candidate` = oracle.pending_first_price
    //
    // Four branches:
    //
    //   (a) Steady state (`prior > 0`): drift-check the new TWAP against
    //       the prior cached price. If the diff exceeds
    //       MAX_TWAP_DRIFT_BPS the entire tx reverts — every storage
    //       write above (twap_observations push, pyth cache update,
    //       snapshot save) is rolled back, so the next caller sees the
    //       same prior state and gets a fresh shot.
    //
    //   (b) Post-reset, no candidate yet (`prior == 0` AND
    //       `pre_reset > 0` AND `candidate == None`): hold the new
    //       TWAP as a candidate. Do NOT publish it to `last_price`,
    //       do NOT decrement `warmup_remaining`. The point of the
    //       buffer is to prevent a single-block manipulation of the
    //       new anchor from anchoring the breaker to a bad value —
    //       the next observation drift-checks against the candidate.
    //
    //   (c) Post-reset, candidate exists (`prior == 0` AND
    //       `candidate == Some(c)`): drift-check the new observation
    //       against the candidate. On success the median of the two
    //       becomes the new `last_price`, the buffer clears, and the
    //       warm-up counter starts ticking. On drift-failure we
    //       discard the prior candidate and replace it with the new
    //       observation (start over).
    //
    //   (d) Bootstrap (`prior == 0` AND `pre_reset == 0` AND
    //       `candidate == None`): the very first observation after
    //       factory instantiate. There is no prior trusted price for
    //       the breaker to be anchored against, so the buffer adds no
    //       protection — and adding it here would force a one-time
    //       extra `UpdateOraclePrice` cycle at every fresh deployment.
    //       Publish directly. This branch fires exactly once per
    //       factory lifecycle (bootstrap), and only on rotation paths
    //       where `pre_reset_last_price` failed to land
    //       (e.g. `SetAnchorPool` happens before any update fired).
    //       Anchor rotations after the first published price always
    //       have `pre_reset > 0` and route through (b)/(c).
    let prior = oracle.bluechip_price_cache.last_price;
    let pre_reset = oracle.pre_reset_last_price;
    let buffered_reset_path = !pre_reset.is_zero();

    // Each breaker branch is factored into a helper that mutates `oracle`
    // and returns a `BreakerOutcome`:
    //   - `Published { ... }` means the helper committed a new
    //     `last_price`; the caller runs the shared success tail
    //     (cache pyth, decrement warmup, save oracle, pay bounty).
    //   - `EarlyReturn(response)` means the helper persisted oracle
    //     state and built a complete `Response`; the caller returns
    //     it directly without touching warmup/bounty.
    let outcome = if !prior.is_zero() {
        breaker_branch_a(&mut oracle, twap_price, current_time)?
    } else if let Some(candidate) = oracle.pending_first_price {
        breaker_branch_c(
            deps.branch(),
            &mut oracle,
            twap_price,
            candidate,
            current_time,
        )?
    } else if buffered_reset_path {
        breaker_branch_b(deps.branch(), &mut oracle, twap_price)?
    } else {
        breaker_branch_d(deps.branch(), &env, &mut oracle, twap_price, atom_price)?
    };

    let published = match outcome {
        BreakerOutcome::EarlyReturn(response) => return Ok(response),
        BreakerOutcome::Published(p) => p,
    };

    // Shared success tail. Reached only by branches (a) and (c-success) —
    // the regular price-publishing rounds. Caches the Pyth ATOM/USD price,
    // decrements the warm-up counter, persists the oracle, and pays the
    // keeper bounty. (c-fail-force-accept) commits a new `last_price` too
    // but does its own pyth/warmup/save inline and returns EarlyReturn so
    // it can skip the bounty — force-accept is a liveness escape valve,
    // not a regular publishing event, mirroring the pre-refactor handler.
    if let Ok((pyth_price, pyth_conf)) =
        query_pyth_atom_usd_price_with_conf(deps.as_ref(), &env)
    {
        oracle.bluechip_price_cache.cached_pyth_price = pyth_price;
        oracle.bluechip_price_cache.cached_pyth_timestamp = current_time;
        oracle.bluechip_price_cache.cached_pyth_conf = pyth_conf;
    }

    let warmup_remaining_before = oracle.warmup_remaining;
    oracle.warmup_remaining = oracle.warmup_remaining.saturating_sub(1);

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;

    let bounty_usd = ORACLE_UPDATE_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();
    let mut response = Response::new()
        .add_attribute("action", "update_oracle")
        .add_attribute("twap_price", published.published_price.to_string())
        .add_attribute("pools_used", pools_to_use.len().to_string())
        .add_attribute(
            "warmup_remaining_before",
            warmup_remaining_before.to_string(),
        )
        .add_attribute(
            "warmup_remaining_after",
            oracle.warmup_remaining.to_string(),
        );
    for (k, v) in published.extra_attrs {
        response = response.add_attribute(k, v);
    }

    if !bounty_usd.is_zero() {
        // L-6 audit fix: use the best-effort conversion path so keepers
        // stay paid through the post-reset warm-up window. The bounty
        // is capped at $0.10/call and the pre-reset price (the fallback
        // the best-effort path uses) is bounded by the 30% TWAP
        // circuit breaker that armed it; mispricing exposure during
        // warm-up is therefore <$0.03 per call, well below the cost
        // of leaving keepers unpaid and risking oracle staleness.
        match usd_to_bluechip_best_effort(deps.as_ref(), bounty_usd, &env) {
            Ok(conv) => {
                let bounty_cfg = FACTORYINSTANTIATEINFO.load(deps.storage)?;
                let balance = deps
                    .querier
                    .query_balance(env.contract.address.as_str(), &bounty_cfg.bluechip_denom)?;
                response = apply_oracle_bounty(
                    response,
                    bounty_usd,
                    conv.amount,
                    balance.amount,
                    &info.sender,
                    &bounty_cfg.bluechip_denom,
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

// ---------------------------------------------------------------------------
// Circuit-breaker branch helpers.
//
// The four breaker branches were previously inlined inside
// `update_internal_oracle_price`, which made the function span ~500
// lines and obscured the shared "publish + bounty" tail that branches
// (a)/(c-success)/(c-force-accept) all funnel into. Each helper now
// owns its branch's storage mutations and signals the caller via
// `BreakerOutcome` whether to run the shared tail or return immediately.
// ---------------------------------------------------------------------------

/// Carries the data the shared success tail needs from a published
/// branch: the price actually written to `last_price` (`twap_price`
/// for branch (a), the candidate/twap median for c-success and
/// c-fail-force-accept) plus per-branch attributes the tail appends
/// after the standard set. c-fail-force-accept routes through here
/// (rather than building its own response) so pyth caching,
/// warmup-decrement, oracle save, and keeper-bounty payout all
/// run from a single place — the keeper that finally closes a 1h
/// failure window deserves the same compensation as a steady-state
/// publish.
struct BreakerPublished {
    published_price: Uint128,
    extra_attrs: Vec<(&'static str, String)>,
}

enum BreakerOutcome {
    /// Branch committed a new `last_price`. Caller runs the shared tail.
    Published(BreakerPublished),
    /// Branch persisted oracle state and produced a complete `Response`.
    /// Caller returns it directly.
    EarlyReturn(Response),
}

/// Branch (a): steady-state drift check.
fn breaker_branch_a(
    oracle: &mut BlueChipPriceInternalOracle,
    twap_price: Uint128,
    current_time: u64,
) -> Result<BreakerOutcome, ContractError> {
    let prior = oracle.bluechip_price_cache.last_price;
    let drift_bps_u128 = drift_bps_saturating(twap_price, prior);
    if drift_bps_u128 > MAX_TWAP_DRIFT_BPS as u128 {
        return Err(ContractError::TwapCircuitBreaker {
            prior,
            new: twap_price,
            drift_bps: drift_bps_u128,
            max_bps: MAX_TWAP_DRIFT_BPS,
        });
    }
    oracle.bluechip_price_cache.last_price = twap_price;
    oracle.bluechip_price_cache.last_update = current_time;
    Ok(BreakerOutcome::Published(BreakerPublished {
        published_price: twap_price,
        extra_attrs: Vec::new(),
    }))
}

/// Branch (b): first post-reset observation with a `pre_reset > 0`
/// trusted price to protect against manipulation. Buffers the TWAP as
/// a candidate; the next round drift-checks in branch (c).
fn breaker_branch_b(
    deps: DepsMut,
    oracle: &mut BlueChipPriceInternalOracle,
    twap_price: Uint128,
) -> Result<BreakerOutcome, ContractError> {
    // Pop the just-pushed observation so the warm-up TWAP window
    // doesn't accumulate buffered candidates as data points.
    oracle.bluechip_price_cache.twap_observations.pop();
    oracle.pending_first_price = Some(twap_price);
    INTERNAL_ORACLE.save(deps.storage, oracle)?;
    Ok(BreakerOutcome::EarlyReturn(
        Response::new()
            .add_attribute("action", "update_oracle")
            .add_attribute("price_published", "false")
            .add_attribute("reason", "first_post_reset_observation_buffered")
            .add_attribute("candidate_price", twap_price.to_string())
            .add_attribute("warmup_remaining", oracle.warmup_remaining.to_string()),
    ))
}

/// Branch (c): second post-reset observation. Drift-check against the
/// buffered candidate. Three sub-cases:
///   - drift OK → publish median (Published).
///   - drift fail, failures < cap → discard candidate, replace,
///     return early (EarlyReturn).
///   - drift fail, failures hits cap → liveness force-accept the
///     median (Published). Routes through the shared success tail
///     for pyth caching, warmup decrement, oracle save, AND keeper
///     bounty — paying the keeper that closes a 1h failure window
///     keeps the incentive aligned with calling through rough rounds.
fn breaker_branch_c(
    deps: DepsMut,
    oracle: &mut BlueChipPriceInternalOracle,
    twap_price: Uint128,
    candidate: Uint128,
    current_time: u64,
) -> Result<BreakerOutcome, ContractError> {
    let drift_bps_u128 = drift_bps_saturating(twap_price, candidate);

    let median = candidate
        .checked_add(twap_price)?
        .checked_div(Uint128::from(2u128))
        .map_err(|_| ContractError::Std(StdError::generic_err("median div-by-zero")))?;

    if drift_bps_u128 > MAX_TWAP_DRIFT_BPS as u128 {
        let next_failures = oracle.post_reset_consecutive_failures.saturating_add(1);
        if next_failures >= MAX_POST_RESET_CONSECUTIVE_FAILURES {
            // c-fail-force-accept: keep the just-pushed observation,
            // overwrite its price with the median so the observation
            // series and last_price stay in lock-step. Route through
            // the shared success tail so pyth-cache + warmup-decrement
            // + oracle-save + keeper-bounty all run consistently with
            // the steady-state path. Force-accept is the liveness
            // escape valve at the end of a 1h failure window; paying
            // the keeper that closed it keeps the incentive aligned
            // with calling through the rough rounds.
            if let Some(last) = oracle.bluechip_price_cache.twap_observations.last_mut() {
                last.price = median;
            }
            oracle.bluechip_price_cache.last_price = median;
            oracle.bluechip_price_cache.last_update = current_time;
            oracle.pending_first_price = None;
            oracle.post_reset_consecutive_failures = 0;
            return Ok(BreakerOutcome::Published(BreakerPublished {
                published_price: median,
                extra_attrs: vec![
                    ("force_accept", "true".to_string()),
                    (
                        "force_accept_reason",
                        "post_reset_consecutive_failures_cap".to_string(),
                    ),
                    (
                        "force_accept_threshold",
                        MAX_POST_RESET_CONSECUTIVE_FAILURES.to_string(),
                    ),
                    ("prior_candidate", candidate.to_string()),
                    ("new_candidate", twap_price.to_string()),
                    ("median_published", median.to_string()),
                    ("drift_bps", drift_bps_u128.to_string()),
                    ("max_bps", MAX_TWAP_DRIFT_BPS.to_string()),
                ],
            }));
        }
        // c-fail-replace: discard prior candidate, hold the new one.
        oracle.bluechip_price_cache.twap_observations.pop();
        oracle.pending_first_price = Some(twap_price);
        oracle.post_reset_consecutive_failures = next_failures;
        INTERNAL_ORACLE.save(deps.storage, oracle)?;
        return Ok(BreakerOutcome::EarlyReturn(
            Response::new()
                .add_attribute("action", "update_oracle")
                .add_attribute("price_published", "false")
                .add_attribute("reason", "post_reset_candidate_replaced_drift_too_large")
                .add_attribute("prior_candidate", candidate.to_string())
                .add_attribute("new_candidate", twap_price.to_string())
                .add_attribute("drift_bps", drift_bps_u128.to_string())
                .add_attribute("max_bps", MAX_TWAP_DRIFT_BPS.to_string())
                .add_attribute("consecutive_failures", next_failures.to_string())
                .add_attribute(
                    "force_accept_threshold",
                    MAX_POST_RESET_CONSECUTIVE_FAILURES.to_string(),
                ),
        ));
    }

    // c-success: drift OK. Publish the median to keep a single
    // manipulated observation among the two from pulling more than
    // half the weight on `last_price`. Overwrite the just-pushed
    // observation's price to keep the TWAP window and `last_price`
    // in lock-step.
    if let Some(last) = oracle.bluechip_price_cache.twap_observations.last_mut() {
        last.price = median;
    }
    oracle.bluechip_price_cache.last_price = median;
    oracle.bluechip_price_cache.last_update = current_time;
    oracle.pending_first_price = None;
    oracle.post_reset_consecutive_failures = 0;
    Ok(BreakerOutcome::Published(BreakerPublished {
        published_price: median,
        extra_attrs: Vec::new(),
    }))
}

/// Branch (d): bootstrap. No prior price, no pre-reset, no candidate.
/// Buffer the TWAP into `PENDING_BOOTSTRAP_PRICE` and require an admin
/// `ConfirmBootstrapPrice` to publish it (HIGH-4 audit fix).
fn breaker_branch_d(
    deps: DepsMut,
    env: &Env,
    oracle: &mut BlueChipPriceInternalOracle,
    twap_price: Uint128,
    atom_price: Uint128,
) -> Result<BreakerOutcome, ContractError> {
    // Pop the just-pushed observation: unconfirmed candidates must not
    // accumulate in the TWAP window.
    oracle.bluechip_price_cache.twap_observations.pop();

    let pending = match crate::state::PENDING_BOOTSTRAP_PRICE.may_load(deps.storage)? {
        Some(prev) => crate::state::PendingBootstrapPrice {
            price: twap_price,
            atom_pool_price: atom_price,
            proposed_at: prev.proposed_at,
            observation_count: prev.observation_count.saturating_add(1),
        },
        None => crate::state::PendingBootstrapPrice {
            price: twap_price,
            atom_pool_price: atom_price,
            proposed_at: env.block.time,
            observation_count: 1,
        },
    };
    let earliest_confirm = pending
        .proposed_at
        .plus_seconds(crate::state::BOOTSTRAP_OBSERVATION_SECONDS);

    crate::state::PENDING_BOOTSTRAP_PRICE.save(deps.storage, &pending)?;
    INTERNAL_ORACLE.save(deps.storage, oracle)?;
    Ok(BreakerOutcome::EarlyReturn(
        Response::new()
            .add_attribute("action", "update_oracle")
            .add_attribute("price_published", "false")
            .add_attribute("reason", "bootstrap_awaiting_admin_confirmation")
            .add_attribute("candidate_price", twap_price.to_string())
            .add_attribute("observation_count", pending.observation_count.to_string())
            .add_attribute(
                "earliest_confirm_time",
                earliest_confirm.seconds().to_string(),
            )
            .add_attribute("warmup_remaining", oracle.warmup_remaining.to_string()),
    ))
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
//     answered a `GetPoolState` query and met `pool_meets_liquidity_floor`
//     (USD-denominated, per-side aware; M-4 audit fix), regardless of
//     whether its price contributed to the weighted sum this round.
//
// SPOT PRICE IS NEVER USED. All three former spot-fallback branches
// (anchor-stale-cumulative, bootstrap, anchor-missing-from-prev) now
// `continue` instead. A single-block `reserve0/reserve1` read is trivially
// manipulable by a sufficiently-funded attacker; rather than mixing it
// into the TWAP and contaminating downstream USD conversions for the
// next ~1h TWAP_WINDOW, we refuse to publish until the AMM has produced
// real cumulative-delta evidence over a real time window.
//
// ANCHOR-ONLY MODE (audit C-1). When `ORACLE_BASKET_ENABLED == false`
// the upstream sampler `select_random_pools_with_atom` returns just
// `[anchor]`, so this function only iterates the anchor and the
// weighted_average simplifies to `atom_pool_price` — `last_price` is
// the pure anchor TWAP, which is unambiguously bluechip-per-ATOM and
// works correctly with the consumer at
// `get_bluechip_usd_price_with_meta`. The cross-pool aggregation code
// below is preserved for the basket-enable milestone; it does not
// run in v1 because no non-anchor pool is ever in `pool_addresses`.
pub fn calculate_weighted_price_with_atom(
    deps: Deps,
    pool_addresses: &[String],
    prev_snapshots: &[PoolCumulativeSnapshot],
    anchor_bluechip_index: u8,
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
                // Determine if Bluechip is reserve0 or reserve1.
                //
                //   - Anchor pool: read the index pinned on
                //     `BlueChipPriceInternalOracle.anchor_bluechip_index`.
                //     Populated at every anchor reset (SetAnchorPool,
                //     timelocked anchor change, ForceRotate). Replaces an
                //     O(N) fallback scan over POOLS_BY_ID that previously
                //     ran for the anchor on every oracle update.
                //
                //   - Non-anchor (creator) pools: read from the cached
                //     `EligiblePoolSnapshot.bluechip_indices`, populated
                //     at snapshot-refresh time. If the lookup misses
                //     (pre-cache snapshot, or pool just rotated out of
                //     the eligible set), skip the pool rather than falling
                //     back to a registry scan — the next snapshot refresh
                //     will repopulate, and skipping a single pool for
                //     one round only diminishes the weighted sum slightly.
                let is_bluechip_second = if pool_address == &atom_pool_address {
                    anchor_bluechip_index == 1
                } else if let Some(idx) = bluechip_index_lookup(deps, pool_address)? {
                    idx == 1
                } else {
                    // Non-anchor pool not in the cached snapshot. Anomalous
                    // — skip rather than guess. The eligible-pool snapshot
                    // is rebuilt every ~5 days, so any newly-eligible pool
                    // shows up there on the next refresh.
                    continue;
                };

                // M-4 liquidity-floor gate. Resolves the bluechip-side
                // index first so the floor can be applied per-side
                // (USD-denominated; falls back to a hardcoded bluechip
                // value when the oracle has no price yet). Replaces the
                // legacy `reserve0 + reserve1 >= MIN_POOL_LIQUIDITY`
                // check, which conflated units across asymmetric pairs
                // and missed lopsided pools whose bluechip side held
                // negligible value despite a large notional total.
                let bluechip_idx_u8 = if is_bluechip_second { 1u8 } else { 0u8 };
                if !pool_meets_liquidity_floor(deps.storage, &pool_state, bluechip_idx_u8)
                    .map_err(ContractError::Std)?
                {
                    continue;
                }

                // Resolve bluechip reserve based on token ordering.
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
                        // TWAP = cumulative_delta / time_delta.
                        //
                        // The accumulator on the pool side is already
                        // pre-scaled by `PRICE_ACCUMULATOR_SCALE` (==
                        // `PRICE_PRECISION`) inside
                        // `pool_core::swap::update_price_accumulator`, so
                        // the result of this division is already in the
                        // 6-decimal `bluechip-per-other` representation
                        // the rest of the oracle expects. The previous
                        // post-divide `* PRICE_PRECISION` step would have
                        // applied scaling twice and is intentionally
                        // omitted here.
                        cumulative_delta
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
/// Thin compatibility wrapper. Existing callers that don't need the
/// confidence interval keep their `Uint128` return shape; the live
/// conf check + caching is fully delegated to
/// `query_pyth_atom_usd_price_with_conf`.
pub fn query_pyth_atom_usd_price(deps: Deps, env: &Env) -> StdResult<Uint128> {
    query_pyth_atom_usd_price_with_conf(deps, env).map(|(price, _)| price)
}

/// Same as `query_pyth_atom_usd_price` but also returns the normalized
/// (6-decimal) confidence interval. Callers that persist the price into
/// `PriceCache` use this so the cached `(price, conf)` pair can be
/// re-validated on the cache-fallback path with the same bps gate as
/// the live read.
pub fn query_pyth_atom_usd_price_with_conf(
    deps: Deps,
    env: &Env,
) -> StdResult<(Uint128, u64)> {
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

        let query_msg = PythQueryMsg::PriceFeed {
            id: feed_id.clone(),
        };

        // The `GetPrice` fallback is only meaningful for the mock
        // oracle (selected via the `mock` cargo feature). In production
        // a Pyth query failure must surface as `Err` so the
        // cache-fallback path inside `get_bluechip_usd_price_with_meta`
        // can decide whether to bridge the outage from the cached price
        // or refuse to serve. Without the cfg-gate, an operator who
        // accidentally pointed `pyth_contract_addr_for_conversions` at
        // a mock-flavoured oracle in production would silently receive
        // mock prices.
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

        let current_time = env.block.time.seconds();

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

        // Reject negative publish_time. Honest Pyth feeds always emit a
        // positive Unix timestamp; a negative value indicates a malformed
        // or attacker-crafted response. Without this guard the staleness
        // arithmetic below would wrap on i64 - i64 in release wasm and
        // a far-past i64::MIN publish_time could vacuously pass the cap.
        if price_data.publish_time < 0 {
            return Err(StdError::generic_err(
                "Pyth publish_time is negative",
            ));
        }
        let publish_time_u64 = price_data.publish_time as u64;

        // Reject publish_time meaningfully in the future. A 5-second tolerance
        // covers honest clock skew between Pyth publishers and the chain;
        // anything beyond that is either (a) a buggy/malicious publisher
        // posting a far-future timestamp to bypass the staleness window, or
        // (b) a feed mis-routing where the consumed value is unrelated to
        // the current chain epoch. Either way, refuse to use it.
        const PYTH_FUTURE_SKEW_TOLERANCE_SECONDS: u64 = 5;
        if publish_time_u64 > current_time.saturating_add(PYTH_FUTURE_SKEW_TOLERANCE_SECONDS) {
            return Err(StdError::generic_err(
                "Pyth publish_time is in the future beyond the allowed skew tolerance",
            ));
        }

        // Saturating subtraction prevents any wrap on borderline cases
        // (publish_time slightly ahead of current_time within tolerance,
        // which is permitted above and yields age == 0 here).
        let age_seconds = current_time.saturating_sub(publish_time_u64);
        if age_seconds > crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE {
            return Err(StdError::generic_err("ATOM price is stale"));
        }

        // Validate price is positive. We rely on this check for the conf
        // threshold below — moving or removing it would cause `price as u64`
        // to wrap a negative value into a huge number and pass the conf
        // check vacuously. Don't reorder.
        let price_i64 = price_data.price.i64();
        if price_i64 <= 0 {
            return Err(StdError::generic_err("Invalid negative or zero price"));
        }

        // Reject prices with wide confidence intervals. Threshold is
        // bps of price loaded from `PYTH_CONF_THRESHOLD_BPS` — admin
        // tunable, bounded to a strict range so neither the admin nor a
        // missing storage slot can effectively disable the check.
        // Default is 200 bps (2%), tightened from the prior hardcoded
        // 500 bps (5%) since the previous band let a feed dispersing
        // 4.99% range still serve commits.
        //
        // Use try_into() rather than `as u64` so a future edit that drops
        // or reorders the negative-price check above produces an explicit
        // runtime error rather than a silent wrap to u64::MAX-ish that
        // would let a wide-conf price pass.
        let price_u64: u64 = price_i64.try_into().map_err(|_| {
            StdError::generic_err("Price overflow when computing conf threshold")
        })?;
        let conf_bps = crate::state::load_pyth_conf_threshold_bps(deps.storage);
        // `price_u64 * conf_bps` cannot overflow at any plausible Pyth
        // ATOM/USD reading: ATOM/USD ≈ 1e7 (6-decimal expo) and bps cap
        // is < 1e4, comfortably under u64::MAX.
        let conf_threshold = price_u64
            .saturating_mul(conf_bps as u64)
            .saturating_div(10_000);
        let conf_u64 = price_data.conf.u64();
        if conf_u64 > conf_threshold {
            return Err(StdError::generic_err(format!(
                "Pyth confidence interval too wide: conf={} exceeds {} bps of price={}",
                conf_u64, conf_bps, price_i64
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

        // Normalize price + conf to 6 decimals (system standard). Both
        // share the same exponent on a Pyth feed, so the same scaling
        // applies. The normalized conf is what gets written into the
        // cache so the cache-fallback re-check is bps-comparable
        // against the cached price without re-reading the exponent.
        let raw_conf_u128: u128 = conf_u64 as u128;
        let (normalized_price, normalized_conf_u128) = match expo.cmp(&-6) {
            std::cmp::Ordering::Equal => (Uint128::from(price_u128), raw_conf_u128),
            std::cmp::Ordering::Less => {
                let divisor = 10u128.pow((expo.abs() - 6) as u32);
                (
                    Uint128::from(price_u128 / divisor),
                    raw_conf_u128 / divisor,
                )
            }
            std::cmp::Ordering::Greater => {
                let multiplier = 10u128.pow((6 - expo.abs()) as u32);
                (
                    Uint128::from(price_u128 * multiplier),
                    raw_conf_u128.saturating_mul(multiplier),
                )
            }
        };
        // Saturate the (already-normalized) conf to u64. The conf was
        // validated above to be ≤ `conf_bps/10000 * price_u64`, which fits
        // in u64; the saturation here is purely defensive against any
        // future change that would relax that gate.
        let normalized_conf_u64 = normalized_conf_u128.min(u64::MAX as u128) as u64;

        Ok((normalized_price, normalized_conf_u64))
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
        // Mock conf = 0 so cache-fallback re-validation always passes
        // in tests; production-only behaviour is exercised in the
        // `not(test)` branch above.
        Ok((mock_price, 0u64))
    }
}

/// Internal: returns the bluechip USD price together with the oracle's
/// `last_update` timestamp from a single load of INTERNAL_ORACLE. The
/// conversion wrappers (`bluechip_to_usd` / `usd_to_bluechip`) need both
/// values to populate `ConversionResponse.timestamp`, and the cache
/// fallback path needs the cache to authorize the stale-pyth bridge —
/// so loading the oracle once and reusing it both for the cache check
/// and for the TWAP read avoids the prior 2× / 3× re-deserialization.
///
/// `allow_warmup_fallback` tiers the warm-up gate (audit fix):
///   - `false` (strict): the historical behaviour. Any non-zero
///     `warmup_remaining` returns Err. Used by the commit valuation
///     path — wrong USD valuation directly translates into wrong
///     threshold-cross arithmetic, so commits hard-fail during warm-up.
///   - `true` (best-effort): if `warmup_remaining > 0` AND
///     `pre_reset_last_price > 0`, fall back to the pre-reset price
///     instead of erroring. Used by `CreateStandardPool` fee
///     conversion and `PayDistributionBounty` payout — best-effort
///     callers where a stale-but-bounded fallback price is preferable
///     to freezing the entire protocol on every anchor rotation.
fn get_bluechip_usd_price_with_meta(
    deps: Deps,
    env: &Env,
    allow_warmup_fallback: bool,
) -> StdResult<(Uint128, u64)> {
    // Single load of INTERNAL_ORACLE shared by both the Pyth-fallback
    // branch (which reads `bluechip_price_cache`) and the post-Pyth TWAP
    // computation (which reads `bluechip_price_cache.last_price`).
    let oracle = INTERNAL_ORACLE
        .load(deps.storage)
        .map_err(|_| StdError::generic_err("Internal oracle not initialized"))?;

    // Warm-up gate. After bootstrap or any anchor change the oracle
    // cache is reset, and the very-first post-reset observation is
    // single-block-manipulable. Refuse to serve a price downstream
    // until ANCHOR_CHANGE_WARMUP_OBSERVATIONS price-publishing updates
    // have accumulated. Strict callers (commit) revert during this
    // window. Best-effort callers may fall back to `pre_reset_last_price`
    // if it's non-zero; otherwise they also revert.
    if oracle.warmup_remaining > 0 {
        if allow_warmup_fallback && !oracle.pre_reset_last_price.is_zero() {
            // Best-effort path during warm-up. Use the pre-reset price
            // and tag the conversion's timestamp with the *current*
            // block time so callers don't see a wildly stale
            // `last_update` (the pre-reset cache.last_update is
            // genuinely old now). Pyth ATOM/USD math still applies on
            // top, same as the steady-state path; this only relaxes
            // the gate on the bluechip-side TWAP factor.
            //
            // Safety: best-effort callers (CreateStandardPool fee,
            // PayDistributionBounty) cap their economic exposure at
            // O($0.10) per call AND have their own retry / skip
            // semantics on conversion failure. Worst-case fee
            // mispricing during a warm-up window is bounded by the
            // 30% TWAP circuit-breaker that armed the pre-reset
            // value in the first place.
            let bluechip_per_atom = oracle.pre_reset_last_price;
            let atom_usd_price = match query_pyth_atom_usd_price(deps, env) {
                Ok(p) => p,
                Err(_) => {
                    let cache = &oracle.bluechip_price_cache;
                    let current_time = env.block.time.seconds();
                    if cache.cached_pyth_price.is_zero()
                        || current_time.saturating_sub(cache.cached_pyth_timestamp)
                            > crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE
                    {
                        return Err(StdError::generic_err(
                            "best-effort warm-up: Pyth stale and no cached pyth price",
                        ));
                    }
                    // Re-validate the cached price against its sampled
                    // confidence interval before serving. The conf was
                    // captured at sampling time, so a price that was
                    // borderline-acceptable at sample time is rejected
                    // here as soon as the bps gate tightens — and a
                    // pre-upgrade record with `cached_pyth_conf == 0`
                    // is treated as "conf unknown" and refused
                    // (fail-closed).
                    crate::state::ensure_cached_pyth_conf_acceptable(
                        deps.storage,
                        cache.cached_pyth_price,
                        cache.cached_pyth_conf,
                    )?;
                    cache.cached_pyth_price
                }
            };
            #[cfg(feature = "mock")]
            {
                return Ok((atom_usd_price, env.block.time.seconds()));
            }
            #[cfg(not(feature = "mock"))]
            {
                let bluechip_usd_price = atom_usd_price
                    .checked_mul(Uint128::from(PRICE_PRECISION))
                    .map_err(|e| {
                        StdError::generic_err(format!(
                            "Overflow calculating best-effort warm-up price: {}",
                            e
                        ))
                    })?
                    .checked_div(bluechip_per_atom)
                    .map_err(|e| {
                        StdError::generic_err(format!(
                            "Division error calculating best-effort warm-up price: {}",
                            e
                        ))
                    })?;
                if bluechip_usd_price.is_zero() {
                    return Err(StdError::generic_err(
                        "best-effort warm-up: calculated price is zero",
                    ));
                }
                return Ok((bluechip_usd_price, env.block.time.seconds()));
            }
        }
        return Err(StdError::generic_err(format!(
            "Oracle warm-up in progress after anchor reset: {} more successful TWAP \
             updates required before pricing resumes",
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
            // Re-validate the cached price's confidence interval. A
            // sample taken near the bps gate may no longer be
            // acceptable if the gate has been tightened since; refuse
            // to serve in that case rather than bridging a stale Pyth
            // outage with an ill-conditioned price. Pre-upgrade
            // records (`cached_pyth_conf == 0`) are refused
            // unconditionally — the absence of a sampled conf means
            // we can't authenticate the cached price.
            crate::state::ensure_cached_pyth_conf_acceptable(
                deps.storage,
                cache.cached_pyth_price,
                cache.cached_pyth_conf,
            )?;
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
    get_bluechip_usd_price_with_meta(deps, env, false).map(|(price, _)| price)
}

/// Core conversion: when `to_usd` is true, converts bluechip→USD; otherwise USD→bluechip.
///
/// `allow_warmup_fallback` tiers the warm-up gate (audit fix). See
/// `get_bluechip_usd_price_with_meta` doc for the rationale; in short,
/// strict callers (commit valuation) hard-fail during warm-up while
/// best-effort callers (CreateStandardPool fee, PayDistributionBounty)
/// fall back to `pre_reset_last_price` when available.
fn convert_with_oracle(
    deps: Deps,
    env: &Env,
    amount: Uint128,
    to_usd: bool,
    allow_warmup_fallback: bool,
) -> StdResult<ConversionResponse> {
    // Single oracle load — `get_bluechip_usd_price_with_meta` returns both
    // the price and the cache's `last_update`, so we no longer need a
    // separate `INTERNAL_ORACLE.load(...)` here just to populate the
    // response timestamp.
    let (cached_price, last_update) =
        get_bluechip_usd_price_with_meta(deps, env, allow_warmup_fallback)?;

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

/// Strict bluechip→USD conversion. Fails during warm-up. Used by the
/// pool's commit-valuation query path: a wrong USD valuation directly
/// translates to wrong threshold-cross arithmetic, so we'd rather
/// freeze commits than misprice them.
pub fn bluechip_to_usd(
    deps: Deps,
    bluechip_amount: Uint128,
    env: &Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, env, bluechip_amount, true, false)
}

/// Strict USD→bluechip conversion. Same warm-up semantics as
/// `bluechip_to_usd`.
pub fn usd_to_bluechip(
    deps: Deps,
    usd_amount: Uint128,
    env: &Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, env, usd_amount, false, false)
}

/// Best-effort USD→bluechip conversion (audit fix).
///
/// Same as `usd_to_bluechip` in steady state. During the post-reset
/// warm-up window, falls back to `pre_reset_last_price` instead of
/// erroring — keeping non-critical USD-denominated paths
/// (`CreateStandardPool` creation fee, `PayDistributionBounty`
/// payout) functional through anchor rotations rather than freezing
/// the entire protocol for ~30 minutes on every legitimate rotation.
///
/// Worst-case mispricing during the fallback window is bounded by the
/// 30% TWAP circuit breaker that armed the pre-reset price in the
/// first place. Both call sites cap their economic exposure at O($0.10)
/// per call and have their own retry/skip semantics on conversion
/// failure, so the residual risk is several orders of magnitude
/// below the cost of full protocol freeze.
pub fn usd_to_bluechip_best_effort(
    deps: Deps,
    usd_amount: Uint128,
    env: &Env,
) -> StdResult<ConversionResponse> {
    convert_with_oracle(deps, env, usd_amount, false, true)
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
    ensure_admin(deps.as_ref(), &info)?;

    if crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .is_some()
    {
        return Err(ContractError::ForceRotateAlreadyPending);
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
    ensure_admin(deps.as_ref(), &info)?;

    if crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .is_none()
    {
        return Err(ContractError::NoPendingForceRotate);
    }

    crate::state::PENDING_ORACLE_ROTATION.remove(deps.storage);

    Ok(Response::new().add_attribute("action", "cancel_force_rotate_pools"))
}

pub fn execute_force_rotate_pools(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    // Must have gone through the 48h propose/wait flow.
    let effective_after = crate::state::PENDING_ORACLE_ROTATION
        .may_load(deps.storage)?
        .ok_or(ContractError::NoPendingForceRotate)?;

    if env.block.time < effective_after {
        return Err(ContractError::TimelockNotExpired { effective_after });
    }

    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let new_pools =
        select_random_pools_with_atom(deps.branch(), env.clone(), ORACLE_POOL_COUNT)?;
    oracle.selected_pools = new_pools.clone();
    oracle.last_rotation = env.block.time.seconds();
    // Treat force-rotate as a full oracle reset, identical to the
    // anchor-change path: clear snapshots + observations, zero
    // `last_price`/`last_update`, re-arm the warm-up gate so
    // downstream consumers refuse to serve a price until enough new
    // observations accumulate. Without this, stale snapshots for
    // pools no longer in `selected_pools` linger, the next update
    // skips newly-selected pools that have no prior snapshot, and
    // the retained `last_price` from the pre-rotation set anchors
    // the circuit breaker — defeating the purpose of force-rotate.
    //
    // Snapshot the pre-reset price BEFORE zeroing so best-effort
    // callers (CreateStandardPool fee, PayDistributionBounty) can
    // keep operating through the warm-up window. The strict commit
    // path never consults this — wrong USD valuation = wrong
    // threshold-cross arithmetic — so commits remain hard-failed.
    oracle.pre_reset_last_price = oracle.bluechip_price_cache.last_price;
    oracle.pool_cumulative_snapshots.clear();
    oracle.bluechip_price_cache.last_price = Uint128::zero();
    oracle.bluechip_price_cache.last_update = 0;
    oracle.bluechip_price_cache.twap_observations.clear();
    oracle.warmup_remaining = ANCHOR_CHANGE_WARMUP_OBSERVATIONS;
    // Clear any leftover candidate from a previous reset; the post-
    // rotation warm-up starts fresh.
    oracle.pending_first_price = None;
    // Reset the (c)-failure counter — the new post-rotation window
    // gets its own budget of consecutive failures before force-accept.
    oracle.post_reset_consecutive_failures = 0;
    // H-2 audit fix: clear any pre-confirm bootstrap candidate. Branch
    // (d) of update_internal_oracle_price only fires when `last_price`
    // AND `pre_reset` are both zero — i.e. before the very first
    // `ConfirmBootstrapPrice` has ever published. If admin
    // force-rotates in that window, the next round re-enters branch
    // (d) and would otherwise find this stale candidate with its old
    // `proposed_at`, letting admin confirm immediately without the
    // 1h observation window re-elapsing against the post-rotation
    // pool sample.
    crate::state::PENDING_BOOTSTRAP_PRICE.remove(deps.storage);

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

// ---------------------------------------------------------------------------
// Bootstrap-price confirmation (HIGH-4 audit fix)
// ---------------------------------------------------------------------------
//
// Two admin-only handlers that gate the very-first published TWAP
// behind an explicit confirmation step. See the module-level
// `PendingBootstrapPrice` doc on `state.rs` and branch (d) of
// `update_internal_oracle_price` above for the full flow.

/// Admin-only. Reads the buffered bootstrap-price candidate (set by
/// branch (d) of `update_internal_oracle_price`), enforces the
/// `BOOTSTRAP_OBSERVATION_SECONDS` (1h) observation window from the
/// candidate's `proposed_at`, then publishes it as `last_price`
/// (decrementing `warmup_remaining` like a normal successful update
/// would). Future updates resume the steady-state breaker on
/// branch (a).
///
/// Reverts when:
///   - sender is not the factory admin
///   - no candidate is pending (admin has nothing to confirm)
///   - `block.time < proposed_at + BOOTSTRAP_OBSERVATION_SECONDS`
///     (insufficient observation window — admin must wait)
pub fn execute_confirm_bootstrap_price(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    let pending = crate::state::PENDING_BOOTSTRAP_PRICE
        .may_load(deps.storage)?
        .ok_or(ContractError::NoPendingBootstrapPriceToConfirm)?;

    let earliest_confirm = pending
        .proposed_at
        .plus_seconds(crate::state::BOOTSTRAP_OBSERVATION_SECONDS);
    if env.block.time < earliest_confirm {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Bootstrap-price observation window not yet elapsed; earliest confirm at {} \
             (proposed at {}, required window {}s)",
            earliest_confirm,
            pending.proposed_at,
            crate::state::BOOTSTRAP_OBSERVATION_SECONDS
        ))));
    }

    let mut oracle = INTERNAL_ORACLE.load(deps.storage)?;
    let current_time = env.block.time.seconds();

    oracle.bluechip_price_cache.last_price = pending.price;
    oracle.bluechip_price_cache.last_update = current_time;
    // Push the confirmed price as a real observation so the next
    // round's TWAP window has prior data to compute against. The
    // observation's `atom_pool_price` mirrors the value captured at
    // the round that produced the candidate, NOT the candidate's own
    // price (those are two different quantities — anchor-pool TWAP
    // vs. cross-pool weighted bluechip-per-atom TWAP).
    oracle
        .bluechip_price_cache
        .twap_observations
        .push(PriceObservation {
            timestamp: current_time,
            price: pending.price,
            atom_pool_price: pending.atom_pool_price,
        });
    // Cache the Pyth price + conf, mirroring the steady-state success
    // tail. Both fields are persisted together so the cache-fallback
    // re-check (in `get_bluechip_usd_price_with_meta`) can validate
    // the cached price against its sampling-time confidence interval.
    if let Ok((pyth_price, pyth_conf)) =
        query_pyth_atom_usd_price_with_conf(deps.as_ref(), &env)
    {
        oracle.bluechip_price_cache.cached_pyth_price = pyth_price;
        oracle.bluechip_price_cache.cached_pyth_timestamp = current_time;
        oracle.bluechip_price_cache.cached_pyth_conf = pyth_conf;
    }
    let warmup_remaining_before = oracle.warmup_remaining;
    oracle.warmup_remaining = oracle.warmup_remaining.saturating_sub(1);

    INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    crate::state::PENDING_BOOTSTRAP_PRICE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "confirm_bootstrap_price")
        .add_attribute("published_price", pending.price.to_string())
        .add_attribute("observation_count", pending.observation_count.to_string())
        .add_attribute("proposed_at", pending.proposed_at.to_string())
        .add_attribute(
            "warmup_remaining_before",
            warmup_remaining_before.to_string(),
        )
        .add_attribute(
            "warmup_remaining_after",
            oracle.warmup_remaining.to_string(),
        ))
}

/// Admin-only. Discards the buffered bootstrap-price candidate. The
/// next successful `UpdateOraclePrice` round in branch (d) starts
/// over with a fresh candidate and a fresh observation window.
///
/// Reverts when sender is not the factory admin or when there is no
/// pending candidate to cancel.
pub fn execute_cancel_bootstrap_price(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    if crate::state::PENDING_BOOTSTRAP_PRICE
        .may_load(deps.storage)?
        .is_none()
    {
        return Err(ContractError::NoPendingBootstrapPriceToCancel);
    }
    crate::state::PENDING_BOOTSTRAP_PRICE.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_bootstrap_price"))
}
