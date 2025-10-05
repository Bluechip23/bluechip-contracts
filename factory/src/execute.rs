use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_force_rotate_pools, initialize_internal_bluechip_oracle, update_internal_oracle_price,
};
use crate::msg::{CreatorTokenInfo, ExecuteMsg, TokenInstantiateMsg};
use crate::pool_create_cleanup::handle_cleanup_reply;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::pool_struct::CreatePool;
use crate::state::{
    CreationStatus, FactoryInstantiate, PoolCreationState, FACTORYINSTANTIATEINFO,
    POOL_CREATION_STATES, TEMPCREATORWALLETADDR, TEMPPOOLID, TEMPPOOLINFO, USED_POOL_IDS,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError, StdResult, SubMsg, Uint128, WasmMsg
};
use cw20::MinterResponse;
use cw_storage_plus::Endian;
use sha2::{Sha256, Digest};
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
    // Save the factory parameters
    FACTORYINSTANTIATEINFO.save(deps.storage, &msg)?;
    // Initialize the oracle properly using your dedicated function
    initialize_internal_bluechip_oracle(deps, env)?;
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
            execute_create_creator_pool(deps, env, info, pool_msg, token_info, token_a, token_b)
        }
        ExecuteMsg::UpdateOraclePrice {} => update_internal_oracle_price(deps, env),
        ExecuteMsg::ForceRotateOraclePools {} => execute_force_rotate_pools(deps, env, info),
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
fn execute_create_creator_pool(
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
    let pool_id = generate_unique_pool_id(&deps.as_ref(), &env, &info.sender)?;
    USED_POOL_IDS.save(deps.storage, pool_id, &true)?;
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

pub fn generate_unique_pool_id(
    deps: &Deps,
    env: &Env,
    creator: &Addr,
) -> StdResult<u64> {
    let mut attempt = 0;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(env.block.time.nanos().to_be_bytes());
        hasher.update(env.block.height.to_be_bytes());
        hasher.update(env.block.chain_id.as_bytes());
        hasher.update(creator.as_bytes());
        hasher.update(attempt.to_be_bytes()); // Add attempt counter for uniqueness
        
        let hash = hasher.finalize();
        let pool_id = u64::from_be_bytes([
            hash[0], hash[1], hash[2], hash[3],
            hash[4], hash[5], hash[6], hash[7],
        ]);
        
        // Check if ID already exists
        if !USED_POOL_IDS.has(deps.storage, pool_id) {
            return Ok(pool_id);
        }
        
        attempt += 1;
        if attempt > 100 {
            return Err(StdError::generic_err("Failed to generate unique pool ID after 100 attempts"));
        }
    }
}
