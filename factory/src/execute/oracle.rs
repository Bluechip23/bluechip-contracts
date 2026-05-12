//! Oracle-adjacent admin handlers: bounty caps + pay-distribution-bounty
//! forward + one-shot anchor-pool set.
//!
//! The TWAP / price-update / pool-rotation logic itself lives in
//! `crate::internal_bluechip_price_oracle`; this module only wires up
//! the admin-facing bounty configuration and the one-time anchor pool
//! bootstrap that can't go through the normal 48h timelock (chicken-and-
//! egg at deploy time).

use cosmwasm_std::{
    Addr, Attribute, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Storage, Uint128,
};
use cw_storage_plus::Item;

use crate::error::ContractError;
use crate::state::{
    AllowlistedOraclePool, PendingCommitPoolsAutoEligible, PendingOracleEligiblePoolAdd,
    ADMIN_TIMELOCK_SECONDS, COMMIT_POOLS_AUTO_ELIGIBLE, DISTRIBUTION_BOUNTY_USD,
    FACTORYINSTANTIATEINFO, LAST_ORACLE_REFRESH_BLOCK, MAX_DISTRIBUTION_BOUNTY_USD,
    MAX_ORACLE_UPDATE_BOUNTY_USD, ORACLE_ELIGIBLE_POOLS,
    ORACLE_REFRESH_RATE_LIMIT_BLOCKS, ORACLE_UPDATE_BOUNTY_USD,
    PENDING_COMMIT_POOLS_AUTO_ELIGIBLE, PENDING_ORACLE_ELIGIBLE_POOL_ADD,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID,
};

use super::ensure_admin;

/// Shared body for the two bounty setters (oracle-update and distribution).
/// Validates against the per-bounty cap, persists the new value, and emits
/// the standard `action` + `new_bounty_usd` attribute pair.
fn save_bounty_with_cap(
    storage: &mut dyn Storage,
    item: Item<Uint128>,
    max_bounty_usd: Uint128,
    new_bounty: Uint128,
    action: &'static str,
) -> Result<Response, ContractError> {
    if new_bounty > max_bounty_usd {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Bounty exceeds max of {} (USD, 6 decimals)",
            max_bounty_usd
        ))));
    }

    item.save(storage, &new_bounty)?;

    Ok(Response::new()
        .add_attribute("action", action)
        .add_attribute("new_bounty_usd", new_bounty.to_string()))
}

/// Builds a uniform "bounty skipped" Response for execute_pay_distribution_bounty.
/// Every skip path emits the same action+bounty_skipped+pool triple plus
/// a few path-specific extras; this keeps the call sites short and the
/// emitted attribute shape consistent.
fn pay_distribution_bounty_skip(
    reason: &'static str,
    pool: &Addr,
    extras: Vec<Attribute>,
) -> Response {
    let mut resp = Response::new()
        .add_attribute("action", "pay_distribution_bounty")
        .add_attribute("bounty_skipped", reason)
        .add_attribute("pool", pool.to_string());
    for attr in extras {
        resp = resp.add_attribute(attr.key, attr.value);
    }
    resp
}

/// Admin-only. Sets the per-call USD bounty (6 decimals, e.g. 5_000 = $0.005)
/// paid to oracle keepers. Capped by MAX_ORACLE_UPDATE_BOUNTY_USD ($0.10).
/// At payout time the value is converted to bluechip via the internal oracle.
pub fn execute_set_oracle_update_bounty(
    deps: DepsMut,
    info: MessageInfo,
    new_bounty: Uint128,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    save_bounty_with_cap(
        deps.storage,
        ORACLE_UPDATE_BOUNTY_USD,
        MAX_ORACLE_UPDATE_BOUNTY_USD,
        new_bounty,
        "set_oracle_update_bounty",
    )
}

/// Admin-only. Sets the Pyth ATOM/USD confidence-interval gate
/// (in basis points of price). Bounded to
/// `[PYTH_CONF_THRESHOLD_BPS_MIN, PYTH_CONF_THRESHOLD_BPS_MAX]` so neither
/// the admin nor a missing storage slot can disable the gate; the same
/// value is read by both the live Pyth check and the cache-fallback
/// re-check. Effect is immediate (no timelock) — tightening the gate is
/// always conservative, and the hardcoded ceiling caps how far an admin
/// can loosen it.
pub fn execute_set_pyth_conf_threshold_bps(
    deps: DepsMut,
    info: MessageInfo,
    bps: u16,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    if bps < crate::state::PYTH_CONF_THRESHOLD_BPS_MIN
        || bps > crate::state::PYTH_CONF_THRESHOLD_BPS_MAX
    {
        return Err(ContractError::Std(cosmwasm_std::StdError::generic_err(
            format!(
                "pyth_conf_threshold_bps={} out of allowed range [{}, {}]",
                bps,
                crate::state::PYTH_CONF_THRESHOLD_BPS_MIN,
                crate::state::PYTH_CONF_THRESHOLD_BPS_MAX,
            ),
        )));
    }
    let prior = crate::state::load_pyth_conf_threshold_bps(deps.storage);
    crate::state::PYTH_CONF_THRESHOLD_BPS.save(deps.storage, &bps)?;
    Ok(Response::new()
        .add_attribute("action", "set_pyth_conf_threshold_bps")
        .add_attribute("prior_bps", prior.to_string())
        .add_attribute("new_bps", bps.to_string()))
}

/// Admin-only. Sets the per-batch USD bounty (6 decimals, e.g. 50_000 = $0.05)
/// paid to keepers calling pool.ContinueDistribution. Capped by
/// MAX_DISTRIBUTION_BOUNTY_USD ($0.10). Converted to bluechip at payout time.
pub fn execute_set_distribution_bounty(
    deps: DepsMut,
    info: MessageInfo,
    new_bounty: Uint128,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    save_bounty_with_cap(
        deps.storage,
        DISTRIBUTION_BOUNTY_USD,
        MAX_DISTRIBUTION_BOUNTY_USD,
        new_bounty,
        "set_distribution_bounty",
    )
}

/// Pool-only. Called by a pool's ContinueDistribution handler to forward
/// the keeper bounty payment to the factory. The factory pays from its
/// own native reserve so pool LP funds are never used for keeper
/// infrastructure.
///
/// Skips gracefully (returns Ok with an attribute) when:
///   - the bounty is disabled (USD value is zero)
///   - the oracle conversion fails (Pyth + cache both unavailable)
///   - the factory's native balance is below the converted amount
/// Skipping rather than erroring means the pool's distribution tx never
/// reverts because of bounty payout state.
pub fn execute_pay_distribution_bounty(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recipient: String,
) -> Result<Response, ContractError> {
    // Auth: caller must be a registered COMMIT pool. POOLS_BY_CONTRACT_ADDRESS
    // is populated at pool creation and keyed by the pool's contract address;
    // it contains both commit and standard pools, so the registry-presence
    // check alone would let any registered pool drain the factory's bounty
    // reserve. Only commit pools run distributions, so we additionally
    // require `pool_kind == Commit` as defense-in-depth: if a future
    // migration of either pool wasm ever introduced a hostile or buggy
    // path that called `PayDistributionBounty`, this gate prevents standard
    // pools from triggering a payout entirely.
    if !POOLS_BY_CONTRACT_ADDRESS.has(deps.storage, info.sender.clone()) {
        return Err(ContractError::Unauthorized {});
    }
    let pool_details = lookup_pool_by_addr(deps.as_ref(), &info.sender)?
        .ok_or(ContractError::Unauthorized {})?;
    if pool_details.pool_kind != pool_factory_interfaces::PoolKind::Commit {
        return Err(ContractError::Unauthorized {});
    }

    let bounty_usd = DISTRIBUTION_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();

    if bounty_usd.is_zero() {
        return Ok(pay_distribution_bounty_skip("disabled", &info.sender, vec![]));
    }

    let bounty_usd_attr = Attribute::new("bounty_configured_usd", bounty_usd.to_string());

    // Convert USD -> bluechip via the internal oracle. Best-effort path
    // (audit fix): during the post-reset warm-up window the strict path
    // would Err and we'd skip every distribution-bounty payment for
    // ~30 min. The bounty itself is capped at $0.10 per call and the
    // pre-reset price is bounded by the 30% TWAP breaker, so falling
    // back during warm-up keeps keepers compensated without meaningful
    // mispricing risk.
    let bounty_bluechip = match crate::internal_bluechip_price_oracle::usd_to_bluechip_best_effort(
        deps.as_ref(),
        bounty_usd,
        &env,
    ) {
        Ok(conv) => conv.amount,
        Err(_) => {
            return Ok(pay_distribution_bounty_skip(
                "price_unavailable",
                &info.sender,
                vec![bounty_usd_attr],
            ));
        }
    };

    if bounty_bluechip.is_zero() {
        return Ok(pay_distribution_bounty_skip(
            "conversion_returned_zero",
            &info.sender,
            vec![bounty_usd_attr],
        ));
    }

    let recipient_addr = deps.api.addr_validate(&recipient)?;
    let bounty_cfg = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let balance = deps
        .querier
        .query_balance(env.contract.address.as_str(), &bounty_cfg.bluechip_denom)?;

    if balance.amount < bounty_bluechip {
        return Ok(pay_distribution_bounty_skip(
            "insufficient_factory_balance",
            &info.sender,
            vec![
                Attribute::new("bounty_required_bluechip", bounty_bluechip.to_string()),
                bounty_usd_attr,
                Attribute::new("factory_balance", balance.amount.to_string()),
            ],
        ));
    }

    Ok(Response::new()
        .add_message(CosmosMsg::Bank(BankMsg::Send {
            to_address: recipient_addr.to_string(),
            amount: vec![Coin {
                denom: bounty_cfg.bluechip_denom.clone(),
                amount: bounty_bluechip,
            }],
        }))
        .add_attribute("action", "pay_distribution_bounty")
        .add_attribute("bounty_paid_bluechip", bounty_bluechip.to_string())
        .add_attribute("bounty_paid_usd", bounty_usd.to_string())
        .add_attribute("recipient", recipient_addr.to_string())
        .add_attribute("pool", info.sender.to_string()))
}

/// One-shot bootstrap: admin sets the ATOM/bluechip anchor pool address
/// to a previously-created standard pool. Callable exactly once per
/// deployment; subsequent anchor changes go through the standard 48h
/// `ProposeConfigUpdate` flow.
///
/// Validates that the chosen pool:
///   - exists in the registry
///   - is a `PoolKind::Standard` pool
///   - includes the canonical bluechip denom on at least one side
///     (so the anchor is actually priceable in bluechip terms)
///
/// On success, also rotates the oracle's `selected_pools` to include the
/// new anchor immediately and clears the price cache so downstream reads
/// see "needs update" rather than the placeholder-derived (zero) value.
pub fn execute_set_anchor_pool(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    if crate::state::INITIAL_ANCHOR_SET
        .may_load(deps.storage)?
        .unwrap_or(false)
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Anchor pool has already been set; subsequent changes require ProposeConfigUpdate (48h timelock)",
        )));
    }

    let pool_details = POOLS_BY_ID.load(deps.storage, pool_id).map_err(|_| {
        ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        )))
    })?;
    let pool_addr = pool_details.creator_pool_addr.clone();

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    // Run the strict anchor checks against the chosen pool.
    validate_anchor_pool_choice(
        &pool_details,
        &factory_config.bluechip_denom,
        &factory_config.atom_denom,
    )?;

    // Update the anchor address on the factory config in-place. We don't
    // go through the timelock path here — that's the entire point of the
    // one-shot.
    FACTORYINSTANTIATEINFO.update(deps.storage, |mut cfg| -> StdResult<_> {
        cfg.atom_bluechip_anchor_pool_address = pool_addr.clone();
        Ok(cfg)
    })?;

    let pools_in_oracle = refresh_internal_oracle_for_anchor_change(
        &mut deps,
        &env,
        &pool_addr,
    )?;

    crate::state::INITIAL_ANCHOR_SET.save(deps.storage, &true)?;

    Ok(Response::new()
        .add_attribute("action", "set_anchor_pool")
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("pool_addr", pool_addr.to_string())
        .add_attribute("pools_in_oracle_after_refresh", pools_in_oracle.to_string()))
}

/// Strict shape check for an anchor-pool candidate. The anchor MUST be a
/// `PoolKind::Standard` pool whose `pool_token_info` is a Native/Native
/// pair of exactly `(bluechip_denom, atom_denom)` in either order.
/// Anything else (bluechip + arbitrary IBC denom, bluechip + CW20, atom +
/// CW20, etc.) is rejected so a compromised admin key can't point the
/// anchor at a pool whose price has no relation to the Pyth ATOM/USD
/// feed the rest of the oracle math depends on.
///
/// Shared between `execute_set_anchor_pool` and the
/// `ProposeConfigUpdate -> UpdateConfig` path so the same invariants
/// apply on both routes.
pub(crate) fn validate_anchor_pool_choice(
    pool_details: &crate::pool_struct::PoolDetails,
    bluechip_denom: &str,
    atom_denom: &str,
) -> Result<(), ContractError> {
    if pool_details.pool_kind != pool_factory_interfaces::PoolKind::Standard {
        return Err(ContractError::Std(StdError::generic_err(
            "Anchor pool must be a standard pool",
        )));
    }

    // Defense for old serialized records that round-trip with an empty
    // `atom_denom` via the field's `#[serde(default)]`.
    if atom_denom.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "atom_denom is not configured; propose a factory config update setting \
             `atom_denom` (e.g. \"uatom\" or your chain's IBC-wrapped atom denom) \
             before configuring an anchor pool.",
        )));
    }

    use crate::asset::TokenType;
    let denoms: Vec<&str> = pool_details
        .pool_token_info
        .iter()
        .filter_map(|t| match t {
            TokenType::Native { denom } => Some(denom.as_str()),
            TokenType::CreatorToken { .. } => None,
        })
        .collect();

    let valid_pair = denoms.len() == 2
        && ((denoms[0] == bluechip_denom && denoms[1] == atom_denom)
            || (denoms[0] == atom_denom && denoms[1] == bluechip_denom));
    if !valid_pair {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Anchor pool must be a Native/Native pair of exactly (bluechip \"{}\", \
             atom \"{}\") in either order; got pool with assets {:?}",
            bluechip_denom, atom_denom, pool_details.pool_token_info
        ))));
    }
    Ok(())
}

/// Look up a registered pool by its contract address. Returns the
/// `PoolDetails` if present, or `None` if no pool matches.
///
/// Fast path: `POOL_ID_BY_ADDRESS.may_load` resolves the address to a
/// `pool_id` in O(1), then `POOLS_BY_ID.load` resolves the id to the
/// full record. Both maps are written atomically inside
/// `state::register_pool`, so every pool created through the live
/// reply chain hits the fast path.
///
/// Slow-path fallback: if the reverse index misses, fall back to an
/// O(N) linear scan of `POOLS_BY_ID`. This exists ONLY so that test
/// fixtures (which historically wrote `POOLS_BY_ID` directly without
/// going through `register_pool`) continue to resolve. Hitting this
/// path in production would indicate a `POOLS_BY_ID` write that
/// bypassed `register_pool` — a bug. The fallback emits no marker on
/// chain; a future tightening could replace it with a defensive panic
/// once all test fixtures and any migrate back-fill are confirmed to
/// populate `POOL_ID_BY_ADDRESS`.
pub(crate) fn lookup_pool_by_addr(
    deps: cosmwasm_std::Deps,
    pool_addr: &cosmwasm_std::Addr,
) -> StdResult<Option<crate::pool_struct::PoolDetails>> {
    if let Some(pool_id) =
        crate::state::POOL_ID_BY_ADDRESS.may_load(deps.storage, pool_addr.clone())?
    {
        return Ok(Some(POOLS_BY_ID.load(deps.storage, pool_id)?));
    }
    use cosmwasm_std::Order;
    for entry in POOLS_BY_ID.range(deps.storage, None, None, Order::Ascending) {
        let (_id, details) = entry?;
        if &details.creator_pool_addr == pool_addr {
            return Ok(Some(details));
        }
    }
    Ok(None)
}

/// Refresh `INTERNAL_ORACLE` after the anchor pool has changed. Mirrors
/// the cleanup `execute_set_anchor_pool` performs on its one-shot path so
/// the timelocked `ProposeConfigUpdate -> UpdateConfig` flow does not leave
/// the oracle pointing at a stale anchor.
///
/// Returns the number of pools the oracle is now sampling (anchor + N
/// random eligible creator pools), useful for response attributes.
pub(crate) fn refresh_internal_oracle_for_anchor_change(
    deps: &mut DepsMut,
    env: &Env,
    new_anchor_addr: &cosmwasm_std::Addr,
) -> Result<usize, ContractError> {
    let mut oracle = crate::internal_bluechip_price_oracle::INTERNAL_ORACLE.load(deps.storage)?;

    // Resolve the new anchor's bluechip-side index from the registry
    // BEFORE mutating any oracle state, so a malformed anchor (somehow
    // missing the canonical bluechip denom — should be impossible after
    // `validate_anchor_pool_choice` but defense-in-depth) errors out
    // cleanly instead of leaving the oracle in a half-reset state.
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let canonical_bluechip = factory_config.bluechip_denom.as_str();
    let pool_details = lookup_pool_by_addr(deps.as_ref(), new_anchor_addr)?
        .ok_or_else(|| {
            ContractError::Std(StdError::generic_err(format!(
                "anchor pool {} not found in registry while refreshing oracle",
                new_anchor_addr
            )))
        })?;
    let anchor_bluechip_index = pool_details
        .pool_token_info
        .iter()
        .position(|t| matches!(t, crate::asset::TokenType::Native { denom } if denom == canonical_bluechip))
        .ok_or_else(|| ContractError::Std(StdError::generic_err(format!(
            "anchor pool {} does not contain canonical bluechip denom \"{}\"",
            new_anchor_addr, canonical_bluechip
        ))))? as u8;

    let new_pools = crate::internal_bluechip_price_oracle::select_random_pools_with_atom(
        deps.branch(),
        env.clone(),
        crate::internal_bluechip_price_oracle::ORACLE_POOL_COUNT,
    )?;
    oracle.selected_pools = new_pools.clone();
    oracle.atom_pool_contract_address = new_anchor_addr.clone();
    oracle.last_rotation = env.block.time.seconds();
    oracle.pool_cumulative_snapshots.clear();
    // Snapshot the pre-reset price for best-effort callers (audit fix).
    // The strict commit path never reads this; only `bluechip_to_usd_best_effort`
    // / `usd_to_bluechip_best_effort` (CreateStandardPool fee + bounty
    // payout) consult it during the warm-up window so the protocol
    // doesn't fully freeze on every legitimate anchor rotation.
    oracle.pre_reset_last_price = oracle.bluechip_price_cache.last_price;
    oracle.bluechip_price_cache.last_price = Uint128::zero();
    oracle.bluechip_price_cache.last_update = 0;
    oracle.bluechip_price_cache.twap_observations.clear();
    // Cache the anchor's bluechip-side index (audit fix). Replaces the
    // O(N) fallback scan over POOLS_BY_ID that previously fired once per
    // oracle update for the anchor pool.
    oracle.anchor_bluechip_index = anchor_bluechip_index;
    // Drop any pending candidate from a prior reset cycle.
    oracle.pending_first_price = None;
    // Reset the consecutive-failure counter so the new post-reset
    // window gets a fresh budget of (c)-failure rounds before the
    // force-accept liveness valve fires.
    oracle.post_reset_consecutive_failures = 0;
    // Arm the warm-up counter on every anchor reset. With the spot
    // fallbacks removed, the very-first post-reset price comes from a
    // TWAP computed against snapshots taken on this very call;
    // until enough additional successful updates accumulate, downstream
    // pricing is held off rather than allowing a single attacker-influenced
    // observation to be served as authoritative. See the warm-up gate
    // in `get_bluechip_usd_price_with_meta`.
    oracle.warmup_remaining =
        crate::internal_bluechip_price_oracle::ANCHOR_CHANGE_WARMUP_OBSERVATIONS;
    crate::internal_bluechip_price_oracle::INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    // H-2 audit fix: clear any pre-confirm bootstrap candidate so an
    // anchor change before the first `ConfirmBootstrapPrice` does not
    // leave a stale candidate with its old `proposed_at` lying around
    // for branch (d) of update_internal_oracle_price to pick back up.
    crate::state::PENDING_BOOTSTRAP_PRICE.remove(deps.storage);
    Ok(new_pools.len())
}

// ===========================================================================
// Oracle eligibility curation (M-3 audit fix).
//
// Two parallel inputs feed the snapshot rebuild:
//   - `ORACLE_ELIGIBLE_POOLS` — admin-curated allowlist (any pool kind).
//     Add: 48h timelock. Remove: immediate.
//   - `COMMIT_POOLS_AUTO_ELIGIBLE` — global flag; when true, threshold-
//     crossed `PoolKind::Commit` pools also flow in. Flip: 48h timelock.
//
// All admin handlers reuse `ADMIN_TIMELOCK_SECONDS` (48h) so the
// observability window matches the rest of the factory's mutation paths.
// ===========================================================================

/// Resolve the bluechip-side index (0 or 1) for a pool currently in
/// `POOLS_BY_ID`. Returns the pool's `PoolDetails` alongside the index for
/// callers (allowlist propose / apply) that need both. Errors with
/// `OracleEligiblePoolNotInRegistry` for unknown addresses and
/// `OracleEligiblePoolMissingBluechipSide` for pools whose `pool_token_info`
/// doesn't contain a `Native` entry matching the configured bluechip denom.
fn resolve_pool_for_allowlist(
    deps: Deps,
    pool_addr: &Addr,
) -> Result<(crate::pool_struct::PoolDetails, u8), ContractError> {
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let bluechip_denom = factory_config.bluechip_denom.as_str();
    let pool_details = lookup_pool_by_addr(deps, pool_addr)?.ok_or_else(|| {
        ContractError::OracleEligiblePoolNotInRegistry {
            pool_addr: pool_addr.to_string(),
        }
    })?;
    let bluechip_index = pool_details
        .pool_token_info
        .iter()
        .position(|t| matches!(t, crate::asset::TokenType::Native { denom } if denom == bluechip_denom))
        .ok_or_else(|| ContractError::OracleEligiblePoolMissingBluechipSide {
            pool_addr: pool_addr.to_string(),
        })? as u8;

    // Commit pools cannot be allowlisted before their threshold has been
    // crossed. A pre-threshold commit pool has no LP and no real swap
    // activity, so its cumulative-delta TWAP is either zero (no activity)
    // or determined entirely by seeded reserves — neither is a
    // meaningful oracle contributor. The auto-eligible source already
    // gates on `POOL_THRESHOLD_MINTED` (in
    // `internal_bluechip_price_oracle::get_eligible_creator_pools`);
    // mirroring the gate here keeps the admin-curated and auto-eligible
    // sources from disagreeing on what counts as a valid oracle pool,
    // and prevents an admin from burning a 48h timelock cycle on a
    // pool that would fail the same gate at sample time anyway.
    // Standard pools have no threshold concept and are exempt.
    if pool_details.pool_kind == pool_factory_interfaces::PoolKind::Commit {
        let threshold_minted = crate::state::POOL_THRESHOLD_MINTED
            .may_load(deps.storage, pool_details.pool_id)?
            .unwrap_or(false);
        if !threshold_minted {
            return Err(ContractError::OracleEligiblePoolCommitPreThreshold {
                pool_addr: pool_addr.to_string(),
            });
        }
    }

    Ok((pool_details, bluechip_index))
}

/// Admin-only. Stage a pool address for inclusion in the oracle allowlist.
/// Validates pool existence + bluechip-side resolution at propose time so
/// the timelock isn't burned on a pool that can't possibly be eligible;
/// the same validation runs again at apply time as defense in depth.
pub fn execute_propose_add_oracle_eligible_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_addr: String,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    let pool_addr = deps.api.addr_validate(&pool_addr)?;

    if ORACLE_ELIGIBLE_POOLS.has(deps.storage, pool_addr.clone()) {
        return Err(ContractError::OracleEligiblePoolAlreadyAdded {
            pool_addr: pool_addr.to_string(),
        });
    }
    if PENDING_ORACLE_ELIGIBLE_POOL_ADD.has(deps.storage, pool_addr.clone()) {
        return Err(ContractError::OracleEligiblePoolAddAlreadyPending {
            pool_addr: pool_addr.to_string(),
        });
    }

    let (_pool_details, bluechip_index) =
        resolve_pool_for_allowlist(deps.as_ref(), &pool_addr)?;

    PENDING_ORACLE_ELIGIBLE_POOL_ADD.save(
        deps.storage,
        pool_addr.clone(),
        &PendingOracleEligiblePoolAdd {
            proposed_at: env.block.time,
            bluechip_index,
        },
    )?;

    let effective_after = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS);
    Ok(Response::new()
        .add_attribute("action", "propose_add_oracle_eligible_pool")
        .add_attribute("pool_addr", pool_addr.to_string())
        .add_attribute("bluechip_index", bluechip_index.to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

/// Admin-only. Apply a previously-proposed allowlist add. Re-resolves the
/// pool's bluechip-side index against current registry state — if the pool
/// somehow lost its bluechip side between propose and apply (which
/// shouldn't be possible, but isn't disprovable in storage), the apply
/// fails closed rather than landing a malformed entry.
pub fn execute_apply_add_oracle_eligible_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_addr: String,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    let pool_addr = deps.api.addr_validate(&pool_addr)?;

    let pending = PENDING_ORACLE_ELIGIBLE_POOL_ADD
        .may_load(deps.storage, pool_addr.clone())?
        .ok_or_else(|| ContractError::NoPendingOracleEligiblePoolAdd {
            pool_addr: pool_addr.to_string(),
        })?;

    let effective_after = pending
        .proposed_at
        .plus_seconds(ADMIN_TIMELOCK_SECONDS);
    if env.block.time < effective_after {
        return Err(ContractError::TimelockNotExpired { effective_after });
    }

    // Re-validate. The propose-time `bluechip_index` was captured against the
    // pool's token info at that moment; we re-resolve to catch the unlikely
    // case of a registry mutation between propose and apply, and to keep the
    // post-apply allowlist entry's `bluechip_index` aligned with present
    // reality.
    let (_pool_details, bluechip_index) =
        resolve_pool_for_allowlist(deps.as_ref(), &pool_addr)?;

    ORACLE_ELIGIBLE_POOLS.save(
        deps.storage,
        pool_addr.clone(),
        &AllowlistedOraclePool {
            bluechip_index,
            added_at: env.block.time,
        },
    )?;
    PENDING_ORACLE_ELIGIBLE_POOL_ADD.remove(deps.storage, pool_addr.clone());

    Ok(Response::new()
        .add_attribute("action", "apply_add_oracle_eligible_pool")
        .add_attribute("pool_addr", pool_addr.to_string())
        .add_attribute("bluechip_index", bluechip_index.to_string()))
}

/// Admin-only. Discard a pending allowlist add before the timelock has
/// expired. Errors if there is no matching pending entry.
pub fn execute_cancel_add_oracle_eligible_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_addr: String,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    let pool_addr = deps.api.addr_validate(&pool_addr)?;

    if !PENDING_ORACLE_ELIGIBLE_POOL_ADD.has(deps.storage, pool_addr.clone()) {
        return Err(ContractError::NoPendingOracleEligiblePoolAdd {
            pool_addr: pool_addr.to_string(),
        });
    }
    PENDING_ORACLE_ELIGIBLE_POOL_ADD.remove(deps.storage, pool_addr.clone());

    Ok(Response::new()
        .add_attribute("action", "cancel_add_oracle_eligible_pool")
        .add_attribute("pool_addr", pool_addr.to_string()))
}

/// Admin-only. Drop a pool from the oracle allowlist. Effect is immediate
/// (no timelock) — removing a contributor is always safe relative to oracle
/// integrity, and the breaker / next-snapshot-refresh handle the
/// recomputation cleanly.
pub fn execute_remove_oracle_eligible_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_addr: String,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    let pool_addr = deps.api.addr_validate(&pool_addr)?;

    if !ORACLE_ELIGIBLE_POOLS.has(deps.storage, pool_addr.clone()) {
        return Err(ContractError::OracleEligiblePoolNotAllowlisted {
            pool_addr: pool_addr.to_string(),
        });
    }
    ORACLE_ELIGIBLE_POOLS.remove(deps.storage, pool_addr.clone());

    Ok(Response::new()
        .add_attribute("action", "remove_oracle_eligible_pool")
        .add_attribute("pool_addr", pool_addr.to_string()))
}

/// Admin-only. Stage a flip of `COMMIT_POOLS_AUTO_ELIGIBLE`. Both
/// directions (ON→OFF and OFF→ON) go through the same 48h timelock so
/// creator-pool operators losing oracle weight have the same
/// observability window as new operators gaining it.
pub fn execute_propose_set_commit_pools_auto_eligible(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    enabled: bool,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    if PENDING_COMMIT_POOLS_AUTO_ELIGIBLE.may_load(deps.storage)?.is_some() {
        return Err(ContractError::CommitPoolsAutoEligibleAlreadyPending);
    }

    let current = crate::state::load_commit_pools_auto_eligible(deps.storage);
    if current == enabled {
        return Err(ContractError::CommitPoolsAutoEligibleNoChange { value: enabled });
    }

    PENDING_COMMIT_POOLS_AUTO_ELIGIBLE.save(
        deps.storage,
        &PendingCommitPoolsAutoEligible {
            new_value: enabled,
            proposed_at: env.block.time,
        },
    )?;

    let effective_after = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS);
    Ok(Response::new()
        .add_attribute("action", "propose_set_commit_pools_auto_eligible")
        .add_attribute("new_value", enabled.to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

/// Admin-only. Apply a previously-proposed flag flip after the timelock.
pub fn execute_apply_set_commit_pools_auto_eligible(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    let pending = PENDING_COMMIT_POOLS_AUTO_ELIGIBLE
        .may_load(deps.storage)?
        .ok_or(ContractError::NoPendingCommitPoolsAutoEligible)?;

    let effective_after = pending
        .proposed_at
        .plus_seconds(ADMIN_TIMELOCK_SECONDS);
    if env.block.time < effective_after {
        return Err(ContractError::TimelockNotExpired { effective_after });
    }

    COMMIT_POOLS_AUTO_ELIGIBLE.save(deps.storage, &pending.new_value)?;
    PENDING_COMMIT_POOLS_AUTO_ELIGIBLE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "apply_set_commit_pools_auto_eligible")
        .add_attribute("new_value", pending.new_value.to_string()))
}

/// Admin-only. Discard a pending flag flip before the timelock has expired.
pub fn execute_cancel_set_commit_pools_auto_eligible(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    if PENDING_COMMIT_POOLS_AUTO_ELIGIBLE.may_load(deps.storage)?.is_none() {
        return Err(ContractError::NoPendingCommitPoolsAutoEligible);
    }
    PENDING_COMMIT_POOLS_AUTO_ELIGIBLE.remove(deps.storage);

    Ok(Response::new().add_attribute("action", "cancel_set_commit_pools_auto_eligible"))
}

/// Permissionless. Force a rebuild of `ELIGIBLE_POOL_SNAPSHOT` from the
/// current allowlist + auto-flag inputs. Rate-limited via
/// `ORACLE_REFRESH_RATE_LIMIT_BLOCKS` so it can't be spammed; never
/// changes which pools are eligible (that's controlled by the admin
/// inputs above), only when the snapshot reflects them.
pub fn execute_refresh_oracle_pool_snapshot(
    mut deps: DepsMut,
    env: Env,
) -> Result<Response, ContractError> {
    let current_block = env.block.height;
    if let Some(last) = LAST_ORACLE_REFRESH_BLOCK.may_load(deps.storage)? {
        let next_allowed = last.saturating_add(ORACLE_REFRESH_RATE_LIMIT_BLOCKS);
        if current_block < next_allowed {
            return Err(ContractError::OracleRefreshRateLimited {
                next_block: next_allowed,
            });
        }
    }

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let atom_pool_addr = factory_config
        .atom_bluechip_anchor_pool_address
        .to_string();
    let (pool_addresses, bluechip_indices) =
        crate::internal_bluechip_price_oracle::get_eligible_creator_pools(
            deps.as_ref(),
            &env,
            &atom_pool_addr,
        )?;
    crate::state::ELIGIBLE_POOL_SNAPSHOT.save(
        deps.storage,
        &crate::state::EligiblePoolSnapshot {
            pool_addresses: pool_addresses.clone(),
            bluechip_indices,
            captured_at_block: current_block,
        },
    )?;
    LAST_ORACLE_REFRESH_BLOCK.save(deps.storage, &current_block)?;

    let _ = &mut deps; // borrow already moved into get_eligible_creator_pools via as_ref
    Ok(Response::new()
        .add_attribute("action", "refresh_oracle_pool_snapshot")
        .add_attribute("eligible_count", pool_addresses.len().to_string())
        .add_attribute("captured_at_block", current_block.to_string()))
}
