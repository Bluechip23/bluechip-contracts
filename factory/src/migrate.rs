#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{DepsMut, Empty, Env, Response};
use cw2::{get_contract_version, set_contract_version};
use semver::Version;

use crate::error::ContractError;

const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: Empty) -> Result<Response, ContractError> {
    let stored_version = get_contract_version(deps.storage)?;
    let version: Version = CONTRACT_VERSION.parse()?;
    let stored_semver: Version = stored_version.version.parse()?;

    if stored_semver >= version {
        return Ok(Response::new());
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from", stored_version.version)
        .add_attribute("to", CONTRACT_VERSION))
}
