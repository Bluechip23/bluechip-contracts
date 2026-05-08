//! Post-threshold distribution batch processing.
//!
//! Once a pool crosses its commit threshold, the commit ledger is paid
//! out to committers in batches. Each call to
//! `execute_continue_distribution` processes the next slice, awards the
//! caller the distribution bounty (paid by the factory from its own
//! native reserve, not LP funds), and advances the cursor in
//! `DISTRIBUTION_STATE` until the pool is fully distributed.

use cosmwasm_std::{to_json_binary, CosmosMsg, DepsMut, Env, MessageInfo, Response, SubMsg, WasmMsg};

use crate::admin::ensure_not_drained;
use crate::error::ContractError;
use crate::generic_helpers::process_distribution_batch;
use crate::state::{
    CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS, DISTRIBUTION_STATE,
    LAST_CONTINUE_DISTRIBUTION_AT, POOL_INFO, POOL_PAUSED,
};

pub fn execute_continue_distribution(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Defense in depth: emergency_withdraw flips is_distributing=false on
    // drain (admin.rs::execute_emergency_withdraw), so the load+check below
    // already rejects calls on drained pools — but reading EMERGENCY_DRAINED
    // up front fails early with the canonical error and avoids the keeper
    // ever issuing a tx against a drained pool.
    ensure_not_drained(deps.storage)?;

    // Honor admin pause. Distribution is permissionless (any keeper may
    // call it), but it mints creator tokens to committers — exactly the
    // kind of state-mutating activity an admin pause is meant to halt
    // while investigating suspicious behavior. Every other liquidity-
    // touching path goes through `check_pool_writable`, which checks
    // both pause AND drain; this brings distribution under the same
    // uniform halt semantics. Pause is reversible by the factory, so
    // legitimate distribution resumes once the admin clears the pause.
    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }

    // Per-address rate limit. Same-block spam from a single keeper
    // (or two competing keepers) wastes gas on no-op tx after the
    // ledger is empty / cursor is past the end. Reject if this address
    // called within the last CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS.
    // Stamp the new timestamp before any further work — a successful
    // tx records the call; a tx that errors below atomically reverts
    // the stamp along with everything else.
    let now = env.block.time.seconds();
    if let Some(prev) =
        LAST_CONTINUE_DISTRIBUTION_AT.may_load(deps.storage, &info.sender)?
    {
        let earliest_next = prev.saturating_add(CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS);
        if now < earliest_next {
            return Err(ContractError::ContinueDistributionRateLimited {
                earliest_next,
                last_call: prev,
                cooldown_seconds: CONTINUE_DISTRIBUTION_RATE_LIMIT_SECONDS,
            });
        }
    }
    LAST_CONTINUE_DISTRIBUTION_AT.save(deps.storage, &info.sender, &now)?;

    let dist_state = DISTRIBUTION_STATE.load(deps.storage)?;
    if !dist_state.is_distributing {
        return Err(ContractError::NothingToRecover {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;

    // process_distribution_batch returns a `Vec<SubMsg>` — each per-user
    // mint is wrapped in `reply_always` so a single failing recipient
    // can no longer revert the batch. Failures land in `FAILED_MINTS`
    // via the contract's reply handler and are claimable later through
    // `ClaimFailedDistribution`.
    let (mint_submsgs, processed_count): (Vec<SubMsg>, u32) =
        process_distribution_batch(deps.storage, &pool_info, &env)?;

    // Bounty paid by the factory from its own reserve, not pool LP funds.
    // Only emit the PayDistributionBounty message when this call actually
    // processed at least one committer. An empty/no-op call (cursor past
    // end, stale-state cleanup) must not earn a bounty: it would let a
    // keeper farm the factory reserve for zero work, and the factory's
    // bounty cap doesn't gate frequency the way the oracle cooldown does.
    let mut msgs: Vec<CosmosMsg> = Vec::new();
    if processed_count > 0 {
        // Factory rejects unregistered pools, which reverts this whole tx —
        // desired behavior since only legitimate pools should pay bounties.
        msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.factory_addr.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::FactoryExecuteMsg::PayDistributionBounty {
                    recipient: info.sender.to_string(),
                },
            )?,
            funds: vec![],
        }));
    }

    // process_distribution_batch may have either removed the state
    // entirely (genuine completion) or flipped is_distributing=false
    // (recovery path after repeated failures). Treat both as "stop
    // calling this pool" from the keeper's perspective.
    let (remaining_after, is_complete) = match DISTRIBUTION_STATE.may_load(deps.storage)? {
        None => (0u32, true),
        Some(d) => (d.distributions_remaining, !d.is_distributing),
    };

    Ok(Response::new()
        .add_submessages(mint_submsgs)
        .add_messages(msgs)
        .add_attribute("action", "continue_distribution")
        .add_attribute("caller", info.sender.to_string())
        .add_attribute("processed_count", processed_count.to_string())
        .add_attribute("bounty_paid", (processed_count > 0).to_string())
        .add_attribute(
            "remaining_before",
            dist_state.distributions_remaining.to_string(),
        )
        .add_attribute("remaining_after", remaining_after.to_string())
        .add_attribute("distribution_complete", is_complete.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}
