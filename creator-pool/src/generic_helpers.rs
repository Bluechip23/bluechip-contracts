//! Re-export shim for shared primitives + the genuinely-generic
//! per-commit `update_commit_info`.
//!
//! Shared primitives (`check_rate_limit`, `enforce_transaction_deadline`,
//! `update_pool_fee_growth`, `decimal2decimal256`,
//! `get_bank_transfer_to_msg`) live in `pool_core::generic` and are
//! re-exported below so every existing `use crate::generic_helpers::X;`
//! import resolves unchanged.
//!
//! Threshold-payout orchestration was hoisted to
//! [`crate::commit::threshold_payout`]; the post-threshold batch
//! processor was hoisted to [`crate::commit::distribution_batch`].
//! Re-exports here keep both reachable through the original path so
//! existing call sites (the threshold-crossing handler, the
//! distribution dispatcher) compile unchanged.

pub use pool_core::generic::*;

pub use crate::commit::distribution_batch::{
    calculate_effective_batch_size, process_distribution_batch,
};
pub use crate::commit::threshold_payout::{
    mint_tokens, trigger_threshold_payout, validate_pool_threshold_payments, ThresholdPayoutMsgs,
};

use crate::error::ContractError;
use crate::state::{Committing, COMMIT_INFO, REENTRANCY_LOCK};
use cosmwasm_std::{Addr, DepsMut, Storage, Timestamp, Uint128};

/// Run `body` under the contract-wide `REENTRANCY_LOCK`.
///
/// Centralizes the load → check → save(true) → run → save(false)
/// pattern previously open-coded in three places (`commit::commit`,
/// `liquidity_helpers::execute_claim_creator_fees`,
/// `liquidity_helpers::execute_claim_creator_excess`).
///
/// The guard is cleared **unconditionally** before returning, on both
/// success and error paths. Production CosmWasm reverts every staged
/// storage write when a handler returns `Err`, so the explicit
/// `save(false)` on the error path is redundant in production —
/// but mock test environments (`mock_dependencies`) do **not** revert,
/// and a follow-up call in the same `#[test]` would otherwise see
/// a stuck `REENTRANCY_LOCK = true` from the prior failed attempt.
/// Always clearing keeps the helper safe under both runtimes.
pub fn with_reentrancy_guard<F, T>(
    mut deps: DepsMut,
    body: F,
) -> Result<T, ContractError>
where
    F: FnOnce(DepsMut) -> Result<T, ContractError>,
{
    if REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;
    let result = body(deps.branch());
    // Unconditional clear. See doc-comment above for why this matters
    // for the test mock-storage path even though production tx
    // atomicity makes it redundant on the error branch.
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
}

pub fn update_commit_info(
    storage: &mut dyn Storage,
    sender: &Addr,
    pool_contract_address: &Addr,
    bluechip_amount: Uint128,
    usd_amount: Uint128,
    timestamp: Timestamp,
) -> Result<(), ContractError> {
    COMMIT_INFO.update(
        storage,
        sender,
        |maybe_committing| -> Result<_, ContractError> {
            match maybe_committing {
                Some(mut committing) => {
                    committing.total_paid_bluechip = committing
                        .total_paid_bluechip
                        .checked_add(bluechip_amount)?;
                    committing.total_paid_usd =
                        committing.total_paid_usd.checked_add(usd_amount)?;
                    committing.last_payment_bluechip = bluechip_amount;
                    committing.last_payment_usd = usd_amount;
                    committing.last_committed = timestamp;
                    Ok(committing)
                }
                // First-commit for this sender: clone only here, where the
                // owned Addr is actually stored. Repeat committers (the
                // common path) pass through zero Addr allocations.
                None => Ok(Committing {
                    pool_contract_address: pool_contract_address.clone(),
                    committer: sender.clone(),
                    total_paid_bluechip: bluechip_amount,
                    total_paid_usd: usd_amount,
                    last_committed: timestamp,
                    last_payment_bluechip: bluechip_amount,
                    last_payment_usd: usd_amount,
                }),
            }
        },
    )?;
    Ok(())
}
