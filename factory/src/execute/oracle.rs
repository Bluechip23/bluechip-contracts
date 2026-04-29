//! Oracle-adjacent admin handlers: bounty caps + pay-distribution-bounty
//! forward + one-shot anchor-pool set.
//!
//! The TWAP / price-update / pool-rotation logic itself lives in
//! `crate::internal_bluechip_price_oracle`; this module only wires up
//! the admin-facing bounty configuration and the one-time anchor pool
//! bootstrap that can't go through the normal 48h timelock (chicken-and-
//! egg at deploy time).

use cosmwasm_std::{
    Addr, Attribute, BankMsg, Coin, CosmosMsg, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Storage, Uint128,
};
use cw_storage_plus::Item;

use crate::error::ContractError;
use crate::state::{
    DISTRIBUTION_BOUNTY_USD, FACTORYINSTANTIATEINFO, MAX_DISTRIBUTION_BOUNTY_USD,
    MAX_ORACLE_UPDATE_BOUNTY_USD, ORACLE_BOUNTY_DENOM, ORACLE_UPDATE_BOUNTY_USD,
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
/// paid to oracle keepers. Capped by MAX_ORACLE_UPDATE_BOUNTY_USD ($1).
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

/// Admin-only. Sets the per-batch USD bounty (6 decimals, e.g. 50_000 = $0.05)
/// paid to keepers calling pool.ContinueDistribution. Capped by
/// MAX_DISTRIBUTION_BOUNTY_USD ($1). Converted to bluechip at payout time.
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

    // Convert USD -> bluechip via the internal oracle. If the oracle is
    // unavailable, skip gracefully.
    let bounty_bluechip = match crate::internal_bluechip_price_oracle::usd_to_bluechip(
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
    let balance = deps
        .querier
        .query_balance(env.contract.address.as_str(), ORACLE_BOUNTY_DENOM)?;

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
                denom: ORACLE_BOUNTY_DENOM.to_string(),
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
/// `PoolDetails` if present, or `None` if no pool in `POOLS_BY_ID` has
/// that `creator_pool_addr`. Linear scan; fires at most once per
/// `propose` / `apply` of an anchor change, so the cost is fine.
pub(crate) fn lookup_pool_by_addr(
    deps: cosmwasm_std::Deps,
    pool_addr: &cosmwasm_std::Addr,
) -> StdResult<Option<crate::pool_struct::PoolDetails>> {
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
    let new_pools = crate::internal_bluechip_price_oracle::select_random_pools_with_atom(
        deps.branch(),
        env.clone(),
        crate::internal_bluechip_price_oracle::ORACLE_POOL_COUNT,
    )?;
    oracle.selected_pools = new_pools.clone();
    oracle.atom_pool_contract_address = new_anchor_addr.clone();
    oracle.last_rotation = env.block.time.seconds();
    oracle.pool_cumulative_snapshots.clear();
    oracle.bluechip_price_cache.last_price = Uint128::zero();
    oracle.bluechip_price_cache.last_update = 0;
    oracle.bluechip_price_cache.twap_observations.clear();
    crate::internal_bluechip_price_oracle::INTERNAL_ORACLE.save(deps.storage, &oracle)?;
    Ok(new_pools.len())
}
