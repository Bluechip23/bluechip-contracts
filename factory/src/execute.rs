use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_force_rotate_pools, initialize_internal_bluechip_oracle, update_internal_oracle_price,
};
use crate::mint_bluechips_pool_creation::calculate_and_mint_bluechip;
use crate::msg::{CreatorTokenInfo, ExecuteMsg, TokenInstantiateMsg};
use crate::pool_create_cleanup::handle_cleanup_reply;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::pool_struct::{CreatePool, PoolConfigUpdate, TempPoolCreation};
use crate::state::{
    CreationStatus, FactoryInstantiate, PendingConfig, PoolCreationState, PoolUpgrade,
    FACTORYINSTANTIATEINFO, PENDING_CONFIG, PENDING_POOL_UPGRADE, POOL_COUNTER,
    POOL_CREATION_STATES, POOL_REGISTRY, POOL_THRESHOLD_MINTED, TEMP_POOL_CREATION,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::{
    entry_point, to_json_binary, Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError,
    StdResult, SubMsg, Uint128, WasmMsg,
};
use cosmwasm_std::{Binary, CosmosMsg, Order};
use cw20::MinterResponse;
use std::env;

const CONTRACT_NAME: &str = "crates.io:factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BURN_ADDRESS: &str = "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnrql8a";
/// Reply step constants (stored in low 8 bits of reply ID).
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
pub const CLEANUP_TOKEN_STEP: u64 = 100;
pub const CLEANUP_NFT_STEP: u64 = 101;

/// Encodes a pool_id and a step into a single SubMsg reply ID.
/// Layout: upper 56 bits = pool_id, lower 8 bits = step.
pub fn encode_reply_id(pool_id: u64, step: u64) -> u64 {
    (pool_id << 8) | (step & 0xFF)
}

/// Decodes a reply ID back into (pool_id, step).
pub fn decode_reply_id(reply_id: u64) -> (u64, u64) {
    (reply_id >> 8, reply_id & 0xFF)
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // M-1 FIX: Validate all address fields during instantiation
    deps.api.addr_validate(msg.factory_admin_address.as_str())?;
    deps.api.addr_validate(msg.bluechip_wallet_address.as_str())?;
    deps.api.addr_validate(msg.atom_bluechip_anchor_pool_address.as_str())?;
    if let Some(ref mint_addr) = msg.bluechip_mint_contract_address {
        deps.api.addr_validate(mint_addr.as_str())?;
    }

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
        ExecuteMsg::ProposeConfigUpdate {
            config: factory_instantiate,
        } => execute_propose_factory_config_update(deps, env, info, factory_instantiate),
        ExecuteMsg::UpdateConfig {} => execute_update_factory_config(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_factory_config_update(deps, info),
        ExecuteMsg::Create {
            pool_msg,
            token_info,
        } => execute_create_creator_pool(deps, env, info, pool_msg, token_info),
        ExecuteMsg::UpdateOraclePrice {} => update_internal_oracle_price(deps, env),
        ExecuteMsg::ForceRotateOraclePools {} => execute_force_rotate_pools(deps, env, info),
        ExecuteMsg::UpgradePools {
            new_code_id,
            pool_ids,
            migrate_msg,
        } => execute_propose_pool_upgrade(deps, env, info, new_code_id, pool_ids, migrate_msg),
        ExecuteMsg::ExecutePoolUpgrade {} => execute_apply_pool_upgrade(deps, env, info),
        ExecuteMsg::CancelPoolUpgrade {} => execute_cancel_pool_upgrade(deps, info),
        ExecuteMsg::ContinuePoolUpgrade {} => execute_continue_pool_upgrade(deps, env, info),
        ExecuteMsg::UpdatePoolConfig {
            pool_id,
            pool_config,
        } => execute_update_pool_config(deps, env, info, pool_id, pool_config),
        ExecuteMsg::NotifyThresholdCrossed { pool_id } => {
            execute_notify_threshold_crossed(deps, env, info, pool_id)
        }
    }
}

pub fn assert_correct_factory_address(deps: Deps, info: MessageInfo) -> StdResult<bool> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    if info.sender != config.factory_admin_address {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.factory_admin_address, info.sender
        )));
    }
    Ok(true)
}

pub fn execute_update_factory_config(deps: DepsMut, env: Env, info: MessageInfo) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let pending = PENDING_CONFIG.load(deps.storage)?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: pending.effective_after,
        });
    }
    FACTORYINSTANTIATEINFO.save(deps.storage, &pending.new_config)?;
    PENDING_CONFIG.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "execute_update_config"))
}

pub fn execute_propose_factory_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let pending = PendingConfig {
        new_config: config,
        effective_after: env.block.time.plus_seconds(86400 * 2), // 48 hour delay
    };
    PENDING_CONFIG.save(deps.storage, &pending)?;
    Ok(Response::new()
        .add_attribute("action", "propose_config_update")
        .add_attribute("effective_after", pending.effective_after.to_string()))
}

pub fn execute_cancel_factory_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;
    PENDING_CONFIG.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

fn execute_create_creator_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_msg: CreatePool,
    token_info: CreatorTokenInfo,
) -> Result<Response, ContractError> {
    let factory_cw20 = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let sender = info.sender.clone();
    let pool_counter = POOL_COUNTER.load(deps.storage).unwrap_or(0);
    let pool_id = pool_counter + 1;
    POOL_COUNTER.save(deps.storage, &pool_id)?;
    TEMP_POOL_CREATION.save(
        deps.storage,
        pool_id,
        &TempPoolCreation {
            temp_pool_info: pool_msg,
            temp_creator_wallet: info.sender.clone(),
            pool_id,
            creator_token_addr: None,
            nft_addr: None,
        },
    )?;
    let msg = WasmMsg::Instantiate {
        code_id: factory_cw20.cw20_token_contract_id,
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
        admin: Some(env.contract.address.to_string()),
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
    let sub_msg = vec![SubMsg::reply_on_success(msg, encode_reply_id(pool_id, SET_TOKENS))];

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
    let (pool_id, step) = decode_reply_id(msg.id);
    match step {
        SET_TOKENS => set_tokens(deps, env, msg, pool_id),
        MINT_CREATE_POOL => mint_create_pool(deps, env, msg, pool_id),
        FINALIZE_POOL => finalize_pool(deps, env, msg, pool_id),
        CLEANUP_TOKEN_STEP | CLEANUP_NFT_STEP => handle_cleanup_reply(deps, env, msg, pool_id),
        _ => Err(ContractError::UnknownReplyId { id: msg.id }),
    }
}

pub fn execute_propose_pool_upgrade(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    new_code_id: u64,
    pool_ids: Option<Vec<u64>>, // None = all pools
    migrate_msg: Binary,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    // Reject if there's already a pending upgrade
    if PENDING_POOL_UPGRADE.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "A pool upgrade is already pending. Cancel it first.",
        )));
    }

    let pools_to_upgrade = if let Some(ids) = pool_ids {
        ids
    } else {
        POOL_REGISTRY
            .keys(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?
    };

    let effective_after = env.block.time.plus_seconds(86400 * 2); // 48 hour delay

    PENDING_POOL_UPGRADE.save(
        deps.storage,
        &PoolUpgrade {
            new_code_id,
            migrate_msg,
            pools_to_upgrade: pools_to_upgrade.clone(),
            upgraded_count: 0,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_pool_upgrade")
        .add_attribute("new_code_id", new_code_id.to_string())
        .add_attribute("pool_count", pools_to_upgrade.len().to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_apply_pool_upgrade(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;

    // Enforce timelock
    if env.block.time < upgrade.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: upgrade.effective_after,
        });
    }

    // Must not have started yet
    if upgrade.upgraded_count > 0 {
        return Err(ContractError::Std(StdError::generic_err(
            "Upgrade already in progress. Use ContinuePoolUpgrade.",
        )));
    }

    let batch_size = 10;
    let mut messages = vec![];

    for pool_id in upgrade.pools_to_upgrade.iter().take(batch_size) {
        let pool_addr = POOL_REGISTRY.load(deps.storage, *pool_id)?;
        messages.push(CosmosMsg::Wasm(WasmMsg::Migrate {
            contract_addr: pool_addr.to_string(),
            new_code_id: upgrade.new_code_id,
            msg: upgrade.migrate_msg.clone(),
        }));
    }

    // Save progress
    let mut upgrade = upgrade;
    upgrade.upgraded_count = messages.len() as u32;
    PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;

    if upgrade.pools_to_upgrade.len() > batch_size {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&ExecuteMsg::ContinuePoolUpgrade {})?,
            funds: vec![],
        }));
    } else {
        // All pools fit in one batch, clean up
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "execute_pool_upgrade")
        .add_attribute("new_code_id", upgrade.new_code_id.to_string())
        .add_attribute("pool_count", upgrade.pools_to_upgrade.len().to_string()))
}

pub fn execute_cancel_pool_upgrade(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let upgrade = PENDING_POOL_UPGRADE.may_load(deps.storage)?;
    if upgrade.is_none() {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending pool upgrade to cancel",
        )));
    }

    PENDING_POOL_UPGRADE.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_pool_upgrade"))
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

    // Send update message to specific pool using the pool's expected message format
    #[derive(serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum PoolExecuteMsg {
        UpdateConfigFromFactory { update: PoolConfigUpdate },
    }
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&PoolExecuteMsg::UpdateConfigFromFactory {
            update: update_msg,
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
    if info.sender != env.contract.address {
        return Err(ContractError::Unauthorized {});
    }

    let mut upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;

    // Skip already upgraded pools (borrow the vector immutably to avoid moving `upgrade`)
    let remaining_pools: Vec<u64> = upgrade
        .pools_to_upgrade
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

    if remaining_pools.len() > batch_size {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&ExecuteMsg::ContinuePoolUpgrade {})?,
            funds: vec![],
        }));
    } else {
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    Ok(Response::new()
        .add_messages(messages.clone())
        .add_attribute("action", "continue_upgrade")
        .add_attribute("upgraded_in_batch", messages.len().to_string())
        .add_attribute("total_upgraded", upgrade.upgraded_count.to_string()))
}

/// Called by a pool when its commit threshold has been crossed.
/// Triggers the bluechip mint for this pool (only once per pool).
pub fn execute_notify_threshold_crossed(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    // Verify the caller is the registered pool contract for this pool_id
    let pool_addr = POOL_REGISTRY.load(deps.storage, pool_id).map_err(|_| {
        ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        )))
    })?;

    if info.sender != pool_addr {
        return Err(ContractError::Std(StdError::generic_err(
            "Only the registered pool contract can notify threshold crossed",
        )));
    }

    // Check if this pool has already triggered its mint
    if POOL_THRESHOLD_MINTED
        .may_load(deps.storage, pool_id)?
        .unwrap_or(false)
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Bluechip mint already triggered for this pool",
        )));
    }

    // Mark as minted before executing to prevent reentrancy
    POOL_THRESHOLD_MINTED.save(deps.storage, pool_id, &true)?;

    // Trigger the bluechip mint using pool_id as the count parameter
    let mint_messages = calculate_and_mint_bluechip(&mut deps, env, pool_id)?;

    Ok(Response::new()
        .add_messages(mint_messages)
        .add_attribute("action", "threshold_crossed_mint")
        .add_attribute("pool_id", pool_id.to_string()))
}
