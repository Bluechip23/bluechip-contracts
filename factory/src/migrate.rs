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

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from", stored_version.version)
        .add_attribute("to", CONTRACT_VERSION))
}
