use crate::error::ContractError;
use crate::internal_pool_oracle::{BlueChipPriceInternalOracle, PriceCache, ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS, INTERNAL_ORACLE, ROTATION_INTERVAL, UPDATE_INTERVAL};
use crate::msg::{CreatorTokenInfo, ExecuteMsg, TokenInstantiateMsg};
use crate::pool_struct::CreatePool;
use crate::pool_create_cleanup::handle_cleanup_reply;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::state::{
    PoolCreationState, CreationStatus, FactoryInstantiate,
    POOL_CREATION_STATES, FACTORYINSTANTIATEINFO, NEXT_POOL_ID, TEMPCREATORWALLETADDR, TEMPPOOLID,
    TEMPPOOLINFO,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError, StdResult, SubMsg, Uint128, WasmMsg
};
use cw20::MinterResponse;
use std::env;

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
    env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    //saves the factory parameters set in the json file
    FACTORYINSTANTIATEINFO.save(deps.storage, &msg)?;

     let internal_bluechip_price_oracle = BlueChipPriceInternalOracle {
        selected_pools: vec![ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()],
        atom_pool_contract_address: Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS),
        last_rotation: env.block.time.seconds(),
        rotation_interval: ROTATION_INTERVAL,
        bluechip_price_cache: PriceCache {
            last_price: Uint128::zero(),
            last_update: 0,
            twap_observations: vec![],
        },
        update_interval: UPDATE_INTERVAL,
    };
    INTERNAL_ORACLE.save(deps.storage, &internal_bluechip_price_oracle)?;
    //sets the first pool created by this factory to 1
    //viola
    Ok(Response::new().add_attribute("action", "init_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
        ExecuteMsg::Create {
            pool_msg,
            token_info,
        } => {
        let token_a = pool_msg.pool_token_info[0].to_string();
        let token_b = pool_msg.pool_token_info[1].to_string();
        execute_create(
            deps,
            env,
            info,
            pool_msg,
            token_info,
            token_a,
            token_b,
        )
    }
    }
}

//make sure the correct factory sent the message to instantiate the pool or other execute messages.
//users can ensure pools made by a certain factory are safe.
fn assert_correct_factory_address(deps: Deps, info: MessageInfo) -> StdResult<bool> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    if info.sender != config.factory_admin_address {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.factory_admin_address, info.sender
        )));
    }

    Ok(true)
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    FACTORYINSTANTIATEINFO.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

//create pool - 3 step pool process through reply function (mostly found in reply.rs)
//partial creations will be cleaned up found in pool_create_cleanup
fn execute_create(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_msg: CreatePool,
    token_info: CreatorTokenInfo,
    token_a: String,
    token_b: String,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info.clone())?;
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let sender = info.sender.clone();
    let pool_id = NEXT_POOL_ID.load(deps.storage)?;
    TEMPPOOLID.save(deps.storage, &pool_id)?;
    TEMPPOOLINFO.save(deps.storage, &pool_msg)?;
    TEMPCREATORWALLETADDR.save(deps.storage, &sender)?;
    let msg = WasmMsg::Instantiate {
        code_id: config.cw20_token_contract_id,
        //creating the creator tokens - they are not minted yet. Simply created. Factory hands the minting responsibilities to pool.
        msg: to_json_binary(&TokenInstantiateMsg {
            token_name: token_info.token_name.clone(),
            ticker: token_info.ticker.clone(),
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
        label: token_info.token_name,
    };
    //set the tracking state for pool creation - all fields start as none and get populated throughout pool creation
    let creation_state = PoolCreationState {
        pool_id,
        creator: info.sender,
        creator_token_address: None,
        mint_new_position_nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
    //triggers reply function when things go well.
    let sub_msg = vec![SubMsg::reply_on_success(msg, SET_TOKENS)];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

#[entry_point]
//called by execute create.
//each step can either succeed (advancing to the next step) or fail (triggering cleanup) - found on reply.rs and pool_create_cleanup respectively
pub fn pool_creation_reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    match msg.id {
        SET_TOKENS => set_tokens(deps, env, msg),
        MINT_CREATE_POOL => mint_create_pool(deps, env, msg),
        FINALIZE_POOL => finalize_pool(deps, env, msg),
        CLEANUP_TOKEN_REPLY_ID => handle_cleanup_reply(deps, env, msg),
        CLEANUP_NFT_REPLY_ID => handle_cleanup_reply(deps, env, msg),
        _ => Err(ContractError::UnknownReplyId { id: msg.id }),
    }
}
