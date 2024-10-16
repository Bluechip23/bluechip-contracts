use std::env;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    Addr, AllBalanceResponse, BankMsg, BankQuery, Coin, CosmosMsg, DepsMut, Env, MessageInfo,
    QuerierWrapper, QueryRequest, Response, StdResult, Uint128,
};
use cw2::set_contract_version;
// use cw_storage_plus::{Item, Map};
// use serde::{Deserialize, Serialize};

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg};
use crate::state::{State, CLAIMED, STATE, WHITELISTED};

const CONTRACT_NAME: &str = "crates.io:airdrop";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    let state = State {
        owner: info.sender.clone(),
        total_whitelist_wallets: msg.total_whitelist_wallets, // 100000 wallets - whiltelisted
        eligible_wallets: msg.eligible_wallets,               // 60000 wallets can get airdrop
        imported_wallets: Uint128::zero(),                    // no imported wallets now
        claimed_wallets: Uint128::zero(),                     // no claimed wallets now
        airdrop_amount: msg.airdrop_amount, // airdrop amount each wallets can get.
        is_opened: false,                   // airdrop is not started
    };
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new().add_attribute("method", "instantiate"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        // ExecuteMsg::SetRewards { recipients } => try_set_rewards(deps, env, info, recipients),
        ExecuteMsg::ImportWhitelist { whitelist } => {
            try_import_whitelist(deps, env, info, whitelist)
        }
        ExecuteMsg::Start {} => try_start_airdrop(deps, env, info),
        ExecuteMsg::Claim {} => try_claim(deps, info),
    }
}

fn try_import_whitelist(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    whitelist: Vec<Addr>,
) -> Result<Response, ContractError> {
    let mut state = STATE.load(deps.storage)?;
    if info.sender != state.owner {
        return Err(ContractError::Unauthorized {});
    }

    for user in whitelist {
        WHITELISTED.save(deps.storage, &user, &true)?;
        state.imported_wallets = state.imported_wallets + Uint128::new(1);
    }

    if state.imported_wallets > state.total_whitelist_wallets {
        return Err(ContractError::TooManyWhitelist {});
    }

    STATE.save(deps.storage, &state)?;

    Ok(Response::new().add_attribute("add_whitelist", "success"))
}

// only admin can start airdrop - but needs to send the tokens before airdrop
fn try_start_airdrop(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let addr = info.sender.clone();
    let mut state = STATE.load(deps.storage)?;

    // Calculate total rewards
    let total_rewards: Uint128 = state.airdrop_amount * state.eligible_wallets;

    // Ensure the contract has enough funds
    let balance = get_contract_balance(&deps.querier, env.contract.address)?;
    if balance < total_rewards {
        return Err(ContractError::InsufficientFunds {});
    }

    if addr != state.owner {
        return Err(ContractError::Unauthorized {});
    }

    state.is_opened = true;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new().add_attribute("status", "opened"))
}

// fn try_set_rewards(
//     deps: DepsMut,
//     env: Env,
//     info: MessageInfo,
//     recipients: Vec<Recipient>,
// ) -> Result<Response, ContractError> {
//     let state = STATE.load(deps.storage)?;
//     if info.sender != state.owner {
//         return Err(ContractError::Unauthorized {});
//     }

//     // Calculate total rewards
//     let total_rewards: Uint128 = recipients.iter().map(|r| r.amount).sum();

//     // Ensure the contract has enough funds
//     let balance = get_contract_balance(&deps.querier, env.contract.address)?;
//     if balance < total_rewards {
//         return Err(ContractError::InsufficientFunds {});
//     }

//     for recipient in recipients {
//         let addr = deps.api.addr_validate(&recipient.address)?;
//         REWARDS.save(deps.storage, &addr, &recipient.amount)?;
//     }

//     Ok(Response::new()
//         .add_attribute("method", "set_rewards")
//         .add_attribute("total_rewards", total_rewards.to_string()))
// }

fn try_claim(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let addr = info.sender.clone();

    let claimed = CLAIMED.may_load(deps.storage, &addr)?;
    if claimed == Some(true) {
        return Err(ContractError::AlreadyClaimed {});
    }

    let whitelisted = WHITELISTED.may_load(deps.storage, &addr)?;
    if whitelisted != Some(true) {
        return Err(ContractError::Unauthorized {}); // it isnot whitelisted wallet
    }

    let mut state = STATE.load(deps.storage)?;
    if state.is_opened != true {
        return Err(ContractError::NotStarted {}); // airdrop not started
    }

    let amount = state.airdrop_amount;

    let msg = CosmosMsg::Bank(BankMsg::Send {
        to_address: addr.to_string(),
        amount: vec![Coin {
            denom: "bluechip".to_string(),
            amount,
        }],
    });

    CLAIMED.save(deps.storage, &addr, &true)?;
    state.claimed_wallets = state.claimed_wallets + Uint128::new(1);
    if state.claimed_wallets > state.eligible_wallets {
        return Err(ContractError::AirdropFinished {}); // all wallets claimed and airdrop finished
    }

    STATE.save(deps.storage, &state)?;

    Ok(Response::new()
        .add_message(msg)
        .add_attribute("method", "claim")
        .add_attribute("amount", amount.to_string()))
}

// Helper function to get the contract's balance
fn get_contract_balance(querier: &QuerierWrapper, contract_addr: Addr) -> StdResult<Uint128> {
    let balance: AllBalanceResponse =
        querier.query(&QueryRequest::Bank(BankQuery::AllBalances {
            address: contract_addr.to_string(),
        }))?;
    // Assume the token to be used is "bluechip", otherwise adjust accordingly
    Ok(balance
        .amount
        .iter()
        .find(|c| c.denom == "bluechip")
        .map(|c| c.amount)
        .unwrap_or_else(Uint128::zero))
}
