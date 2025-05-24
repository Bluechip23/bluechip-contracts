use cosmwasm_std::entry_point;
use cosmwasm_std::{Deps, StdResult};

use crate::msg::{ConfigResponse};
use crate::state::{CONFIG};

#[cfg_attr(not(feature = "library"), entry_point)]

#[allow(dead_code)]
fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse { config })
}
