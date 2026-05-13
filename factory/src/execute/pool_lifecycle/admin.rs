//! Per-pool admin forwards: pause, unpause, emergency withdraw + its
//! cancel, and the stuck-state recovery escape hatch. All five handlers
//! are admin-only and wrap a single `WasmMsg::Execute` to the pool
//! contract — the pool itself gates them on
//! `info.sender == pool_info.factory_addr`, so the factory is the only
//! entity that can issue these commands.
//!
//! Also hosts `execute_notify_threshold_crossed`, the pool-to-factory
//! callback fired when a pool's commit threshold crosses; it lives with
//! the other pool-state transitions rather than in `create.rs`.

use cosmwasm_std::{
    to_json_binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response, StdError, WasmMsg,
};

use crate::error::ContractError;
use crate::mint_bluechips_pool_creation::calculate_and_mint_bluechip;
use crate::state::{POOLS_BY_ID, POOL_THRESHOLD_MINTED};

use super::super::ensure_admin;

/// Messages forwarded to the pool contract on behalf of the factory admin.
/// The pool's handler rejects anything that isn't sent by the factory, so
/// this enum is the only shape the pool ever sees for these operations.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PoolAdminMsg {
    Pause {},
    Unpause {},
    EmergencyWithdraw {},
    CancelEmergencyWithdraw {},
    RecoverStuckStates { recovery_type: crate::pool_struct::RecoveryType },
    /// post-1y-dormancy sweep of the unclaimed
    /// emergency-drain residual. Factory forwards; the pool's handler
    /// verifies dormancy elapsed and `info.sender == factory_addr`
    /// before sending the residual to the bluechip wallet.
    SweepUnclaimedEmergencyShares {},
}

fn forward_pool_admin(
    deps: Deps,
    info: MessageInfo,
    pool_id: u64,
    action: &'static str,
    pool_msg: PoolAdminMsg,
) -> Result<Response, ContractError> {
    ensure_admin(deps, &info)?;
    let pool_addr = POOLS_BY_ID
        .load(deps.storage, pool_id)
        .map_err(|_| {
            ContractError::Std(StdError::generic_err(format!(
                "Pool {} not found in registry",
                pool_id
            )))
        })?
        .creator_pool_addr;
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

/// factory-only entry point that forwards a
/// `SweepUnclaimedEmergencyShares` to a pool whose 1-year claim
/// dormancy has elapsed. The pool itself enforces both the dormancy
/// gate AND the `info.sender == factory_addr` auth check; this
/// wrapper just plumbs the admin's intent through.
pub fn execute_sweep_unclaimed_emergency_shares_pool(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    forward_pool_admin(
        deps.as_ref(),
        info,
        pool_id,
        "sweep_unclaimed_emergency_shares_pool",
        PoolAdminMsg::SweepUnclaimedEmergencyShares {},
    )
}

/// Called by a pool when its commit threshold has been crossed. Triggers
/// the bluechip mint for this pool (only once per pool — the
/// `POOL_THRESHOLD_MINTED` gate prevents a malicious pool from calling
/// back repeatedly).
pub fn execute_notify_threshold_crossed(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    // Single load covers both the caller-address check and the standard-pool
    // defense-in-depth gate below.
    let pool_details = POOLS_BY_ID.load(deps.storage, pool_id).map_err(|_| {
        ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        )))
    })?;

    if info.sender != pool_details.creator_pool_addr {
        return Err(ContractError::Std(StdError::generic_err(
            "Only the registered pool contract can notify threshold crossed",
        )));
    }

    // Defense-in-depth against a standard pool somehow reaching this code
    // path (it shouldn't — the pool-side Commit handler is gated on
    // PoolKind::Commit). Rejecting here too keeps the bluechip mint
    // schedule cleanly tied to commit-pool threshold events only.
    if pool_details.pool_kind == pool_factory_interfaces::PoolKind::Standard {
        return Err(ContractError::Std(StdError::generic_err(
            "Standard pools do not have a commit threshold to cross",
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
