//! Factory contract entry points + shared reply-ID machinery.
//!
//! The bulk of the handler logic has been split into four submodules
//! by message family:
//!
//!   - [`config`]         — propose / apply / cancel for both factory
//!                          config and per-pool config (48h timelock on
//!                          every propose/apply pair).
//!   - [`pool_lifecycle`] — create (commit + standard), pause, unpause,
//!                          emergency withdraw (+ cancel), stuck-state
//!                          recovery, and the threshold-crossed
//!                          callback from pools.
//!   - [`oracle`]         — keeper bounty caps, the pay-distribution-
//!                          bounty forward, and the one-shot anchor
//!                          pool set. The TWAP math itself lives in
//!                          [`crate::internal_bluechip_price_oracle`].
//!   - [`upgrades`]       — pool wasm upgrade proposal + batched migrate
//!                          apply.
//!
//! This file keeps the `#[entry_point]` exports (`instantiate`,
//! `execute`, `reply`), the cross-module helpers (`ensure_admin`,
//! `encode_reply_id`, `decode_reply_id`), and the reply-step
//! constants. Every other public item in `crate::execute` is
//! re-exported from a submodule via `pub use`.

pub mod config;
pub mod oracle;
pub mod pool_lifecycle;
pub mod upgrades;

pub use config::*;
pub use oracle::*;
pub use pool_lifecycle::*;
pub use upgrades::*;

use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_cancel_force_rotate_pools, execute_force_rotate_pools,
    execute_propose_force_rotate_pools, initialize_internal_bluechip_oracle,
    update_internal_oracle_price,
};
use crate::msg::ExecuteMsg;
use crate::pool_creation_reply::{finalize_pool, mint_create_pool, set_tokens};
use crate::state::{
    DISTRIBUTION_BOUNTY_USD, FACTORYINSTANTIATEINFO, INITIAL_ANCHOR_SET,
    ORACLE_UPDATE_BOUNTY_USD,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    Deps, DepsMut, Env, MessageInfo, Reply, Response, StdError, StdResult, Uint128,
};

const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// Reply step constants (stored in low 8 bits of reply ID).
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
// Standard-pool reply chain. Sparse numbering leaves room for additional
// commit-pool steps (4–9) without clashing.
pub const MINT_STANDARD_NFT: u64 = 10;
pub const FINALIZE_STANDARD_POOL: u64 = 11;

/// Encodes a `pool_id` and a reply-chain step into a single SubMsg reply ID.
pub fn encode_reply_id(pool_id: u64, step: u64) -> u64 {
    (pool_id << 8) | (step & 0xFF)
}

/// Decodes a reply ID back into `(pool_id, step)`.
pub fn decode_reply_id(reply_id: u64) -> (u64, u64) {
    (reply_id >> 8, reply_id & 0xFF)
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: crate::state::FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    config::validate_factory_config(deps.as_ref(), &msg)?;

    FACTORYINSTANTIATEINFO.save(deps.storage, &msg)?;
    // Anchor address starts as whatever the deployer passes (typically a
    // placeholder wallet); the one-shot SetAnchorPool overwrites it with
    // the real anchor pool's contract address after that pool is created.
    INITIAL_ANCHOR_SET.save(deps.storage, &false)?;
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
        ExecuteMsg::ProposeConfigUpdate { config } => {
            execute_propose_factory_config_update(deps, env, info, config)
        }
        ExecuteMsg::UpdateConfig {} => execute_update_factory_config(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_factory_config_update(deps, info),
        ExecuteMsg::Create {
            pool_msg,
            token_info,
        } => pool_lifecycle::create::execute_create_creator_pool(deps, env, info, pool_msg, token_info),
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
        ExecuteMsg::CreateStandardPool {
            pool_token_info,
            label,
        } => pool_lifecycle::create::execute_create_standard_pool(deps, env, info, pool_token_info, label),
        ExecuteMsg::SetAnchorPool { pool_id } => {
            execute_set_anchor_pool(deps, env, info, pool_id)
        }
    }
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
        MINT_STANDARD_NFT => {
            crate::pool_creation_reply::mint_standard_nft(deps, env, msg, pool_id)
        }
        FINALIZE_STANDARD_POOL => {
            crate::pool_creation_reply::finalize_standard_pool(deps, env, msg, pool_id)
        }
        _ => Err(ContractError::UnknownReplyId { id: msg.id }),
    }
}

/// Admin gate used by every admin-only handler in this module's submodules.
/// Loads the factory config and rejects if `info.sender` does not match
/// `factory_admin_address`.
pub fn ensure_admin(deps: Deps, info: &MessageInfo) -> StdResult<()> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    if info.sender != config.factory_admin_address {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.factory_admin_address, info.sender
        )));
    }
    Ok(())
}
