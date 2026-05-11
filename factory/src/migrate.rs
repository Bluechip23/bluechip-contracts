#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{DepsMut, Empty, Env, Response};
use cw2::{get_contract_version, set_contract_version};
use semver::Version;

use crate::error::ContractError;
use crate::{CONTRACT_NAME, CONTRACT_VERSION};

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: Empty) -> Result<Response, ContractError> {
    let stored_version = get_contract_version(deps.storage)?;
    let current: Version = CONTRACT_VERSION.parse()?;
    let stored_semver: Version = stored_version.version.parse()?;

    // Strictly reject downgrades. The chain has already replaced the wasm
    // bytecode by the time this handler runs — a no-op here just leaves
    // the cw2 version stale while running the older code. A hard `Err`
    // causes the chain to revert the migration and keep the previous
    // (newer) wasm in place.
    //
    // Equal-version migrations are allowed for idempotent re-runs;
    // strictly-greater stored is rejected.
    if stored_semver > current {
        return Err(ContractError::DowngradeRefused {
            stored: stored_semver.to_string(),
            current: current.to_string(),
        });
    }

    // M-3 migration shim. Existing deployments ran with "every threshold-
    // crossed commit pool is automatically oracle-eligible" baked into the
    // code path. The new build moves that behaviour behind the
    // `COMMIT_POOLS_AUTO_ELIGIBLE` flag (default false on fresh
    // instantiates so admins must opt in for stage 1–3 of the roadmap).
    // Set it true here on migrate so existing chains keep their current
    // oracle composition; admin can flip it off via the timelocked
    // `ProposeSetCommitPoolsAutoEligible` flow at the appropriate stage.
    // Idempotent (saving `true` twice is a no-op), so re-running the
    // migrate is safe.
    if crate::state::COMMIT_POOLS_AUTO_ELIGIBLE
        .may_load(deps.storage)?
        .is_none()
    {
        crate::state::COMMIT_POOLS_AUTO_ELIGIBLE.save(deps.storage, &true)?;
    }

    // PAIRS back-fill. Older deployments registered pools through the
    // pre-uniqueness `register_pool`, so `PAIRS` is empty even though
    // pools exist. Walk `POOLS_BY_ID` once and insert one entry per
    // pair, keeping the FIRST pool seen for any given pair (lowest
    // `pool_id`) and skipping subsequent duplicates. This preserves
    // any legacy duplicates already registered (they remain queryable
    // via `POOLS_BY_ID` / `POOLS_BY_CONTRACT_ADDRESS`) but blocks any
    // FURTHER duplicate creations of the same pair after migration —
    // which is the security-relevant invariant we care about.
    //
    // `range(..)` already iterates in ascending pool_id order, so the
    // first-seen pool wins naturally without a sort.
    //
    // Idempotent: if PAIRS is already populated (re-run migrate), the
    // `may_load` check below short-circuits each entry as a no-op.
    let pool_ids: Vec<u64> = crate::state::POOLS_BY_ID
        .keys(deps.storage, None, None, cosmwasm_std::Order::Ascending)
        .collect::<cosmwasm_std::StdResult<Vec<u64>>>()?;
    let mut backfilled: u32 = 0;
    let mut legacy_duplicates: u32 = 0;
    // POOL_ID_BY_ADDRESS reverse-index back-fill. Same walk as PAIRS,
    // no extra IO. Idempotent — `may_load`-then-save short-circuits if
    // already populated by a prior migrate or a fresh register_pool.
    let mut addr_index_backfilled: u32 = 0;
    for pool_id in pool_ids {
        let details = crate::state::POOLS_BY_ID.load(deps.storage, pool_id)?;
        let key = crate::state::canonical_pair_key(&details.pool_token_info);
        if crate::state::PAIRS.may_load(deps.storage, key.clone())?.is_none() {
            crate::state::PAIRS.save(deps.storage, key, &pool_id)?;
            backfilled += 1;
        } else {
            legacy_duplicates += 1;
        }
        if crate::state::POOL_ID_BY_ADDRESS
            .may_load(deps.storage, details.creator_pool_addr.clone())?
            .is_none()
        {
            crate::state::POOL_ID_BY_ADDRESS.save(
                deps.storage,
                details.creator_pool_addr.clone(),
                &pool_id,
            )?;
            addr_index_backfilled += 1;
        }
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from", stored_version.version)
        .add_attribute("to", CONTRACT_VERSION)
        .add_attribute("pairs_backfilled", backfilled.to_string())
        .add_attribute("legacy_duplicate_pairs_skipped", legacy_duplicates.to_string())
        .add_attribute(
            "pool_id_by_address_backfilled",
            addr_index_backfilled.to_string(),
        ))
}
