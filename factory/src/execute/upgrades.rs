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

    // Pool list resolution + validation. Three rules, all enforced at
    // propose time so a malformed admin-supplied list fails immediately
    // rather than 48h later at apply (where the failure cascades into
    // "cancel + re-propose + wait 48h again"):
    //
    //   1. Caller-supplied IDs are deduplicated. Duplicates would emit
    //      two `WasmMsg::Migrate` to the same pool — the first migrates,
    //      the second runs the migrate handler against the already-new
    //      code with the same migrate_msg, producing whatever the new
    //      code's handler does on a second invocation (probably
    //      undefined behaviour, certainly not what the admin intended).
    //   2. Every supplied ID must reference a registered pool. An
    //      invalid ID would error inside `build_upgrade_batch` at apply
    //      time and revert the entire batch.
    //   3. The anchor pool is rejected. Migrating it mid-flight
    //      would leave the oracle querying `GetPoolState` against
    //      possibly-mid-migration storage; if the migrate changes
    //      the reserve representation the cumulative-delta math
    //      breaks silently. Operators must propose a new anchor first
    //      (CreateStandardPool + 48h ProposeConfigUpdate / one-shot
    //      SetAnchorPool semantics), repoint the factory at it, then
    //      migrate the old anchor in a dedicated cycle.
    //
    // None means "all pools" — same dedup/existence is implicit
    // (POOLS_BY_ID.keys already returns unique, registered IDs) but
    // we still need to filter the anchor.
    let pools_to_upgrade: Vec<u64> = if let Some(ids) = pool_ids {
        // Dedup while preserving order: first occurrence wins, later
        // duplicates dropped. Sort+dedup would also work but reorders
        // the admin's intended migration sequence.
        let mut seen = std::collections::HashSet::with_capacity(ids.len());
        let mut deduped: Vec<u64> = Vec::with_capacity(ids.len());
        for id in ids.into_iter() {
            if seen.insert(id) {
                deduped.push(id);
            }
        }
        // Existence check.
        for id in deduped.iter() {
            if !POOLS_BY_ID.has(deps.storage, *id) {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Pool {} not found in registry — cannot include in upgrade batch",
                    id
                ))));
            }
        }
        deduped
    } else {
        POOLS_BY_ID
            .keys(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?
    };

    // Anchor-pool exclusion. Resolve the configured anchor address
    // once, walk the proposed list, refuse if the anchor pool is in it.
    let factory_config = crate::state::FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let anchor_addr = factory_config.atom_bluechip_anchor_pool_address.clone();
    for id in pools_to_upgrade.iter() {
        let details = POOLS_BY_ID.load(deps.storage, *id)?;
        if details.creator_pool_addr == anchor_addr {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Pool {} is the configured anchor pool ({}). Migrating the \
                 anchor mid-flight would leave the oracle querying \
                 possibly-mid-migration storage. Repoint the factory at a new \
                 anchor (CreateStandardPool → 48h ProposeConfigUpdate) before \
                 migrating this pool.",
                id, anchor_addr
            ))));
        }
    }

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
//
// Anchor exclusion runs here in addition to the propose-time check.
// `execute_propose_pool_upgrade` snapshots the anchor at propose time, but
// the pending list can outlive an anchor change (Propose pool upgrade
// containing pool X -> 48h elapses -> ProposeConfigUpdate that promotes X
// to anchor -> apply config update -> apply upgrade: X is now the live
// anchor but is still in the frozen list). Re-resolving the anchor here
// and hard-failing if it appears in the batch closes that race. We
// hard-fail rather than silently skip so an operator notices the
// collision and decides whether to drop X from the upgrade or rotate
// the anchor first.
fn build_upgrade_batch(
    deps: Deps,
    pool_ids: &[u64],
    new_code_id: u64,
    migrate_msg: &Binary,
) -> Result<(Vec<CosmosMsg>, Vec<u64>, u32), ContractError> {
    let mut messages: Vec<CosmosMsg> = Vec::new();
    let mut skipped: Vec<u64> = Vec::new();
    let processed: u32 = pool_ids.len() as u32;

    let current_anchor_addr = crate::state::FACTORYINSTANTIATEINFO
        .load(deps.storage)?
        .atom_bluechip_anchor_pool_address;

    for pool_id in pool_ids.iter() {
        let pool_addr = POOLS_BY_ID.load(deps.storage, *pool_id)?.creator_pool_addr;

        if pool_addr == current_anchor_addr {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Pool {} ({}) is the current anchor pool. The pending upgrade was \
                 created when a different pool was the anchor; an anchor change \
                 has landed since. Cancel this upgrade, choose whether to keep \
                 the new anchor or drop pool {} from the batch, and re-propose.",
                pool_id, pool_addr, pool_id
            ))));
        }

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
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Admin-only now. Previously this was self-called from
    // execute_apply_pool_upgrade, which worked only until the pool list grew
    // large enough that the chained execute messages exceeded block gas
    // limits in a single tx. Making this admin-only forces batches to be
    // submitted as separate transactions, each with its own gas budget.
    ensure_admin(deps.as_ref(), &info)?;

    let mut upgrade = PENDING_POOL_UPGRADE.load(deps.storage)?;

    // Honor the same 48h `ADMIN_TIMELOCK_SECONDS` window `apply` enforces.
    // Without this gate, an admin could `Propose -> Continue` directly and
    // migrate batches before the community observation window elapses.
    if env.block.time < upgrade.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: upgrade.effective_after,
        });
    }

    // Require `apply` to have run at least once. `upgraded_count > 0` is the
    // only on-chain signal that the timelock check fired and the first
    // batch was processed under that gate; gating `Continue` on it forces
    // the canonical sequence `Propose -> wait 48h -> ExecutePoolUpgrade ->
    // ContinuePoolUpgrade*` and rejects a `Propose -> Continue` shortcut
    // even in the (unlikely) case `effective_after` has elapsed.
    if upgrade.upgraded_count == 0 {
        return Err(ContractError::Std(StdError::generic_err(
            "Must call ExecutePoolUpgrade first to start the upgrade. \
             ContinuePoolUpgrade only resumes an in-progress batch.",
        )));
    }

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
