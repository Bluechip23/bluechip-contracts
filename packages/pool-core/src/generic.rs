//! Shared primitives used by liquidity (rate-limit + transaction deadline).
//!
//! Step 3a lands only what `pool_core::liquidity` imports.
//! Step 3b moves the rest of `pool/src/generic_helpers.rs`
//! (`update_pool_fee_growth`, `decimal2decimal256`,
//! `get_bank_transfer_to_msg`) here and deletes the creator-pool copies.
//! Commit-phase helpers (threshold payout, distribution batching,
//! commit-info updates, `mint_tokens`) stay in the creator-pool crate.

use crate::error::ContractError;
use crate::state::{PoolSpecs, USER_LAST_COMMIT};
use cosmwasm_std::{Addr, DepsMut, Env, Timestamp};

pub fn check_rate_limit(
    deps: &mut DepsMut,
    env: &Env,
    pool_specs: &PoolSpecs,
    sender: &Addr,
) -> Result<(), ContractError> {
    if let Some(last_commit_time) = USER_LAST_COMMIT.may_load(deps.storage, sender)? {
        let time_since_last = env.block.time.seconds().saturating_sub(last_commit_time);

        if time_since_last < pool_specs.min_commit_interval {
            let wait_time = pool_specs
                .min_commit_interval
                .saturating_sub(time_since_last);
            return Err(ContractError::TooFrequentCommits { wait_time });
        }
    }

    USER_LAST_COMMIT.save(deps.storage, sender, &env.block.time.seconds())?;

    Ok(())
}

pub fn enforce_transaction_deadline(
    current: Timestamp,
    transaction_deadline: Option<Timestamp>,
) -> Result<(), ContractError> {
    if let Some(dl) = transaction_deadline {
        if current > dl {
            return Err(ContractError::TransactionExpired {});
        }
    }
    Ok(())
}
