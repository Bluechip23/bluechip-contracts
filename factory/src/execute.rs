use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_cancel_force_rotate_pools, execute_force_rotate_pools,
    execute_propose_force_rotate_pools, initialize_internal_bluechip_oracle,
    update_internal_oracle_price,
};
use crate::mint_bluechips_pool_creation::calculate_and_mint_bluechip;
use crate::msg::{CreatorTokenInfo, ExecuteMsg, TokenInstantiateMsg};
use crate::pool_create_cleanup::handle_cleanup_reply;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::pool_struct::{CreatePool, PoolConfigUpdate, TempPoolCreation};
use crate::state::{
    CreationStatus, FactoryInstantiate, PendingConfig, PendingPoolConfig, PoolCreationState,
    PoolUpgrade, DISTRIBUTION_BOUNTY_USD, FACTORYINSTANTIATEINFO, MAX_DISTRIBUTION_BOUNTY_USD,
    MAX_ORACLE_UPDATE_BOUNTY_USD, ORACLE_BOUNTY_DENOM, ORACLE_UPDATE_BOUNTY_USD, PENDING_CONFIG,
    PENDING_POOL_CONFIG, PENDING_POOL_UPGRADE, POOL_COUNTER, POOL_CREATION_STATES,
    POOLS_BY_CONTRACT_ADDRESS, POOL_REGISTRY, POOL_THRESHOLD_MINTED, TEMP_POOL_CREATION,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError, StdResult, SubMsg,
    Uint128, WasmMsg,
};
use cosmwasm_std::{Addr, Attribute, Binary, CosmosMsg, Order};
use cw20::MinterResponse;
const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BURN_ADDRESS: &str = "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnrql8a";
// Reply step constants (stored in low 8 bits of reply ID).
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
pub const CLEANUP_TOKEN_STEP: u64 = 100;
pub const CLEANUP_NFT_STEP: u64 = 101;

// Encodes a pool_id and a step into a single SubMsg reply ID.
pub fn encode_reply_id(pool_id: u64, step: u64) -> u64 {
    (pool_id << 8) | (step & 0xFF)
}

// Decodes a reply ID back into (pool_id, step).
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

    deps.api.addr_validate(msg.factory_admin_address.as_str())?;
    deps.api
        .addr_validate(msg.bluechip_wallet_address.as_str())?;
    deps.api
        .addr_validate(msg.atom_bluechip_anchor_pool_address.as_str())?;
    if let Some(ref mint_addr) = msg.bluechip_mint_contract_address {
        deps.api.addr_validate(mint_addr.as_str())?;
    }

    FACTORYINSTANTIATEINFO.save(deps.storage, &msg)?;
    // Both keeper bounties default to zero. Admin enables them via
    // SetOracleUpdateBounty / SetDistributionBounty (each takes a USD
    // value in 6 decimals) once the factory has been pre-funded with
    // ubluechip from the bluechip main wallet.
    ORACLE_UPDATE_BOUNTY_USD.save(deps.storage, &Uint128::zero())?;
    DISTRIBUTION_BOUNTY_USD.save(deps.storage, &Uint128::zero())?;
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
        ExecuteMsg::UpdateOraclePrice {} => update_internal_oracle_price(deps, env, info),
        ExecuteMsg::SetOracleUpdateBounty { new_bounty } => {
            execute_set_oracle_update_bounty(deps, info, new_bounty)
        }
        ExecuteMsg::SetDistributionBounty { new_bounty } => {
            execute_set_distribution_bounty(deps, info, new_bounty)
        }
        ExecuteMsg::PayDistributionBounty { recipient } => {
            execute_pay_distribution_bounty(deps, env, info, recipient)
        }
        ExecuteMsg::ProposeForceRotateOraclePools {} => {
            execute_propose_force_rotate_pools(deps, env, info)
        }
        ExecuteMsg::CancelForceRotateOraclePools {} => {
            execute_cancel_force_rotate_pools(deps, info)
        }
        ExecuteMsg::ForceRotateOraclePools {} => execute_force_rotate_pools(deps, env, info),
        ExecuteMsg::UpgradePools {
            new_code_id,
            pool_ids,
            migrate_msg,
        } => execute_propose_pool_upgrade(deps, env, info, new_code_id, pool_ids, migrate_msg),
        ExecuteMsg::ExecutePoolUpgrade {} => execute_apply_pool_upgrade(deps, env, info),
        ExecuteMsg::CancelPoolUpgrade {} => execute_cancel_pool_upgrade(deps, info),
        ExecuteMsg::ContinuePoolUpgrade {} => execute_continue_pool_upgrade(deps, env, info),
        ExecuteMsg::ProposePoolConfigUpdate {
            pool_id,
            pool_config,
        } => execute_propose_pool_config_update(deps, env, info, pool_id, pool_config),
        ExecuteMsg::ExecutePoolConfigUpdate { pool_id } => {
            execute_apply_pool_config_update(deps, env, info, pool_id)
        }
        ExecuteMsg::CancelPoolConfigUpdate { pool_id } => {
            execute_cancel_pool_config_update(deps, info, pool_id)
        }
        ExecuteMsg::NotifyThresholdCrossed { pool_id } => {
            execute_notify_threshold_crossed(deps, env, info, pool_id)
        }
        ExecuteMsg::PausePool { pool_id } => execute_pause_pool(deps, info, pool_id),
        ExecuteMsg::UnpausePool { pool_id } => execute_unpause_pool(deps, info, pool_id),
        ExecuteMsg::EmergencyWithdrawPool { pool_id } => {
            execute_emergency_withdraw_pool(deps, info, pool_id)
        }
        ExecuteMsg::CancelEmergencyWithdrawPool { pool_id } => {
            execute_cancel_emergency_withdraw_pool(deps, info, pool_id)
        }
        ExecuteMsg::RecoverPoolStuckStates {
            pool_id,
            recovery_type,
        } => execute_recover_pool_stuck_states(deps, info, pool_id, recovery_type),
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

pub fn execute_update_factory_config(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
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
    deps.api
        .addr_validate(config.factory_admin_address.as_str())?;
    deps.api
        .addr_validate(config.bluechip_wallet_address.as_str())?;
    deps.api
        .addr_validate(config.atom_bluechip_anchor_pool_address.as_str())?;
    if let Some(ref mint_addr) = config.bluechip_mint_contract_address {
        deps.api.addr_validate(mint_addr.as_str())?;
    }

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

// Validates creator token metadata before any state is written.
// - decimals must be 6 (threshold payout and mint cap are calibrated for 6-decimal tokens)
// - name: 3-50 chars, printable ASCII only (no control chars, no extended unicode)
// - symbol: 3-12 chars, uppercase ASCII letters and digits only (matches cw20-base spec)
pub(crate) fn validate_creator_token_info(
    token_info: &CreatorTokenInfo,
) -> Result<(), ContractError> {
    if token_info.decimal != 6 {
        return Err(ContractError::Std(StdError::generic_err(
            "Token decimals must be 6. Threshold payout amounts and mint caps are calibrated for 6-decimal tokens.",
        )));
    }

    let name_len = token_info.name.chars().count();
    if !(3..=50).contains(&name_len) {
        return Err(ContractError::Std(StdError::generic_err(
            "Token name must be between 3 and 50 characters",
        )));
    }
    if !token_info
        .name
        .chars()
        .all(|c| c.is_ascii() && !c.is_ascii_control())
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Token name must contain only printable ASCII characters",
        )));
    }

    let symbol_len = token_info.symbol.chars().count();
    if !(3..=12).contains(&symbol_len) {
        return Err(ContractError::Std(StdError::generic_err(
            "Token symbol must be between 3 and 12 characters",
        )));
    }
    if !token_info
        .symbol
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Token symbol must contain only uppercase ASCII letters (A-Z) and digits (0-9)",
        )));
    }

    Ok(())
}

fn execute_create_creator_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_msg: CreatePool,
    token_info: CreatorTokenInfo,
) -> Result<Response, ContractError> {
    // Validate token metadata up front, before any state writes.
    validate_creator_token_info(&token_info)?;

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
            decimals: token_info.decimal,
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
    let sub_msg = vec![SubMsg::reply_on_success(
        msg,
        encode_reply_id(pool_id, SET_TOKENS),
    )];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

#[cfg_attr(not(feature = "library"), entry_point)]
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
    // None = all pools
    pool_ids: Option<Vec<u64>>,
    migrate_msg: Binary,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

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

// Processes a single batch of pools from the pending upgrade. Runs paused
// pools through a skip path (admin must unpause and re-run) so the upgrade
// doesn't migrate a pool that is mid-emergency-withdraw or otherwise in a
// sensitive state. Returns the built messages and records how many pools
// were processed (skipped + migrated) so the next batch can resume.
fn build_upgrade_batch(
    deps: Deps,
    pool_ids: &[u64],
    new_code_id: u64,
    migrate_msg: &Binary,
) -> Result<(Vec<CosmosMsg>, Vec<u64>, u32), ContractError> {
    let mut messages: Vec<CosmosMsg> = Vec::new();
    let mut skipped: Vec<u64> = Vec::new();
    let processed: u32 = pool_ids.len() as u32;

    for pool_id in pool_ids.iter() {
        let pool_addr = POOL_REGISTRY.load(deps.storage, *pool_id)?;

        // Query pause state; if the pool is paused, skip it. A paused pool
        // may be in the middle of a 24h emergency-withdraw timelock or other
        // sensitive state, and migrating it would likely break the invariants
        // the pool code relied on when pausing.
        let is_paused: pool_factory_interfaces::IsPausedResponse = deps
            .querier
            .query_wasm_smart(
                pool_addr.to_string(),
                &pool_factory_interfaces::PoolQueryMsg::IsPaused {},
            )
            .unwrap_or(pool_factory_interfaces::IsPausedResponse { paused: false });

        if is_paused.paused {
            skipped.push(*pool_id);
            continue;
        }

        messages.push(CosmosMsg::Wasm(WasmMsg::Migrate {
            contract_addr: pool_addr.to_string(),
            new_code_id,
            msg: migrate_msg.clone(),
        }));
    }

    Ok((messages, skipped, processed))
}

pub fn execute_apply_pool_upgrade(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;

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
    let first_batch: Vec<u64> = upgrade
        .pools_to_upgrade
        .iter()
        .take(batch_size)
        .cloned()
        .collect();

    let (messages, skipped, processed) = build_upgrade_batch(
        deps.as_ref(),
        &first_batch,
        upgrade.new_code_id,
        &upgrade.migrate_msg,
    )?;

    let mut upgrade = upgrade;
    upgrade.upgraded_count = processed;

    // If all pools were handled in this single batch, remove the pending
    // state. Otherwise persist progress and require the admin to call
    // ContinuePoolUpgrade explicitly — we no longer self-dispatch, which
    // previously risked blowing through the block gas limit for large pool
    // counts by chaining recursive execute messages.
    let total = upgrade.pools_to_upgrade.len() as u32;
    let more_batches = upgrade.upgraded_count < total;
    if more_batches {
        PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;
    } else {
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    let skipped_str = skipped
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "execute_pool_upgrade")
        .add_attribute("new_code_id", upgrade.new_code_id.to_string())
        .add_attribute("pool_count", total.to_string())
        .add_attribute("processed_in_batch", processed.to_string())
        .add_attribute("skipped_paused", skipped_str)
        .add_attribute("more_batches", more_batches.to_string()))
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

pub fn execute_propose_pool_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
    update_msg: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    // Verify pool exists
    let _pool_addr = POOL_REGISTRY.load(deps.storage, pool_id)?;

    if PENDING_POOL_CONFIG
        .may_load(deps.storage, pool_id)?
        .is_some()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "A pool config update is already pending for this pool. Cancel it first.",
        )));
    }

    let effective_after = env.block.time.plus_seconds(86400 * 2); // 48 hours

    PENDING_POOL_CONFIG.save(
        deps.storage,
        pool_id,
        &PendingPoolConfig {
            pool_id,
            update: update_msg,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_apply_pool_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    let pending = PENDING_POOL_CONFIG
        .load(deps.storage, pool_id)
        .map_err(|_| {
            ContractError::Std(StdError::generic_err(
                "No pending pool config update for this pool",
            ))
        })?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: pending.effective_after,
        });
    }

    let pool_addr = POOL_REGISTRY.load(deps.storage, pool_id)?;

    #[derive(serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum PoolExecuteMsg {
        UpdateConfigFromFactory { update: PoolConfigUpdate },
    }
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&PoolExecuteMsg::UpdateConfigFromFactory {
            update: pending.update,
        })?,
        funds: vec![],
    });

    PENDING_POOL_CONFIG.remove(deps.storage, pool_id);

    Ok(Response::new()
        .add_message(msg)
        .add_attribute("action", "execute_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string()))
}

pub fn execute_cancel_pool_config_update(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    if PENDING_POOL_CONFIG
        .may_load(deps.storage, pool_id)?
        .is_none()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending pool config update to cancel",
        )));
    }

    PENDING_POOL_CONFIG.remove(deps.storage, pool_id);

    Ok(Response::new()
        .add_attribute("action", "cancel_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string()))
}

// ---------------------------------------------------------------------------
// Pool admin forwards (pause / unpause / emergency withdraw / recovery)
// ---------------------------------------------------------------------------
// The pool gates these on `info.sender == pool_info.factory_addr`, so the
// factory contract is the only entity that can issue them. These helpers
// check the factory admin, look up the pool address, and build a WasmMsg
// forwarding the corresponding pool ExecuteMsg.

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PoolAdminMsg {
    Pause {},
    Unpause {},
    EmergencyWithdraw {},
    CancelEmergencyWithdraw {},
    RecoverStuckStates { recovery_type: crate::pool_struct::RecoveryType },
}

fn forward_pool_admin(
    deps: Deps,
    info: MessageInfo,
    pool_id: u64,
    action: &'static str,
    pool_msg: PoolAdminMsg,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps, info)?;
    let pool_addr = POOL_REGISTRY.load(deps.storage, pool_id).map_err(|_| {
        ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        )))
    })?;
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&pool_msg)?,
        funds: vec![],
    });
    Ok(Response::new()
        .add_message(msg)
        .add_attribute("action", action)
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("pool_addr", pool_addr.to_string()))
}

pub fn execute_pause_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    forward_pool_admin(deps.as_ref(), info, pool_id, "pause_pool", PoolAdminMsg::Pause {})
}

pub fn execute_unpause_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    forward_pool_admin(deps.as_ref(), info, pool_id, "unpause_pool", PoolAdminMsg::Unpause {})
}

pub fn execute_emergency_withdraw_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    forward_pool_admin(
        deps.as_ref(),
        info,
        pool_id,
        "emergency_withdraw_pool",
        PoolAdminMsg::EmergencyWithdraw {},
    )
}

pub fn execute_cancel_emergency_withdraw_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    forward_pool_admin(
        deps.as_ref(),
        info,
        pool_id,
        "cancel_emergency_withdraw_pool",
        PoolAdminMsg::CancelEmergencyWithdraw {},
    )
}

pub fn execute_recover_pool_stuck_states(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
    recovery_type: crate::pool_struct::RecoveryType,
) -> Result<Response, ContractError> {
    forward_pool_admin(
        deps.as_ref(),
        info,
        pool_id,
        "recover_pool_stuck_states",
        PoolAdminMsg::RecoverStuckStates { recovery_type },
    )
}

pub fn execute_continue_pool_upgrade(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Admin-only now. Previously this was self-called from
    // execute_apply_pool_upgrade, which worked only until the pool list grew
    // large enough that the chained execute messages exceeded block gas
    // limits in a single tx. Making this admin-only forces batches to be
    // submitted as separate transactions, each with its own gas budget.
    assert_correct_factory_address(deps.as_ref(), info)?;

    let mut upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;

    let remaining_pools: Vec<u64> = upgrade
        .pools_to_upgrade
        .iter()
        .cloned()
        .skip(upgrade.upgraded_count as usize)
        .collect();

    if remaining_pools.is_empty() {
        PENDING_POOL_UPGRADE.remove(deps.storage);
        return Ok(Response::new()
            .add_attribute("action", "upgrade_complete")
            .add_attribute("total_upgraded", upgrade.upgraded_count.to_string()));
    }

    let batch_size = 10;
    let batch: Vec<u64> = remaining_pools.iter().take(batch_size).cloned().collect();

    let (messages, skipped, processed) = build_upgrade_batch(
        deps.as_ref(),
        &batch,
        upgrade.new_code_id,
        &upgrade.migrate_msg,
    )?;

    upgrade.upgraded_count += processed;

    let total = upgrade.pools_to_upgrade.len() as u32;
    let more_batches = upgrade.upgraded_count < total;
    if more_batches {
        PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;
    } else {
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    let skipped_str = skipped
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "continue_upgrade")
        .add_attribute("processed_in_batch", processed.to_string())
        .add_attribute("total_processed", upgrade.upgraded_count.to_string())
        .add_attribute("skipped_paused", skipped_str)
        .add_attribute("more_batches", more_batches.to_string()))
}

// Called by a pool when its commit threshold has been crossed.
// Triggers the bluechip mint for this pool (only once per pool).
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

    POOL_THRESHOLD_MINTED.save(deps.storage, pool_id, &true)?;

    let mint_messages = calculate_and_mint_bluechip(&mut deps, env, pool_id)?;

    Ok(Response::new()
        .add_messages(mint_messages)
        .add_attribute("action", "threshold_crossed_mint")
        .add_attribute("pool_id", pool_id.to_string()))
}

// Builds a uniform "bounty skipped" Response for execute_pay_distribution_bounty.
// Every skip path emits the same action+bounty_skipped+pool triple plus
// a few path-specific extras; this keeps the call sites short and the
// emitted attribute shape consistent.
fn pay_distribution_bounty_skip(
    reason: &'static str,
    pool: &Addr,
    extras: Vec<Attribute>,
) -> Response {
    let mut resp = Response::new()
        .add_attribute("action", "pay_distribution_bounty")
        .add_attribute("bounty_skipped", reason)
        .add_attribute("pool", pool.to_string());
    for attr in extras {
        resp = resp.add_attribute(attr.key, attr.value);
    }
    resp
}

// Admin-only. Sets the per-call USD bounty (6 decimals, e.g. 5_000 = $0.005)
// paid to oracle keepers. Capped by MAX_ORACLE_UPDATE_BOUNTY_USD ($1).
// At payout time the value is converted to bluechip via the internal oracle.
pub fn execute_set_oracle_update_bounty(
    deps: DepsMut,
    info: MessageInfo,
    new_bounty: Uint128,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    if new_bounty > MAX_ORACLE_UPDATE_BOUNTY_USD {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Bounty exceeds max of {} (USD, 6 decimals)",
            MAX_ORACLE_UPDATE_BOUNTY_USD
        ))));
    }

    ORACLE_UPDATE_BOUNTY_USD.save(deps.storage, &new_bounty)?;

    Ok(Response::new()
        .add_attribute("action", "set_oracle_update_bounty")
        .add_attribute("new_bounty_usd", new_bounty.to_string()))
}

// Admin-only. Sets the per-batch USD bounty (6 decimals, e.g. 50_000 = $0.05)
// paid to keepers calling pool.ContinueDistribution. Capped by
// MAX_DISTRIBUTION_BOUNTY_USD ($1). Converted to bluechip at payout time.
pub fn execute_set_distribution_bounty(
    deps: DepsMut,
    info: MessageInfo,
    new_bounty: Uint128,
) -> Result<Response, ContractError> {
    assert_correct_factory_address(deps.as_ref(), info)?;

    if new_bounty > MAX_DISTRIBUTION_BOUNTY_USD {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Bounty exceeds max of {} (USD, 6 decimals)",
            MAX_DISTRIBUTION_BOUNTY_USD
        ))));
    }

    DISTRIBUTION_BOUNTY_USD.save(deps.storage, &new_bounty)?;

    Ok(Response::new()
        .add_attribute("action", "set_distribution_bounty")
        .add_attribute("new_bounty_usd", new_bounty.to_string()))
}

// Pool-only. Called by a pool's ContinueDistribution handler to forward
// the keeper bounty payment to the factory. The factory pays from its
// own native reserve so pool LP funds are never used for keeper
// infrastructure.
//
// Skips gracefully (returns Ok with an attribute) when:
//   - the bounty is disabled (USD value is zero)
//   - the oracle conversion fails (Pyth + cache both unavailable)
//   - the factory's native balance is below the converted amount
// Skipping rather than erroring means the pool's distribution tx never
// reverts because of bounty payout state.
pub fn execute_pay_distribution_bounty(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recipient: String,
) -> Result<Response, ContractError> {
    // Auth: caller must be a registered pool. POOLS_BY_CONTRACT_ADDRESS is
    // populated at pool creation and keyed by the pool's contract address.
    if POOLS_BY_CONTRACT_ADDRESS
        .may_load(deps.storage, info.sender.clone())?
        .is_none()
    {
        return Err(ContractError::Unauthorized {});
    }

    let bounty_usd = DISTRIBUTION_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();

    if bounty_usd.is_zero() {
        return Ok(pay_distribution_bounty_skip("disabled", &info.sender, vec![]));
    }

    let bounty_usd_attr = Attribute::new("bounty_configured_usd", bounty_usd.to_string());

    // Convert USD -> bluechip via the internal oracle. If the oracle is
    // unavailable, skip gracefully.
    let bounty_bluechip = match crate::internal_bluechip_price_oracle::usd_to_bluechip(
        deps.as_ref(),
        bounty_usd,
        env.clone(),
    ) {
        Ok(conv) => conv.amount,
        Err(_) => {
            return Ok(pay_distribution_bounty_skip(
                "price_unavailable",
                &info.sender,
                vec![bounty_usd_attr],
            ));
        }
    };

    if bounty_bluechip.is_zero() {
        return Ok(pay_distribution_bounty_skip(
            "conversion_returned_zero",
            &info.sender,
            vec![bounty_usd_attr],
        ));
    }

    let recipient_addr = deps.api.addr_validate(&recipient)?;
    let balance = deps
        .querier
        .query_balance(env.contract.address.as_str(), ORACLE_BOUNTY_DENOM)?;

    if balance.amount < bounty_bluechip {
        return Ok(pay_distribution_bounty_skip(
            "insufficient_factory_balance",
            &info.sender,
            vec![
                Attribute::new("bounty_required_bluechip", bounty_bluechip.to_string()),
                bounty_usd_attr,
                Attribute::new("factory_balance", balance.amount.to_string()),
            ],
        ));
    }

    Ok(Response::new()
        .add_message(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: recipient_addr.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: ORACLE_BOUNTY_DENOM.to_string(),
                amount: bounty_bluechip,
            }],
        }))
        .add_attribute("action", "pay_distribution_bounty")
        .add_attribute("bounty_paid_bluechip", bounty_bluechip.to_string())
        .add_attribute("bounty_paid_usd", bounty_usd.to_string())
        .add_attribute("recipient", recipient_addr.to_string())
        .add_attribute("pool", info.sender.to_string()))
}
