#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdResult,
};
use cw2::set_contract_version;

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, CONFIG};

const CONTRACT_NAME: &str = "crates.io:expand-economy";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let config = Config {
        factory_address: deps.api.addr_validate(&msg.factory_address)?,
        owner: deps
            .api
            .addr_validate(&msg.owner.unwrap_or_else(|| info.sender.to_string()))?,
    };

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::ExpandEconomy(expand_economy_msg) => {
            execute_expand_economy(deps, info, expand_economy_msg)
        }
        ExecuteMsg::UpdateConfig {
            factory_address,
            owner,
        } => execute_update_config(deps, info, factory_address, owner),
        ExecuteMsg::Withdraw {
            amount,
            denom,
            recipient,
        } => execute_withdraw(deps, info, amount, denom, recipient),
    }
}

pub fn execute_expand_economy(
    deps: DepsMut,
    info: MessageInfo,
    msg: ExpandEconomyMsg,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.factory_address {
        return Err(ContractError::Unauthorized {});
    }

    match msg {
        ExpandEconomyMsg::RequestExpansion { recipient, amount } => {
            if amount.is_zero() {
                return Ok(Response::new()
                    .add_attribute("action", "request_reward_skipped")
                    .add_attribute("reason", "zero_amount"));
            }

            let send_msg = BankMsg::Send {
                to_address: recipient.clone(),
                amount: vec![Coin {
                    denom: "stake".to_string(),
                    amount,
                }],
            };

            Ok(Response::new()
                .add_message(send_msg)
                .add_attribute("action", "request_reward")
                .add_attribute("recipient", recipient)
                .add_attribute("amount", amount))
        }
    }
}

pub fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    factory_address: Option<String>,
    owner: Option<String>,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;

    // SECURITY: Only the current owner can update config
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    if let Some(factory) = factory_address {
        config.factory_address = deps.api.addr_validate(&factory)?;
    }

    if let Some(new_owner) = owner {
        config.owner = deps.api.addr_validate(&new_owner)?;
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "update_config")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner))
}

pub fn execute_withdraw(
    deps: DepsMut,
    info: MessageInfo,
    amount: cosmwasm_std::Uint128,
    denom: String,
    recipient: Option<String>,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    // SECURITY: Only the owner can withdraw
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    let target = recipient.unwrap_or_else(|| info.sender.to_string());

    let send_msg = BankMsg::Send {
        to_address: target.clone(),
        amount: vec![Coin {
            denom: denom.clone(),
            amount,
        }],
    };

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "withdraw")
        .add_attribute("recipient", target)
        .add_attribute("amount", amount)
        .add_attribute("denom", denom))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetConfig {} => to_json_binary(&query_config(deps)?),
        QueryMsg::GetBalance { denom } => {
            to_json_binary(&deps.querier.query_balance(_env.contract.address, denom)?)
        }
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        factory_address: config.factory_address,
        owner: config.owner,
    })
}
