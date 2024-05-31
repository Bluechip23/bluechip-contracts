use cosmwasm_std::{to_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError, StdResult, Uint128};
use cosmwasm_std::{Addr, Coin, CosmosMsg, BankMsg, BankQuery, QuerierWrapper, QueryRequest, AllBalanceResponse};
use cw2::set_contract_version;
use cw_storage_plus::{Map, Item};
use serde::{Deserialize, Serialize};

const CONTRACT_NAME: &str = "crates.io:airdrop";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InstantiateMsg {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    SetRewards { recipients: Vec<Recipient> },
    Claim {},
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Recipient {
    pub address: String,
    pub amount: Uint128,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub owner: Addr,
}

const STATE: Item<State> = Item::new("state");
const REWARDS: Map<&Addr, Uint128> = Map::new("rewards");
const CLAIMED: Map<&Addr, bool> = Map::new("claimed");

pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    _msg: InstantiateMsg,
) -> StdResult<Response> {
    let state = State { owner: info.sender.clone() };
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new().add_attribute("method", "instantiate"))
}

pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> StdResult<Response> {
    match msg {
        ExecuteMsg::SetRewards { recipients } => try_set_rewards(deps, env, info, recipients),
        ExecuteMsg::Claim {} => try_claim(deps, info),
    }
}

fn try_set_rewards(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recipients: Vec<Recipient>,
) -> StdResult<Response> {
    let state = STATE.load(deps.storage)?;
    if info.sender != state.owner {
        return Err(StdError::unauthorized());
    }

    // Calculate total rewards
    let total_rewards: Uint128 = recipients.iter().map(|r| r.amount).sum();

    // Ensure the contract has enough funds
    let balance = get_contract_balance(&deps.querier, env.contract.address)?;
    if balance < total_rewards {
        return Err(StdError::generic_err("Insufficient funds to cover all rewards"));
    }

    for recipient in recipients {
        let addr = deps.api.addr_validate(&recipient.address)?;
        REWARDS.save(deps.storage, &addr, &recipient.amount)?;
    }

    Ok(Response::new().add_attribute("method", "set_rewards").add_attribute("total_rewards", total_rewards.to_string()))
}

fn try_claim(deps: DepsMut, info: MessageInfo) -> StdResult<Response> {
    let addr = info.sender.clone();
    let claimed = CLAIMED.may_load(deps.storage, &addr)?;

    if claimed == Some(true) {
        return Err(StdError::generic_err("Tokens already claimed"));
    }

    let amount = match REWARDS.may_load(deps.storage, &addr)? {
        Some(amt) => amt,
        None => return Err(StdError::generic_err("No rewards found for address")),
    };

    let msg = CosmosMsg::Bank(BankMsg::Send {
        to_address: addr.to_string(),
        amount: vec![Coin {
            denom: "bluechip".to_string(),
            amount,
        }],
    });

    CLAIMED.save(deps.storage, &addr, &true)?;

    Ok(Response::new()
        .add_message(msg)
        .add_attribute("method", "claim")
        .add_attribute("amount", amount.to_string()))
}

// Helper function to get the contract's balance
fn get_contract_balance(querier: &QuerierWrapper, contract_addr: Addr) -> StdResult<Uint128> {
    let balance: AllBalanceResponse = querier.query(&QueryRequest::Bank(BankQuery::AllBalances {
        address: contract_addr.to_string(),
    }))?;
    // Assume the token to be used is "bluechip", otherwise adjust accordingly
    Ok(balance.amount.iter().find(|c| c.denom == "bluechip").map(|c| c.amount).unwrap_or_else(Uint128::zero))
}
