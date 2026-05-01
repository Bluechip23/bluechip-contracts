#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{DepsMut, Empty, Env, Response, StdError};
use cw2::{get_contract_version, set_contract_version};
use semver::Version;

use crate::error::ContractError;

const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: Empty) -> Result<Response, ContractError> {
    let stored_version = get_contract_version(deps.storage)?;
    let current: Version = CONTRACT_VERSION.parse()?;
    let stored_semver: Version = stored_version.version.parse()?;

    // M-3: strictly reject downgrades. Pre-fix this branch returned
    // `Ok(no-op)` when stored >= current, but the chain has already
    // replaced the wasm bytecode by the time this handler runs — a no-op
    // here just leaves the cw2 version stale while running the older
    // code. A hard `Err` causes the chain to revert the migration and
    // keep the previous (newer) wasm in place.
    //
    // Equal-version migrations are allowed for idempotent re-runs;
    // strictly-greater stored is rejected.
    if stored_semver > current {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Migration would downgrade factory from {} to {}; refusing.",
            stored_semver, current
        ))));
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from", stored_version.version)
        .add_attribute("to", CONTRACT_VERSION))
}
