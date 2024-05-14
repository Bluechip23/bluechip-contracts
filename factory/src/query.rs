use cosmwasm_std::entry_point;
use cosmwasm_std::{to_binary, Addr, Binary, Deps, Env, Order, StdResult, Uint128};

use crate::msg::{ConfigResponse, QueryMsg, SubscribeInfoResponse};
use crate::state::{CONFIG, SUBSCRIBE};

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&query_config(deps)?),
        QueryMsg::SubscribeInfo { creator } => to_binary(&query_subscribe_info(deps, creator)?),
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse { config })
}

fn query_subscribe_info(deps: Deps, creator: Addr) -> StdResult<SubscribeInfoResponse> {
    let subscribe_info = SUBSCRIBE.load(deps.storage, &creator.to_string())?;
    Ok(SubscribeInfoResponse { subscribe_info })
}
