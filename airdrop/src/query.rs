use cosmwasm_std::entry_point;
use cosmwasm_std::{to_json_binary, Addr, Binary, Deps, Env, StdResult};

use crate::msg::{ConfigResponse, QueryMsg, StatusResponse};
use crate::state::{CLAIMED, STATE, WHITELISTED};

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::IsWhitelisted { address } => {
            to_json_binary(&query_is_whitelisted(deps, env, address)?)
        }
        QueryMsg::IsClaimed { address } => to_json_binary(&query_is_claimed(deps, env, address)?),
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = STATE.load(deps.storage)?;
    Ok(ConfigResponse { config })
}

fn query_is_whitelisted(deps: Deps, _env: Env, address: Addr) -> StdResult<StatusResponse> {
    let whitelisted = WHITELISTED.may_load(deps.storage, &address)?;
    if whitelisted == Some(true) {
        return Ok(StatusResponse { status: true });
    } else {
        return Ok(StatusResponse { status: false });
    }
}

fn query_is_claimed(deps: Deps, _env: Env, address: Addr) -> StdResult<StatusResponse> {
    let claimed = CLAIMED.may_load(deps.storage, &address)?;
    if claimed == Some(true) {
        return Ok(StatusResponse { status: true });
    } else {
        return Ok(StatusResponse { status: false });
    }
}