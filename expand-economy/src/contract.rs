#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult, Storage, Uint128,
};
use cw2::set_contract_version;
use cw_storage_plus::Item;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
use crate::state::{
    Config, PendingConfigUpdate, PendingWithdrawal, CONFIG, CONFIG_TIMELOCK_SECONDS,
    DEFAULT_BLUECHIP_DENOM, PENDING_CONFIG_UPDATE, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS,
};

const CONTRACT_NAME: &str = "crates.io:expand-economy";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Load `CONFIG` and require the sender to match `config.owner`.
fn load_config_as_owner(storage: &dyn Storage, sender: &Addr) -> Result<Config, ContractError> {
    let config = CONFIG.load(storage)?;
    if sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }
    Ok(config)
}

/// Error with `err_msg` if `item` is already populated.
fn ensure_absent<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err_msg: &str,
) -> Result<(), ContractError>
where
    T: Serialize + DeserializeOwned,
{
    if item.may_load(storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(err_msg)));
    }
    Ok(())
}

/// Load `item` or return `ContractError::Std(generic_err(err_msg))`.
fn load_or_err<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err_msg: &str,
) -> Result<T, ContractError>
where
    T: Serialize + DeserializeOwned,
{
    item.may_load(storage)?
        .ok_or_else(|| ContractError::Std(StdError::generic_err(err_msg)))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let bluechip_denom = msg
        .bluechip_denom
        .unwrap_or_else(|| DEFAULT_BLUECHIP_DENOM.to_string());
    if bluechip_denom.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "bluechip_denom must be non-empty",
        )));
    }

    let config = Config {
        factory_address: deps.api.addr_validate(&msg.factory_address)?,
        owner: deps
            .api
            .addr_validate(&msg.owner.unwrap_or_else(|| info.sender.to_string()))?,
        bluechip_denom,
    };

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner)
        .add_attribute("bluechip_denom", config.bluechip_denom))
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
            bluechip_denom,
        } => execute_propose_config_update(deps, env, info, factory_address, owner, bluechip_denom),
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
                    denom: config.bluechip_denom.clone(),
                    amount,
                }],
            };

            Ok(Response::new()
                .add_message(send_msg)
                .add_attribute("action", "request_reward")
                .add_attribute("recipient", recipient)
                .add_attribute("amount", amount)
                .add_attribute("denom", config.bluechip_denom))
        }
    }
}

pub fn execute_propose_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    factory_address: Option<String>,
    owner: Option<String>,
    bluechip_denom: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "A config update is already pending. Cancel it first.",
    )?;

    // Validate addresses early so invalid proposals fail at propose time
    if let Some(ref addr) = factory_address {
        deps.api.addr_validate(addr)?;
    }
    if let Some(ref addr) = owner {
        deps.api.addr_validate(addr)?;
    }
    // Reject empty/whitespace denom at propose time so the mistake surfaces
    // 48h earlier than it otherwise would (when someone tries to apply it
    // and every subsequent RequestExpansion breaks).
    if let Some(ref d) = bluechip_denom {
        if d.trim().is_empty() {
            return Err(ContractError::Std(StdError::generic_err(
                "bluechip_denom must be non-empty",
            )));
        }
    }

    let effective_after = env.block.time.plus_seconds(CONFIG_TIMELOCK_SECONDS);

    PENDING_CONFIG_UPDATE.save(
        deps.storage,
        &PendingConfigUpdate {
            factory_address,
            owner,
            bluechip_denom,
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
    let mut config = load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "No pending config update to execute",
    )?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Timelock not expired. Execute after: {}",
            pending.effective_after
        ))));
    }

    if let Some(factory) = pending.factory_address {
        config.factory_address = deps.api.addr_validate(&factory)?;
    }
    if let Some(new_owner) = pending.owner {
        config.owner = deps.api.addr_validate(&new_owner)?;
    }
    if let Some(new_denom) = pending.bluechip_denom {
        // Non-empty was already enforced at propose time; re-check here in
        // case a migration ever inserts a PendingConfigUpdate directly.
        if new_denom.trim().is_empty() {
            return Err(ContractError::Std(StdError::generic_err(
                "bluechip_denom must be non-empty",
            )));
        }
        config.bluechip_denom = new_denom;
    }

    CONFIG.save(deps.storage, &config)?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "execute_config_update")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner)
        .add_attribute("bluechip_denom", config.bluechip_denom))
}

pub fn execute_cancel_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    load_or_err(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "No pending config update to cancel",
    )?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

pub fn execute_propose_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
    denom: String,
    recipient: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "A withdrawal is already pending. Cancel it first.",
    )?;

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

pub fn execute_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "No pending withdrawal to execute",
    )?;

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

pub fn execute_cancel_withdrawal(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    load_or_err(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "No pending withdrawal to cancel",
    )?;
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
        bluechip_denom: config.bluechip_denom,
    })
}
