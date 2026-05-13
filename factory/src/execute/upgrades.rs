//! Pool wasm upgrade proposal + batched migrate apply.
//!
//! Two-phase flow:
//! - `ExecuteMsg::UpgradePools` → `execute_propose_pool_upgrade`
//! (registers a `PENDING_POOL_UPGRADE`, starts the 48h timelock)
//! - `ExecuteMsg::ExecutePoolUpgrade` → `execute_apply_pool_upgrade`
//! (runs the FIRST batch of 10 migrates once the timelock elapses)
//! - `ExecuteMsg::ContinuePoolUpgrade` → `execute_continue_pool_upgrade`
//! (runs each subsequent batch; admin must call once per batch because
//! self-dispatch would risk blowing through block gas limits)
//! - `ExecuteMsg::CancelPoolUpgrade` → `execute_cancel_pool_upgrade`
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
    // 1. Caller-supplied IDs are deduplicated. Duplicates would emit
    // two `WasmMsg::Migrate` to the same pool — the first migrates,
    // the second runs the migrate handler against the already-new
    // code with the same migrate_msg, producing whatever the new
    // code's handler does on a second invocation (probably
    // undefined behaviour, certainly not what the admin intended).
    // 2. Every supplied ID must reference a registered pool. An
    // invalid ID would error inside `build_upgrade_batch` at apply
    // time and revert the entire batch.
    // 3. The anchor pool is rejected. Migrating it mid-flight
    // would leave the oracle querying `GetPoolState` against
    // possibly-mid-migration storage; if the migrate changes
    // the reserve representation the cumulative-delta math
    // breaks silently. Operators must propose a new anchor first
    // (CreateStandardPool + 48h ProposeConfigUpdate / one-shot
    // SetAnchorPool semantics), repoint the factory at it, then
    // migrate the old anchor in a dedicated cycle.
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
            pending_retry: Vec::new(),
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_pool_upgrade")
        .add_attribute("new_code_id", new_code_id.to_string())
        .add_attribute("pool_count", pools_to_upgrade.len().to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

/// Outcome of processing a batch through [`build_upgrade_batch`].
///
/// Paused pools land in `skipped_pool_ids` and (for first-pass calls)
/// flow into `PoolUpgrade.pending_retry` so a later `ContinuePoolUpgrade`
/// can retry them once the admin has unpaused. Migrated pools are
/// returned in `migrated_pool_ids` so retry-pass callers know which
/// entries to drop from the retry queue.
struct UpgradeBatchOutcome {
    messages: Vec<CosmosMsg>,
    skipped_pool_ids: Vec<u64>,
    migrated_pool_ids: Vec<u64>,
}

/// Processes a single batch of pools from the pending upgrade.
///
/// Anchor exclusion runs here in addition to the propose-time check.
/// `execute_propose_pool_upgrade` snapshots the anchor at propose time, but
/// the pending list can outlive an anchor change (Propose pool upgrade
/// containing pool X -> 48h elapses -> ProposeConfigUpdate that promotes X
/// to anchor -> apply config update -> apply upgrade: X is now the live
/// anchor but is still in the frozen list). Re-resolving the anchor here
/// and hard-failing if it appears in the batch closes that race. We
/// hard-fail rather than silently skip so an operator notices the
/// collision and decides whether to drop X from the upgrade or rotate
/// the anchor first.
///
/// Paused pools are skipped (returned in `skipped_pool_ids`) so the
/// upgrade doesn't migrate a pool that is mid-emergency-withdraw or
/// otherwise in a sensitive state.
///
/// `IsPaused` query error semantics depend on `retry_mode`:
///
/// * `retry_mode == false` (first-pass): the query result is
/// load-bearing — we're about to commit to either migrating or
/// deferring each pool. A query error means we genuinely don't know
/// whether the pool is in a sensitive state; treating "unknown" as
/// "not paused" would be a fail-open that lets a pool with a broken
/// query path silently get migrated despite possibly being paused.
/// Instead we propagate the error and the apply reverts. The admin
/// can then `Cancel` and either drop the unreachable pool from the
/// batch via a fresh `Propose` or repair the pool first.
///
/// * `retry_mode == true` (revisiting `pending_retry`): the pool was
/// already deferred once; we're seeing if it has become migrate-able.
/// A query error here is treated as "still not migrate-able" (push
/// back into `skipped_pool_ids`) rather than reverting the entire
/// retry batch — that way a single transiently-broken pool can't
/// veto progress on the other retry candidates.
fn build_upgrade_batch(
    deps: Deps,
    pool_ids: &[u64],
    new_code_id: u64,
    migrate_msg: &Binary,
    retry_mode: bool,
) -> Result<UpgradeBatchOutcome, ContractError> {
    let mut messages: Vec<CosmosMsg> = Vec::new();
    let mut skipped: Vec<u64> = Vec::new();
    let mut migrated: Vec<u64> = Vec::new();

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

        let is_paused: pool_factory_interfaces::IsPausedResponse = match deps
            .querier
            .query_wasm_smart(
                pool_addr.to_string(),
                &pool_factory_interfaces::PoolQueryMsg::IsPaused {},
            ) {
            Ok(r) => r,
            Err(e) => {
                if retry_mode {
                    // Keep the pool on the retry queue so the admin can
                    // try again after addressing whatever is making
                    // IsPaused unreachable.
                    skipped.push(*pool_id);
                    continue;
                } else {
                    return Err(ContractError::Std(StdError::generic_err(format!(
                        "Pool {} ({}) IsPaused query failed: {}. Refusing to \
                         migrate without confirming pause state — Cancel and \
                         either fix the pool or re-propose without it.",
                        pool_id, pool_addr, e
                    ))));
                }
            }
        };

        if is_paused.paused {
            skipped.push(*pool_id);
            continue;
        }

        messages.push(CosmosMsg::Wasm(WasmMsg::Migrate {
            contract_addr: pool_addr.to_string(),
            new_code_id,
            msg: migrate_msg.clone(),
        }));
        migrated.push(*pool_id);
    }

    Ok(UpgradeBatchOutcome {
        messages,
        skipped_pool_ids: skipped,
        migrated_pool_ids: migrated,
    })
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
    let first_batch_len = first_batch.len() as u32;

    let outcome = build_upgrade_batch(
        deps.as_ref(),
        &first_batch,
        upgrade.new_code_id,
        &upgrade.migrate_msg,
        false, // first-pass: a query error reverts
    )?;

    let mut upgrade = upgrade;
    upgrade.upgraded_count = first_batch_len;
    // Paused pools land in pending_retry instead of being silently
    // counted-and-dropped. A later ContinuePoolUpgrade re-checks each
    // entry and migrates the ones that have unpaused (in-place
    // retry). first-pass-only: this list starts empty so we just
    // assign rather than extend.
    upgrade.pending_retry = outcome.skipped_pool_ids.clone();

    let total = upgrade.pools_to_upgrade.len() as u32;
    let more_first_pass = upgrade.upgraded_count < total;
    let has_retry_queue = !upgrade.pending_retry.is_empty();
    if more_first_pass || has_retry_queue {
        PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;
    } else {
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    let skipped_str = outcome
        .skipped_pool_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Ok(Response::new()
        .add_messages(outcome.messages)
        .add_attribute("action", "execute_pool_upgrade")
        .add_attribute("new_code_id", upgrade.new_code_id.to_string())
        .add_attribute("pool_count", total.to_string())
        .add_attribute("processed_in_batch", first_batch_len.to_string())
        .add_attribute("skipped_paused", skipped_str)
        .add_attribute("more_batches", (more_first_pass || has_retry_queue).to_string())
        .add_attribute("pending_retry_count", upgrade.pending_retry.len().to_string()))
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

    let total = upgrade.pools_to_upgrade.len() as u32;
    let batch_size: usize = 10;

    // Continue has two phases. While the first-pass cursor hasn't
    // reached the end of `pools_to_upgrade`, each call advances it by
    // up to `batch_size`. Once the first pass is done, subsequent
    // calls drain `pending_retry`: re-query IsPaused for each
    // candidate and migrate the ones that have unpaused since first
    // pass. The upgrade is complete only when BOTH the first pass is
    // exhausted AND the retry queue is empty.
    let (mode, batch, batch_len) = if upgrade.upgraded_count < total {
        let slice: Vec<u64> = upgrade
            .pools_to_upgrade
            .iter()
            .cloned()
            .skip(upgrade.upgraded_count as usize)
            .take(batch_size)
            .collect();
        let len = slice.len();
        ("first_pass", slice, len)
    } else if !upgrade.pending_retry.is_empty() {
        let slice: Vec<u64> = upgrade
            .pending_retry
            .iter()
            .cloned()
            .take(batch_size)
            .collect();
        let len = slice.len();
        ("retry", slice, len)
    } else {
        // Nothing left to do; close out the upgrade.
        PENDING_POOL_UPGRADE.remove(deps.storage);
        return Ok(Response::new()
            .add_attribute("action", "upgrade_complete")
            .add_attribute("total_upgraded", upgrade.upgraded_count.to_string()));
    };

    let outcome = build_upgrade_batch(
        deps.as_ref(),
        &batch,
        upgrade.new_code_id,
        &upgrade.migrate_msg,
        mode == "retry", // retry-mode: tolerate IsPaused query errors
    )?;

    if mode == "first_pass" {
        upgrade.upgraded_count += batch_len as u32;
        // Pools paused on first pass move into pending_retry for later
        // revisit. Dedup defensively even though `pools_to_upgrade` is
        // deduped at propose time.
        for id in outcome.skipped_pool_ids.iter() {
            if !upgrade.pending_retry.contains(id) {
                upgrade.pending_retry.push(*id);
            }
        }
    } else {
        // Retry pass. Drop the ones we just migrated from pending_retry;
        // any in `skipped_pool_ids` are still-paused/unreachable and
        // remain in pending_retry (their position is preserved via the
        // `retain` below).
        let migrated_set: std::collections::HashSet<u64> =
            outcome.migrated_pool_ids.iter().copied().collect();
        upgrade.pending_retry.retain(|id| !migrated_set.contains(id));
    }

    let more_first_pass = upgrade.upgraded_count < total;
    let has_retry_queue = !upgrade.pending_retry.is_empty();
    let more_batches = more_first_pass || has_retry_queue;
    if more_batches {
        PENDING_POOL_UPGRADE.save(deps.storage, &upgrade)?;
    } else {
        PENDING_POOL_UPGRADE.remove(deps.storage);
    }

    let skipped_str = outcome
        .skipped_pool_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Ok(Response::new()
        .add_messages(outcome.messages)
        .add_attribute("action", "continue_upgrade")
        .add_attribute("mode", mode)
        .add_attribute("processed_in_batch", batch_len.to_string())
        .add_attribute("migrated_in_batch", outcome.migrated_pool_ids.len().to_string())
        .add_attribute("total_first_pass", upgrade.upgraded_count.to_string())
        .add_attribute("skipped_paused", skipped_str)
        .add_attribute("pending_retry_count", upgrade.pending_retry.len().to_string())
        .add_attribute("more_batches", more_batches.to_string()))
}
