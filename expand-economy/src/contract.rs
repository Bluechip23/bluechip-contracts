#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128,
};
use cw2::set_contract_version;

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
use crate::state::{
    Config, PendingConfigUpdate, PendingWithdrawal, CONFIG, CONFIG_TIMELOCK_SECONDS,
    PENDING_CONFIG_UPDATE, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS,
};

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
        ExecuteMsg::ProposeConfigUpdate {
            factory_address,
            owner,
        } => execute_propose_config_update(deps, env, info, factory_address, owner),
        ExecuteMsg::ExecuteConfigUpdate {} => execute_apply_config_update(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_config_update(deps, info),
        ExecuteMsg::ProposeWithdrawal {
            amount,
            denom,
            recipient,
        } => execute_propose_withdrawal(deps, env, info, amount, denom, recipient),
        ExecuteMsg::ExecuteWithdrawal {} => execute_withdrawal(deps, env, info),
        ExecuteMsg::CancelWithdrawal {} => execute_cancel_withdrawal(deps, info),
    }
}

//triggered when pool crosses threshold. Sends specified amount of ubluechip to the recipient address. Only callable by factory.
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

// F2-H1: Config updates now use a 48-hour timelock. This prevents a
// compromised owner key from instantly changing factory_address to drain
// funds via RequestExpansion, bypassing the withdrawal timelock.

pub fn execute_propose_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    factory_address: Option<String>,
    owner: Option<String>,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    if PENDING_CONFIG_UPDATE.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "A config update is already pending. Cancel it first.",
        )));
    }

    // Validate addresses early so invalid proposals fail at propose time
    if let Some(ref addr) = factory_address {
        deps.api.addr_validate(addr)?;
    }
    if let Some(ref addr) = owner {
        deps.api.addr_validate(addr)?;
    }

    let effective_after = env.block.time.plus_seconds(CONFIG_TIMELOCK_SECONDS);

    PENDING_CONFIG_UPDATE.save(
        deps.storage,
        &PendingConfigUpdate {
            factory_address,
            owner,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_config_update")
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_apply_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    let pending = PENDING_CONFIG_UPDATE.may_load(deps.storage)?.ok_or_else(|| {
        ContractError::Std(StdError::generic_err("No pending config update to execute"))
    })?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Timelock not expired. Execute after: {}",
            pending.effective_after
        ))));
    }

    let mut config = config;
    if let Some(factory) = pending.factory_address {
        config.factory_address = deps.api.addr_validate(&factory)?;
    }
    if let Some(new_owner) = pending.owner {
        config.owner = deps.api.addr_validate(&new_owner)?;
    }

    CONFIG.save(deps.storage, &config)?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "execute_config_update")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner))
}

pub fn execute_cancel_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }

    if PENDING_CONFIG_UPDATE.may_load(deps.storage)?.is_none() {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending config update to cancel",
        )));
    }

    PENDING_CONFIG_UPDATE.remove(deps.storage);

    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

// only one at a time, and only owner can call
pub fn execute_propose_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
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

// execute the timelocked withdrawal after the delay has elapsed. 48hrs
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

// Cancel a pending withdrawal before it executes.
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
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetConfig {} => to_json_binary(&query_config(deps)?),
        QueryMsg::GetBalance { denom } => {
            to_json_binary(&deps.querier.query_balance(env.contract.address, denom)?)
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
