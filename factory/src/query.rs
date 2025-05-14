use cosmwasm_std::entry_point;
use cosmwasm_std::{to_json_binary, Addr, Binary, Deps, Env, StdResult};

use crate::msg::{ConfigResponse};
use crate::state::{CONFIG, SUBSCRIBE, SubscribeInfo};

#[cfg_attr(not(feature = "library"), entry_point)]


fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse { config })
}
