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

// Explicit re-exports keep the public surface of `crate::execute::*`
// auditable from this file rather than implicitly extending whenever a
// submodule adds a new `pub fn`. Adding a handler now requires touching
// the dispatcher in this file, which keeps the two in step.
pub use config::{
    execute_apply_pool_config_update, execute_cancel_factory_config_update,
    execute_cancel_pool_config_update, execute_propose_factory_config_update,
    execute_propose_pool_config_update, execute_update_factory_config,
};
// `validate_factory_config` is intentionally NOT re-exported — it's
// reached via the `config::validate_factory_config(...)` path in
// `instantiate` so the gate is visible at the call site.
pub use oracle::{
    execute_apply_add_oracle_eligible_pool, execute_apply_set_commit_pools_auto_eligible,
    execute_cancel_add_oracle_eligible_pool, execute_cancel_set_commit_pools_auto_eligible,
    execute_pay_distribution_bounty, execute_propose_add_oracle_eligible_pool,
    execute_propose_set_commit_pools_auto_eligible, execute_refresh_oracle_pool_snapshot,
    execute_remove_oracle_eligible_pool, execute_set_anchor_pool,
    execute_set_distribution_bounty, execute_set_oracle_update_bounty,
    execute_set_pyth_conf_threshold_bps,
};
pub use pool_lifecycle::admin::{
    execute_cancel_emergency_withdraw_pool, execute_emergency_withdraw_pool,
    execute_notify_threshold_crossed, execute_pause_pool, execute_recover_pool_stuck_states,
    execute_sweep_unclaimed_emergency_shares_pool, execute_unpause_pool,
};
pub use upgrades::{
    execute_apply_pool_upgrade, execute_cancel_pool_upgrade, execute_continue_pool_upgrade,
    execute_propose_pool_upgrade,
};

use crate::error::ContractError;
use crate::internal_bluechip_price_oracle::{
    execute_cancel_bootstrap_price, execute_cancel_force_rotate_pools,
    execute_confirm_bootstrap_price, execute_force_rotate_pools,
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
use cosmwasm_std::{Deps, DepsMut, Env, MessageInfo, Reply, Response, Uint128};

use crate::{CONTRACT_NAME, CONTRACT_VERSION};

// Reply step constants (stored in low 8 bits of reply ID).
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
// Standard-pool reply chain. Sparse numbering leaves room for additional
// commit-pool steps (4–9) without clashing.
pub const MINT_STANDARD_NFT: u64 = 10;
pub const FINALIZE_STANDARD_POOL: u64 = 11;

/// Encodes a `pool_id` and a reply-chain step into a single SubMsg reply ID.
///
/// Layout: low 8 bits = step, high 56 bits = pool_id.
/// Step IDs MUST fit in 8 bits (0..=0xFF). Pool IDs are bumped by a single
/// counter per pool create and so cannot reach 2^56 in any realistic
/// deployment, but the asserts keep these invariants explicit so a future
/// step-constant change above 0xFF or a malformed pool_id is caught in
/// debug builds before it silently truncates and routes to UnknownReplyId.
pub fn encode_reply_id(pool_id: u64, step: u64) -> u64 {
    debug_assert!(step <= 0xFF, "reply step {} does not fit in 8 bits", step);
    debug_assert!(
        pool_id < (1u64 << 56),
        "pool_id {} risks truncation in reply id",
        pool_id
    );
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
        ExecuteMsg::SetPythConfThresholdBps { bps } => {
            execute_set_pyth_conf_threshold_bps(deps, info, bps)
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
        ExecuteMsg::SweepUnclaimedEmergencyPool { pool_id } => {
            execute_sweep_unclaimed_emergency_shares_pool(deps, info, pool_id)
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
        ExecuteMsg::ConfirmBootstrapPrice {} => {
            execute_confirm_bootstrap_price(deps, env, info)
        }
        ExecuteMsg::CancelBootstrapPrice {} => execute_cancel_bootstrap_price(deps, info),
        ExecuteMsg::PruneRateLimits { batch_size } => {
            execute_prune_rate_limits(deps, env, batch_size)
        }
        ExecuteMsg::ProposeAddOracleEligiblePool { pool_addr } => {
            execute_propose_add_oracle_eligible_pool(deps, env, info, pool_addr)
        }
        ExecuteMsg::ApplyAddOracleEligiblePool { pool_addr } => {
            execute_apply_add_oracle_eligible_pool(deps, env, info, pool_addr)
        }
        ExecuteMsg::CancelAddOracleEligiblePool { pool_addr } => {
            execute_cancel_add_oracle_eligible_pool(deps, info, pool_addr)
        }
        ExecuteMsg::RemoveOracleEligiblePool { pool_addr } => {
            execute_remove_oracle_eligible_pool(deps, info, pool_addr)
        }
        ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled } => {
            execute_propose_set_commit_pools_auto_eligible(deps, env, info, enabled)
        }
        ExecuteMsg::ApplySetCommitPoolsAutoEligible {} => {
            execute_apply_set_commit_pools_auto_eligible(deps, env, info)
        }
        ExecuteMsg::CancelSetCommitPoolsAutoEligible {} => {
            execute_cancel_set_commit_pools_auto_eligible(deps, info)
        }
        ExecuteMsg::RefreshOraclePoolSnapshot {} => {
            execute_refresh_oracle_pool_snapshot(deps, env)
        }
    }
}

/// Permissionless storage hygiene (MEDIUM-2 audit fix). Iterates the
/// per-address rate-limit maps and removes entries older than 10× the
/// per-map cooldown window.
///
/// `batch_size` caps the number of entries REMOVED per call (default
/// 100, hard cap 500). It does NOT cap the number iterated — each
/// phase walks its map until either `batch_size` stale entries have
/// been collected or the map ends. For realistic deployment scales
/// this is fine: per-address 1h cooldowns mean the map can't grow
/// faster than ~24 entries/day/active-address, and the keeper runs
/// prune ~daily, so the map stays small enough that full iteration
/// is well within block gas. If the map ever does balloon (extended
/// prune outage, deliberate storage-bloat attack), operators tune
/// the keeper to run more frequently AND can manually invoke this
/// handler with larger `batch_size` to drain the backlog.
///
/// Without this handler, the rate-limit maps grow monotonically as
/// new addresses interact and never shrink. Pruning is anybody's
/// job: ops, keepers, or any community member can run it.
fn execute_prune_rate_limits(
    deps: DepsMut,
    env: Env,
    batch_size: Option<u32>,
) -> Result<Response, ContractError> {
    let batch = batch_size.unwrap_or(100).min(500) as usize;
    let now_secs = env.block.time.seconds();

    // 10× the longer of the two cooldowns. Both are 1h today so this
    // is 10h, well beyond any legitimate user's natural retry cadence.
    let stale_after_secs = std::cmp::max(
        crate::state::COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS,
        crate::state::STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS,
    )
    .saturating_mul(10);

    // Each map gets its own `batch` budget so a `batch_size` of 100
    // prunes up to 100 from each.
    let commit_pruned = prune_rate_limit_map(
        deps.storage,
        crate::state::LAST_COMMIT_POOL_CREATE_AT,
        now_secs,
        stale_after_secs,
        batch,
    )?;
    let std_pruned = prune_rate_limit_map(
        deps.storage,
        crate::state::LAST_STANDARD_POOL_CREATE_AT,
        now_secs,
        stale_after_secs,
        batch,
    )?;

    Ok(Response::new()
        .add_attribute("action", "prune_rate_limits")
        .add_attribute("commit_pruned", commit_pruned.to_string())
        .add_attribute("standard_pruned", std_pruned.to_string())
        .add_attribute("stale_after_secs", stale_after_secs.to_string())
        .add_attribute("batch_size", batch.to_string()))
}

/// Prune up to `batch` entries from a per-address `Addr -> Timestamp`
/// rate-limit map whose timestamp is older than
/// `now_secs - stale_after_secs`. Returns the number of entries actually
/// removed (`<= batch`). Centralized so adding a third such map (e.g.
/// per-keeper bounty cooldown) is a one-line addition rather than a
/// copy-pasted loop with risk of attribute-key drift.
fn prune_rate_limit_map(
    storage: &mut dyn cosmwasm_std::Storage,
    map: cw_storage_plus::Map<cosmwasm_std::Addr, cosmwasm_std::Timestamp>,
    now_secs: u64,
    stale_after_secs: u64,
    batch: usize,
) -> cosmwasm_std::StdResult<u32> {
    use cosmwasm_std::Order;

    let mut to_remove: Vec<cosmwasm_std::Addr> = Vec::new();
    for entry in map.range(storage, None, None, Order::Ascending) {
        if to_remove.len() >= batch {
            break;
        }
        let (addr, ts) = entry?;
        if now_secs.saturating_sub(ts.seconds()) >= stale_after_secs {
            to_remove.push(addr);
        }
    }
    let mut pruned: u32 = 0;
    for addr in to_remove.into_iter() {
        map.remove(storage, addr);
        pruned = pruned.saturating_add(1);
    }
    Ok(pruned)
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
/// Loads the factory config and rejects with [`ContractError::Unauthorized`]
/// if `info.sender` does not match `factory_admin_address`.
pub fn ensure_admin(deps: Deps, info: &MessageInfo) -> Result<(), ContractError> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if info.sender != config.factory_admin_address {
        return Err(ContractError::Unauthorized {});
    }
    Ok(())
}
