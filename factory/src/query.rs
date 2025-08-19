use cosmwasm_std::{Deps, StdResult};

use crate::msg::ConfigResponse;
use crate::state::CONFIG;


pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse { config })
}
