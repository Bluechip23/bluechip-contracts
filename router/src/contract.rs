//! Router contract entry points.
//!
//! This module wires the external [`crate::msg`] surface to the
//! handlers in [`crate::execution`] and [`crate::simulation`]. Only the
//! `instantiate`, `UpdateConfig`, and `Config` query handlers live
//! directly in this file -- everything else is delegated.

use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    StdResult,
};
use cw2::set_contract_version;

use crate::error::RouterError;
use crate::execution::{
    execute_assert_received, execute_multi_hop, execute_receive_cw20, execute_swap_operation,
    handle_reply,
};
use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::simulation::simulate_multi_hop;
use crate::state::{Config, PendingConfigUpdate, CONFIG, PENDING_CONFIG, ROUTER_TIMELOCK_SECONDS};

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
            deadline,
            recipient,
        } => execute_multi_hop(
            deps,
            env,
            info,
            operations,
            minimum_receive,
            deadline,
            recipient,
        ),
        ExecuteMsg::ProposeConfigUpdate {
            admin,
            factory_addr,
        } => execute_propose_config_update(deps, env, info, admin, factory_addr),
        ExecuteMsg::UpdateConfig {} => execute_apply_config_update(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_config_update(deps, info),
        ExecuteMsg::Receive(cw20_msg) => execute_receive_cw20(deps, env, info, cw20_msg),
        ExecuteMsg::ExecuteSwapOperation {
            operation,
            hop_index,
            to,
        } => execute_swap_operation(deps, env, info, operation, hop_index, to),
        ExecuteMsg::AssertReceived {
            ask_info,
            recipient,
            prev_balance,
            minimum_receive,
        } => execute_assert_received(
            deps,
            env,
            info,
            ask_info,
            recipient,
            prev_balance,
            minimum_receive,
        ),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, RouterError> {
    handle_reply(deps, env, msg)
}

/// Step 1 of the 48h timelocked config rotation. Admin-only. Validates
/// any address fields up front so a malformed proposal fails immediately
/// instead of 48h later at apply.
fn execute_propose_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    new_admin: Option<String>,
    new_factory_addr: Option<String>,
) -> Result<Response, RouterError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin {
        return Err(RouterError::Unauthorized);
    }

    // Reject re-propose while a prior proposal is still pending. Without
    // this gate, a benign-looking proposal observed by the community
    // could be silently swapped for a hostile one minutes before the
    // window elapses; a watcher polling `PENDING_CONFIG` would see "still
    // pending" without an explicit cancellation event in between.
    if PENDING_CONFIG.may_load(deps.storage)?.is_some() {
        return Err(RouterError::ConfigUpdateAlreadyPending);
    }

    // Early validation: addr_validate the candidate fields now so a
    // malformed proposal fails at propose time rather than 48h later.
    if let Some(addr) = &new_admin {
        deps.api.addr_validate(addr)?;
    }
    if let Some(addr) = &new_factory_addr {
        deps.api.addr_validate(addr)?;
    }

    let effective_after = env.block.time.plus_seconds(ROUTER_TIMELOCK_SECONDS);
    PENDING_CONFIG.save(
        deps.storage,
        &PendingConfigUpdate {
            admin: new_admin,
            factory_addr: new_factory_addr,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_config_update")
        .add_attribute("effective_after", effective_after.to_string()))
}

/// Step 2 of the timelocked flow. Admin-only. Applies the pending
/// proposal once `effective_after` has elapsed.
fn execute_apply_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, RouterError> {
    let mut config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin {
        return Err(RouterError::Unauthorized);
    }

    let pending = PENDING_CONFIG
        .may_load(deps.storage)?
        .ok_or(RouterError::NoPendingConfigUpdate)?;

    if env.block.time < pending.effective_after {
        return Err(RouterError::TimelockNotExpired {
            effective_after: pending.effective_after.seconds(),
        });
    }

    if let Some(admin) = pending.admin {
        config.admin = deps.api.addr_validate(&admin)?;
    }
    if let Some(factory) = pending.factory_addr {
        config.factory_addr = deps.api.addr_validate(&factory)?;
    }

    CONFIG.save(deps.storage, &config)?;
    PENDING_CONFIG.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "update_config")
        .add_attribute("admin", config.admin)
        .add_attribute("factory_addr", config.factory_addr))
}

/// Cancels a pending proposal before its `effective_after`. Admin-only.
fn execute_cancel_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, RouterError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.admin {
        return Err(RouterError::Unauthorized);
    }
    if PENDING_CONFIG.may_load(deps.storage)?.is_none() {
        return Err(RouterError::NoPendingConfigUpdate);
    }
    PENDING_CONFIG.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
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
