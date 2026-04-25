#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult, Storage, Uint128,
};
use cosmwasm_schema::cw_serde;
use cw2::set_contract_version;
use cw_storage_plus::Item;
use serde::{de::DeserializeOwned, Serialize};

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
use crate::state::{
    Config, ExpansionWindow, PendingConfigUpdate, PendingWithdrawal, CONFIG,
    CONFIG_TIMELOCK_SECONDS, DAILY_EXPANSION_CAP, DAILY_WINDOW_SECONDS, DEFAULT_BLUECHIP_DENOM,
    EXPANSION_WINDOW, PENDING_CONFIG_UPDATE, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS,
};

/// Minimal subset of the factory's query interface that this contract
/// uses to cross-validate `bluechip_denom`. Defined locally to avoid a
/// compile-time dependency on the `factory` crate (the two communicate
/// only over wasm message boundaries). Wire format must mirror
/// `factory::query::QueryMsg::Factory {}` exactly.
#[cw_serde]
enum FactoryQuery {
    Factory {},
}

/// Wire-compatible subset of `factory::msg::FactoryInstantiateResponse`
/// — only the field this contract reads. Round-trips against the full
/// factory response because `cw_serde` does not set
/// `deny_unknown_fields`, so the extra factory-side fields are ignored
/// during deserialization.
#[cw_serde]
struct FactoryConfigSubset {
    bluechip_denom: String,
}

#[cw_serde]
struct FactoryInstantiateResponseSubset {
    factory: FactoryConfigSubset,
}

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
            execute_expand_economy(deps, env, info, expand_economy_msg)
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
    env: Env,
    info: MessageInfo,
    msg: ExpandEconomyMsg,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.factory_address {
        return Err(ContractError::Unauthorized {});
    }

    // Cross-validate the factory's `bluechip_denom` against this contract's
    // configured denom. Both fields are independently admin-mutable (each
    // contract has its own propose/apply config flow with separate 48h
    // timelocks), so they can drift if a single-side update is forgotten.
    // Drift would silently fund rewards in the wrong denom — better to
    // refuse the call and surface the mismatch loudly.
    //
    // One additional cross-contract query per RequestExpansion. Cost is
    // negligible: the call fires only on threshold-crossing events, not
    // on hot paths.
    let factory_resp: FactoryInstantiateResponseSubset = deps
        .querier
        .query_wasm_smart(&config.factory_address, &FactoryQuery::Factory {})
        .map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "Failed to query factory config for denom validation: {}",
                e
            )))
        })?;
    if factory_resp.factory.bluechip_denom != config.bluechip_denom {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "bluechip_denom mismatch: factory has \"{}\", expand-economy has \"{}\". \
             Update one side via its config-update flow before retrying.",
            factory_resp.factory.bluechip_denom, config.bluechip_denom
        ))));
    }

    match msg {
        ExpandEconomyMsg::RequestExpansion { recipient, amount } => {
            if amount.is_zero() {
                // The factory's bluechip mint-decay polynomial drops to zero
                // once `pool_id` and `seconds_elapsed` grow past the curve's
                // crossover. Once it does, this contract is "dormant" by
                // design — there is no more bluechip-economy expansion to
                // dispense, and the mechanism's job is done. Surface that
                // explicitly so operators and monitoring can distinguish
                // "skipped because schedule has expired" from "skipped
                // because of a bug".
                return Ok(Response::new()
                    .add_attribute("action", "request_reward_skipped")
                    .add_attribute("reason", "economy_dormant")
                    .add_attribute(
                        "note",
                        "ExpandEconomy mint schedule has reached zero; \
                         no further expansions will be dispensed. This \
                         is the intended end-state of the decay curve.",
                    ));
            }

            // Validate the recipient at the contract boundary rather than
            // letting a malformed string surface as an opaque bank-module
            // error deep in the tx pipeline. Also guards against callers
            // accidentally forwarding an IBC-wrapped / wrong-prefix string.
            let recipient_addr = deps.api.addr_validate(&recipient)?;

            // Rolling 24-hour spend cap. Defense-in-depth against a
            // compromised factory key forwarding huge RequestExpansion
            // calls. The legitimate threshold-mint schedule is well below
            // DAILY_EXPANSION_CAP per day; an attacker with full factory
            // control can extract at most CAP per 24-hour window via this
            // path. Window resets opportunistically on the first call after
            // expiry rather than continuously, which is fine for cap
            // semantics — see ExpansionWindow doc.
            let now = env.block.time;
            let window = match EXPANSION_WINDOW.may_load(deps.storage)? {
                Some(w)
                    if now.seconds().saturating_sub(w.window_start.seconds())
                        < DAILY_WINDOW_SECONDS =>
                {
                    w
                }
                _ => ExpansionWindow {
                    window_start: now,
                    spent_in_window: Uint128::zero(),
                },
            };
            let new_spent = window.spent_in_window.checked_add(amount)?;
            if new_spent > DAILY_EXPANSION_CAP {
                return Err(ContractError::DailyExpansionCapExceeded {
                    requested: amount,
                    spent_in_window: window.spent_in_window,
                    cap: DAILY_EXPANSION_CAP,
                });
            }

            // Graceful no-op when the contract's balance is below the
            // requested amount. Running out of expand-economy funds is the
            // INTENDED end-state: the contract is a finite "bluechip mint
            // boost" reservoir that drains as the early ecosystem grows,
            // tapering rewards toward zero by design. A failed BankMsg
            // here would propagate up through `NotifyThresholdCrossed` and
            // revert the entire factory tx, which would in turn leave the
            // pool's `IS_THRESHOLD_HIT = true` state in place but force
            // operators to chase the failed mint via `RetryFactoryNotify`
            // forever. Instead, log the skip and return Ok so threshold
            // crossings continue to settle cleanly even when the reservoir
            // is empty.
            let balance = deps
                .querier
                .query_balance(env.contract.address.as_str(), &config.bluechip_denom)?;
            if balance.amount < amount {
                return Ok(Response::new()
                    .add_attribute("action", "request_reward_skipped")
                    .add_attribute("reason", "insufficient_balance")
                    .add_attribute("recipient", recipient_addr)
                    .add_attribute("requested_amount", amount)
                    .add_attribute("contract_balance", balance.amount)
                    .add_attribute("denom", config.bluechip_denom));
            }

            // Persist the rolling-window debit only after balance check
            // passes, so a skipped (insufficient_balance) request doesn't
            // burn cap budget that the protocol could spend later when
            // the contract is refunded.
            EXPANSION_WINDOW.save(
                deps.storage,
                &ExpansionWindow {
                    window_start: window.window_start,
                    spent_in_window: new_spent,
                },
            )?;

            let send_msg = BankMsg::Send {
                to_address: recipient_addr.to_string(),
                amount: vec![Coin {
                    denom: config.bluechip_denom.clone(),
                    amount,
                }],
            };

            Ok(Response::new()
                .add_message(send_msg)
                .add_attribute("action", "request_reward")
                .add_attribute("recipient", recipient_addr)
                .add_attribute("amount", amount)
                .add_attribute("denom", config.bluechip_denom)
                .add_attribute("spent_in_window_after", new_spent)
                .add_attribute("daily_cap", DAILY_EXPANSION_CAP))
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

    // Clamp the requested amount to the contract's current balance so a
    // proposed-but-stale withdrawal (e.g. balance drew down via
    // RequestExpansion between propose and execute) doesn't fail the
    // whole tx at the bank module. Transfer the smaller of (requested,
    // balance) and emit both values so the caller can detect the clamp.
    let balance = deps
        .querier
        .query_balance(env.contract.address.as_str(), &pending.denom)?;
    let amount_to_send = pending.amount.min(balance.amount);

    let mut response = Response::new()
        .add_attribute("action", "execute_withdrawal")
        .add_attribute("recipient", pending.recipient.clone())
        .add_attribute("requested_amount", pending.amount)
        .add_attribute("amount", amount_to_send)
        .add_attribute("contract_balance", balance.amount)
        .add_attribute("denom", pending.denom.clone());

    if !amount_to_send.is_zero() {
        let send_msg = BankMsg::Send {
            to_address: pending.recipient.clone(),
            amount: vec![Coin {
                denom: pending.denom,
                amount: amount_to_send,
            }],
        };
        response = response.add_message(send_msg);
    } else {
        response = response.add_attribute("note", "no funds available; withdrawal skipped");
    }

    Ok(response)
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
