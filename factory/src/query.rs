use cosmwasm_std::{Deps, StdResult};

use crate::msg::ConfigResponse;
use crate::state::CONFIG;

/// This function is a regular helper function, not a top-level entry point.
/// Do NOT add #[entry_point] here.
pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse { config })
}
