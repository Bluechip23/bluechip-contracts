use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_force_rotate_pools, initialize_internal_bluechip_oracle, update_internal_oracle_price,
};
use crate::msg::{CreatorTokenInfo, ExecuteMsg, TokenInstantiateMsg};
use crate::pool_create_cleanup::handle_cleanup_reply;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::pool_struct::{CreatePool, PoolConfigUpdate, TempPoolCreation};
use crate::state::{
    CreationStatus, FactoryInstantiate, PoolCreationState, PoolUpgrade, FACTORYINSTANTIATEINFO, PENDING_POOL_UPGRADE, POOL_COUNTER, POOL_CREATION_STATES, POOL_REGISTRY, TEMP_POOL_CREATION
};
use cosmwasm_std::{Binary, CosmosMsg, Order};
#[cfg(not(feature = "library"))]
use cosmwasm_std::{
    entry_point, to_json_binary, Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError,
    StdResult, SubMsg, Uint128, WasmMsg,
};
use cw20::MinterResponse;
use std::env;

const CONTRACT_NAME: &str = "crates.io:factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BURN_ADDRESS: &str = "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnrql8a";
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
pub const CLEANUP_TOKEN_REPLY_ID: u64 = 100;
pub const CLEANUP_NFT_REPLY_ID: u64 = 101;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    FACTORYINSTANTIATEINFO.save(deps.storage, &msg)?;
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
        } => execute_create_creator_pool(deps, env, info, pool_msg, token_info),
        ExecuteMsg::UpdateOraclePrice {} => update_internal_oracle_price(deps, env),
        ExecuteMsg::ForceRotateOraclePools {} => execute_force_rotate_pools(deps, env, info),
         ExecuteMsg::UpgradePools { new_code_id, pool_ids, migrate_msg } => 
            execute_upgrade_pools(deps, env, info, new_code_id, pool_ids, migrate_msg),
        ExecuteMsg::ContinuePoolUpgrade {} => 
            execute_continue_pool_upgrade(deps, env, info),
        ExecuteMsg::UpdatePoolConfig { pool_id, pool_config } => 
            execute_update_pool_config(deps, env, info, pool_id, pool_config),
    }
}

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

fn execute_create_creator_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_msg: CreatePool,
    token_info: CreatorTokenInfo,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info.clone())?;
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let sender = info.sender.clone();
    let pool_counter = POOL_COUNTER.load(deps.storage).unwrap_or(0);
    let pool_id = pool_counter + 1;
    POOL_COUNTER.save(deps.storage, &pool_id)?;
    TEMP_POOL_CREATION.save(
        deps.storage,
        &TempPoolCreation {
            temp_pool_info: pool_msg,
            temp_creator_wallet: info.sender.clone(),
            pool_id,
            creator_token_addr: None,
            nft_addr: None,
        },
    )?;
    let msg = WasmMsg::Instantiate {
        code_id: config.cw20_token_contract_id,
        //creating the creator token only, no minting.
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
        admin: Some(info.sender.to_string()),
        label: token_info.name,
    };
    //set the trackingfor pool creation
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
    let sub_msg = vec![SubMsg::reply_on_success(msg, SET_TOKENS)];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

#[entry_point]
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    pool_creation_reply(deps, env, msg)
}

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

// In factory/execute.rs
pub fn execute_upgrade_pools(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    new_code_id: u64,
    pool_ids: Option<Vec<u64>>, // None = all pools
    migrate_msg: Binary,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;
    
    // Get pools to upgrade
    let pools_to_upgrade = if let Some(ids) = pool_ids {
        ids
    } else {
        // Get all pool IDs
        POOL_REGISTRY
            .keys(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?
    };
    
    // Store upgrade info
    PENDING_POOL_UPGRADE.save(deps.storage, &PoolUpgrade {
        new_code_id,
        migrate_msg: migrate_msg.clone(),
        pools_to_upgrade: pools_to_upgrade.clone(),
        upgraded_count: 0,
    })?;
    
    // Start upgrading (batch to avoid gas issues)
    let batch_size = 10; // Upgrade 10 pools per tx
    let mut messages = vec![];
    
    for pool_id in pools_to_upgrade.iter().take(batch_size) {
        let pool_addr = POOL_REGISTRY.load(deps.storage, *pool_id)?;
        messages.push(CosmosMsg::Wasm(WasmMsg::Migrate {
            contract_addr: pool_addr.to_string(),
            new_code_id,
            msg: migrate_msg.clone(),
        }));
    }
    
    // If more pools remain, trigger continuation
    if pools_to_upgrade.len() > batch_size {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&ExecuteMsg::ContinuePoolUpgrade {})?,
            funds: vec![],
        }));
    }
    
    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "upgrade_pools")
        .add_attribute("new_code_id", new_code_id.to_string())
        .add_attribute("pool_count", pools_to_upgrade.len().to_string()))
}

pub fn execute_update_pool_config(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    pool_id: u64,
    update_msg: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;
    
    let pool_addr = POOL_REGISTRY.load(deps.storage, pool_id)?;
    
    // Send update message to specific pool
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&ExecuteMsg::UpdatePoolConfig {
            pool_id: pool_id,
            pool_config: update_msg,
        })?,
        funds: vec![],
    });
    
    Ok(Response::new()
        .add_message(msg)
        .add_attribute("action", "update_pool_config")
        .add_attribute("pool_id", pool_id.to_string()))
}

pub fn execute_continue_pool_upgrade(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Only the contract itself can call this
    if info.sender != env.contract.address {
        return Err(ContractError::Unauthorized {});
    }
    
    let mut upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;
    
    // Skip already upgraded pools (borrow the vector immutably to avoid moving `upgrade`)
    let remaining_pools: Vec<u64> = upgrade.pools_to_upgrade
        .iter()
        .cloned()
        .skip(upgrade.upgraded_count as usize)
        .collect();
    
    if remaining_pools.is_empty() {
        // All done
        PENDING_POOL_UPGRADE.remove(deps.storage);
        return Ok(Response::new()
            .add_attribute("action", "upgrade_complete")
            .add_attribute("total_upgraded", upgrade.upgraded_count.to_string()));
    }
    
    let batch_size = 10;
    let mut messages = vec![];
    
    for pool_id in remaining_pools.iter().take(batch_size) {
        let pool_addr = POOL_REGISTRY.load(deps.storage, *pool_id)?;
        messages.push(CosmosMsg::Wasm(WasmMsg::Migrate {
            contract_addr: pool_addr.to_string(),
            new_code_id: upgrade.new_code_id,
            msg: upgrade.migrate_msg.clone(),
        }));
        upgrade.upgraded_count += 1;
    }
    
    // Save progress
    PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;
    
    // Continue if more remain
    if remaining_pools.len() > batch_size {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&ExecuteMsg::ContinuePoolUpgrade {})?,
            funds: vec![],
        }));
    } else {
        // This was the last batch
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }
    
    Ok(Response::new()
        .add_messages(messages.clone())
        .add_attribute("action", "continue_upgrade")
        .add_attribute("upgraded_in_batch", messages.len().to_string())
        .add_attribute("total_upgraded", upgrade.upgraded_count.to_string()))
}
