//! Router contract entry points.
//!
//! This module contains [`instantiate`], [`execute`], [`query`] and the
//! handlers for the messages that are fully implemented in Phase 2:
//! instantiate, [`ExecuteMsg::UpdateConfig`], and [`QueryMsg::Config`].
//! Multi-hop execution and simulation are dispatched here but live in
//! [`crate::execution`] and [`crate::simulation`] respectively.

use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult,
};
use cw2::set_contract_version;

use crate::error::RouterError;
use crate::execution::execute_multi_hop;
use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::simulation::simulate_multi_hop;
use crate::state::{Config, CONFIG};

const CONTRACT_NAME: &str = "crates.io:bluechip-router";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, RouterError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let config = Config {
        factory_addr: deps.api.addr_validate(&msg.factory_addr)?,
        bluechip_denom: msg.bluechip_denom,
        admin: deps.api.addr_validate(&msg.admin)?,
    };
    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("factory_addr", config.factory_addr)
        .add_attribute("bluechip_denom", config.bluechip_denom)
        .add_attribute("admin", config.admin))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, RouterError> {
    match msg {
        ExecuteMsg::ExecuteMultiHop {
            operations,
            minimum_receive,
            belief_price,
            max_spread,
            deadline,
            recipient,
        } => execute_multi_hop(
            deps,
            env,
            info,
            operations,
            minimum_receive,
            belief_price,
            max_spread,
            deadline,
            recipient,
        ),
        ExecuteMsg::UpdateConfig {
            admin,
            factory_addr,
        } => execute_update_config(deps, info, admin, factory_addr),
    }
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    new_admin: Option<String>,
    new_factory_addr: Option<String>,
) -> Result<Response, RouterError> {
    let mut config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin {
        return Err(RouterError::Unauthorized);
    }

    if let Some(admin) = new_admin {
        config.admin = deps.api.addr_validate(&admin)?;
    }
    if let Some(factory) = new_factory_addr {
        config.factory_addr = deps.api.addr_validate(&factory)?;
    }
    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "update_config")
        .add_attribute("admin", config.admin)
        .add_attribute("factory_addr", config.factory_addr))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::SimulateMultiHop {
            operations,
            offer_amount,
        } => {
            let response = simulate_multi_hop(deps, operations, offer_amount)
                .map_err(|err| cosmwasm_std::StdError::generic_err(err.to_string()))?;
            to_json_binary(&response)
        }
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        factory_addr: config.factory_addr,
        bluechip_denom: config.bluechip_denom,
        admin: config.admin,
    })
}
