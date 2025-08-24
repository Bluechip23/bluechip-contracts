use std::env;
use crate::error::ContractError;
use crate::msg::{ ExecuteMsg, MigrateMsg, TokenInfo, TokenInstantiateMsg};
use crate::pair::{CreatePool,};
use crate::state::{
    CreationState, CreationStatus, FactoryInstantiate, CONFIG, CREATION_STATES,
    NEXT_POOL_ID, TEMPCREATOR, TEMPPAIRINFO, TEMPPOOLID,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Deps, DepsMut, Env, MessageInfo,Response,
    StdError, StdResult, SubMsg, Uint128, WasmMsg,
};
use cw20::{MinterResponse};

const CONTRACT_NAME: &str = "bluechip_factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BURN_ADDRESS: &str = "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnrql8a";
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
pub const CLEANUP_TOKEN_REPLY_ID: u64 = 100;
pub const CLEANUP_NFT_REPLY_ID: u64 = 101;

#[cfg_attr(not(feature = "library"), entry_point)]
//instantiates the factory for pools to use.
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    //saves the factory parameters set in the json file
    CONFIG.save(deps.storage, &msg)?;
    //sets the first pool created by this factory to 1
    NEXT_POOL_ID.save(deps.storage, &1u64)?;
    //viola
    Ok(Response::new().add_attribute("action", "init_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    let version = cw2::get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }
    if version.version != CONTRACT_VERSION {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }

    Ok(Response::default().add_attribute("action", "migrate_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        //edit factory parameters - only bluechip can - does not touch existing pools unless we do a chain wide change
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
        //creates new pool
        ExecuteMsg::Create {
            pool_msg,
            token_info,
        } => execute_create(deps, env, info, pool_msg, token_info),
    }
}

//make sure the factory sent the message to instantiate the pool or other execute messages.
fn assert_is_admin(deps: Deps, info: MessageInfo) -> StdResult<bool> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.admin {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.admin, info.sender
        )));
    }

    Ok(true)
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info)?;

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

//create pool
fn execute_create(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pair_msg: CreatePool,
    token_info: TokenInfo,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info.clone())?;
    let config = CONFIG.load(deps.storage)?;
    let sender = info.sender.clone();
    let pool_id = NEXT_POOL_ID.load(deps.storage)?;
    NEXT_POOL_ID.save(deps.storage, &(pool_id + 1))?;
    TEMPPOOLID.save(deps.storage, &pool_id)?;
    TEMPPAIRINFO.save(deps.storage, &pair_msg)?;
    TEMPCREATOR.save(deps.storage, &sender)?;
    let msg = WasmMsg::Instantiate {
        code_id: config.token_id,
        //creating the creator tokens - they are not minted yet. Simply created. Factory hands the minting responsibilities to pool.
        msg: to_json_binary(&TokenInstantiateMsg {
            name: token_info.name.clone(),
            symbol: token_info.symbol.clone(),
            decimals: 6,
            initial_balances: vec![],
            mint: Some(MinterResponse {
                minter: env.contract.address.to_string(),
                //amount minted after threshold.
                cap: Some(Uint128::new(1_500_000_000_000)),
            }),
        })?,
        //no initial balance. waits until threshold is crossed to mint creator tokens.
        funds: vec![],
        //the factory is the admin to the pool so it can call upgrades to the pools as the chain advances.
        admin: Some(info.sender.to_string()),
        label: token_info.name,
    };
    //set the tracking state for pool creation
    let creation_state = CreationState {
        pool_id,
        creator: info.sender,
        token_address: None,
        nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
    //triggers reply function in reply.rs when things go well.
    let sub_msg = vec![SubMsg::reply_on_success(msg, SET_TOKENS)];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

