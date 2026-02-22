#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult,
};
use cw2::set_contract_version;

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, PendingWithdrawal, CONFIG, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS};

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
    env: Env,
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
        ExecuteMsg::ProposeWithdrawal {
            amount,
            denom,
            recipient,
        } => execute_propose_withdrawal(deps, env, info, amount, denom, recipient),
        ExecuteMsg::ExecuteWithdrawal {} => execute_withdrawal(deps, env, info),
        ExecuteMsg::CancelWithdrawal {} => execute_cancel_withdrawal(deps, info),
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
                    denom: "ubluechip".to_string(),
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

/// Step 1: record a proposed withdrawal and start the 48-hour timelock.
pub fn execute_propose_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: cosmwasm_std::Uint128,
    denom: String,
    recipient: Option<String>,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }
    if PENDING_WITHDRAWAL.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "A withdrawal is already pending. Cancel it first.",
        )));
    }

    let target = recipient.unwrap_or_else(|| info.sender.to_string());
    deps.api.addr_validate(&target)?;

    let execute_after = env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS);
    PENDING_WITHDRAWAL.save(
        deps.storage,
        &PendingWithdrawal {
            amount,
            denom: denom.clone(),
            recipient: target.clone(),
            execute_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_withdrawal")
        .add_attribute("recipient", target)
        .add_attribute("amount", amount)
        .add_attribute("denom", denom)
        .add_attribute("execute_after", execute_after.to_string()))
}

/// Step 2: execute the timelocked withdrawal after the delay has elapsed.
pub fn execute_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    let pending = PENDING_WITHDRAWAL.may_load(deps.storage)?.ok_or_else(|| {
        ContractError::Std(StdError::generic_err("No pending withdrawal to execute"))
    })?;

    if env.block.time < pending.execute_after {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Timelock not expired. Execute after: {}",
            pending.execute_after
        ))));
    }

    PENDING_WITHDRAWAL.remove(deps.storage);

    let send_msg = BankMsg::Send {
        to_address: pending.recipient.clone(),
        amount: vec![Coin {
            denom: pending.denom.clone(),
            amount: pending.amount,
        }],
    };

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "execute_withdrawal")
        .add_attribute("recipient", pending.recipient)
        .add_attribute("amount", pending.amount)
        .add_attribute("denom", pending.denom))
}

/// Cancel a pending withdrawal before it executes.
pub fn execute_cancel_withdrawal(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    if PENDING_WITHDRAWAL.may_load(deps.storage)?.is_none() {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending withdrawal to cancel",
        )));
    }

    PENDING_WITHDRAWAL.remove(deps.storage);

    Ok(Response::new().add_attribute("action", "cancel_withdrawal"))
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
