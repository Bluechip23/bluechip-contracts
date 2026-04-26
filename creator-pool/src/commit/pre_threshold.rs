//! Pre-threshold commit handler.
//!
//! Runs while the pool is still accumulating USD towards its
//! `commit_amount_for_threshold_usd` target. Each call appends to
//! `COMMIT_LEDGER` for the sender and bumps the cumulative
//! `USD_RAISED_FROM_COMMIT` / `NATIVE_RAISED_FROM_COMMIT` totals.
//! Threshold-crossing commits are routed elsewhere — see
//! `super::threshold_crossing` and `super::execute_commit_logic`.

use cosmwasm_std::{Addr, CosmosMsg, DepsMut, Env, Response, Uint128};

use crate::asset::TokenInfo;
use crate::error::ContractError;
use crate::generic_helpers::update_commit_info;
use crate::state::{
    PoolAnalytics, PoolState, COMMIT_LEDGER, NATIVE_RAISED_FROM_COMMIT, USD_RAISED_FROM_COMMIT,
};

use super::commit_base_attributes;

#[allow(clippy::too_many_arguments)]
pub(super) fn process_pre_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &TokenInfo,
    usd_value: Uint128,
    messages: Vec<CosmosMsg>,
    pool_state: &PoolState,
    analytics: &mut PoolAnalytics,
) -> Result<Response, ContractError> {
    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default().checked_add(usd_value)?)
    })?;
    // Capture the update return values so we don't re-read USD_RAISED /
    // NATIVE_RAISED after the writes. `Item::update` returns the new value.
    let total_usd_raised = USD_RAISED_FROM_COMMIT
        .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(usd_value)?))?;
    let total_bluechip_raised = NATIVE_RAISED_FROM_COMMIT
        .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(asset.amount)?))?;

    update_commit_info(
        deps.storage,
        &sender,
        &pool_state.pool_contract_address,
        asset.amount,
        usd_value,
        env.block.time,
    )?;

    // Analytics counter is incremented and persisted by the dispatcher
    // (`commit::execute_commit_logic`); this handler only reads the
    // already-bumped `total_commit_count` for response attributes.
    let base = commit_base_attributes(
        "funding",
        &sender,
        &pool_state.pool_contract_address,
        analytics.total_commit_count,
        &env,
    );
    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(base)
        .add_attribute("commit_amount_bluechip", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string())
        .add_attribute("total_usd_raised_after", total_usd_raised.to_string())
        .add_attribute(
            "total_bluechip_raised_after",
            total_bluechip_raised.to_string(),
        ))
}
