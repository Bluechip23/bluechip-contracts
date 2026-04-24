//! Pool wasm upgrade proposal + batched migrate apply.
//!
//! Two-phase flow:
//!   - `ExecuteMsg::UpgradePools` → `execute_propose_pool_upgrade`
//!     (registers a `PENDING_POOL_UPGRADE`, starts the 48h timelock)
//!   - `ExecuteMsg::ExecutePoolUpgrade` → `execute_apply_pool_upgrade`
//!     (runs the FIRST batch of 10 migrates once the timelock elapses)
//!   - `ExecuteMsg::ContinuePoolUpgrade` → `execute_continue_pool_upgrade`
//!     (runs each subsequent batch; admin must call once per batch because
//!     self-dispatch would risk blowing through block gas limits)
//!   - `ExecuteMsg::CancelPoolUpgrade` → `execute_cancel_pool_upgrade`
//!
//! Paused pools are skipped rather than migrated — the admin must unpause
//! and re-run to include them.

use cosmwasm_std::{
    Binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult,
    WasmMsg,
};

use crate::error::ContractError;
use crate::state::{
    PoolUpgrade, ADMIN_TIMELOCK_SECONDS, PENDING_POOL_UPGRADE, POOLS_BY_ID,
};

use super::ensure_admin;

pub fn execute_propose_pool_upgrade(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    new_code_id: u64,
    // None = all pools
    pool_ids: Option<Vec<u64>>,
    migrate_msg: Binary,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    if PENDING_POOL_UPGRADE.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "A pool upgrade is already pending. Cancel it first.",
        )));
    }

    let pools_to_upgrade = if let Some(ids) = pool_ids {
        ids
    } else {
        POOLS_BY_ID
            .keys(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?
    };

    let effective_after = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS);

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
        let pool_addr = POOLS_BY_ID.load(deps.storage, *pool_id)?.creator_pool_addr;

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
    ensure_admin(deps.as_ref(), &info)?;

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
    ensure_admin(deps.as_ref(), &info)?;

    let upgrade = PENDING_POOL_UPGRADE.may_load(deps.storage)?;
    if upgrade.is_none() {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending pool upgrade to cancel",
        )));
    }

    PENDING_POOL_UPGRADE.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_pool_upgrade"))
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
    ensure_admin(deps.as_ref(), &info)?;

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
