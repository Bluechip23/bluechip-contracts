use cosmwasm_std::{entry_point, DepsMut, Empty, Env, Response};
use cw2::{get_contract_version, set_contract_version};
use semver::Version;

use crate::{error::ContractError, state::FACTORYINSTANTIATEINFO};

const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, _msg: Empty) -> Result<Response, ContractError> {
    // Load current contract version from storage
    let stored_version = get_contract_version(deps.storage)?;
    let version: Version = CONTRACT_VERSION.parse()?;
    let stored_semver: Version = stored_version.version.parse()?;

    // If the stored version is already newer or equal, no migration needed
    if stored_semver >= version {
        return Ok(Response::new());
    }

    // Example: perform migrations for versions before 2.0.0
    if stored_semver < Version::parse("2.0.0")? {
        let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
        // Apply migrations...
        FACTORYINSTANTIATEINFO.save(deps.storage, &config)?;
    }

    // Update version info in storage
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from", stored_version.version)
        .add_attribute("to", CONTRACT_VERSION))
}
