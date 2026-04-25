//! Factory- and pool-level config propose/apply/cancel handlers.
//!
//! Every handler in this module is admin-only (gated through
//! [`super::ensure_admin`]) and, for the propose/apply pairs, subject to
//! the standard 48h [`ADMIN_TIMELOCK_SECONDS`] timelock so the community
//! has a full two-day observability window before a mutation lands.

use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, MessageInfo, Response, StdError, WasmMsg,
};

use crate::error::ContractError;
use crate::pool_struct::PoolConfigUpdate;
use crate::state::{
    FactoryInstantiate, PendingConfig, PendingPoolConfig, ADMIN_TIMELOCK_SECONDS,
    FACTORYINSTANTIATEINFO, PENDING_CONFIG, PENDING_POOL_CONFIG, POOLS_BY_ID,
};

use super::ensure_admin;

/// Validates every caller-supplied address + the bluechip_denom on a
/// `FactoryInstantiate` payload. Shared between `instantiate` and
/// `execute_propose_factory_config_update` so the same rules apply to
/// the initial config and any subsequent config proposal.
pub(crate) fn validate_factory_config(
    deps: cosmwasm_std::Deps,
    config: &FactoryInstantiate,
) -> Result<(), ContractError> {
    deps.api.addr_validate(config.factory_admin_address.as_str())?;
    deps.api.addr_validate(config.bluechip_wallet_address.as_str())?;
    deps.api
        .addr_validate(config.atom_bluechip_anchor_pool_address.as_str())?;
    if let Some(ref mint_addr) = config.bluechip_mint_contract_address {
        deps.api.addr_validate(mint_addr.as_str())?;
    }
    if config.bluechip_denom.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "bluechip_denom must be non-empty",
        )));
    }
    Ok(())
}

pub fn execute_update_factory_config(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

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
    ensure_admin(deps.as_ref(), &info)?;
    // Validate at propose time so any mistake surfaces 48h earlier than it
    // otherwise would (the existing config keeps flowing until the timelock
    // elapses and the admin calls UpdateConfig, but a malformed proposal
    // should fail loudly now, not then).
    validate_factory_config(deps.as_ref(), &config)?;

    let pending = PendingConfig {
        new_config: config,
        effective_after: env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS),
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
    ensure_admin(deps.as_ref(), &info)?;
    PENDING_CONFIG.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

pub fn execute_propose_pool_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
    update_msg: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    // Verify pool exists. `.has()` skips deserializing PoolDetails since
    // we only need the existence check, not the value.
    if !POOLS_BY_ID.has(deps.storage, pool_id) {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        ))));
    }

    if PENDING_POOL_CONFIG
        .may_load(deps.storage, pool_id)?
        .is_some()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "A pool config update is already pending for this pool. Cancel it first.",
        )));
    }

    let effective_after = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS);

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
    ensure_admin(deps.as_ref(), &info)?;

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

    let pool_addr = POOLS_BY_ID.load(deps.storage, pool_id)?.creator_pool_addr;

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
    ensure_admin(deps.as_ref(), &info)?;

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
