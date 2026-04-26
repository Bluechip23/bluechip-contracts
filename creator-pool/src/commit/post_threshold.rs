//! Post-threshold commit handler. Once the pool has crossed its commit
//! threshold the commit flow becomes a plain AMM swap: the caller's
//! bluechip deposit is swapped against the creator-token reserve, the
//! reserves + fee growth are updated, and the creator tokens are sent
//! back to the caller. Commit ledger and pre-threshold USD totals are
//! NOT touched here — those belong to the funding phase.
//!
//! All four of the pool's hot-path state items (`POOL_INFO`, `POOL_SPECS`,
//! `POOL_STATE`, `POOL_FEE_STATE`) are threaded in from `execute_commit_logic`
//! via references — see `super::execute_commit_logic` for the outer load.

use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, Response, Uint128, WasmMsg,
};
use cw20::Cw20ExecuteMsg;

use crate::asset::TokenInfo;
use crate::error::ContractError;
use crate::generic_helpers::{update_commit_info, update_pool_fee_growth};
use crate::state::{
    PoolAnalytics, PoolFeeState, PoolInfo, PoolSpecs, PoolState, POOL_FEE_STATE, POOL_PAUSED,
    POOL_STATE, POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK,
};
use crate::swap_helper::{assert_max_spread, compute_swap, update_price_accumulator};

use super::commit_base_attributes;

#[allow(clippy::too_many_arguments)]
pub(super) fn process_post_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: TokenInfo,
    swap_amount: Uint128,
    usd_value: Uint128,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    pool_info: &PoolInfo,
    pool_specs: &PoolSpecs,
    pool_state: &mut PoolState,
    pool_fee_state: &mut PoolFeeState,
    analytics: &mut PoolAnalytics,
) -> Result<Response, ContractError> {
    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }

    // Post-threshold-crossing cooldown. Mirrors the gate in
    // pool_core::swap::execute_simple_swap: a follower commit landing in
    // the crossing block (after the crosser's tx) or the next N blocks
    // is rejected so it can't atomically sandwich the freshly-seeded
    // pool. The crosser's own bounded excess swap runs before any other
    // tx ever observes this storage item, so it isn't gated by itself.
    let cooldown_until = POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK
        .may_load(deps.storage)?
        .unwrap_or(0);
    if env.block.height < cooldown_until {
        return Err(ContractError::PostThresholdCooldownActive {
            until_block: cooldown_until,
        });
    }

    let offer_pool = pool_state.reserve0;
    let ask_pool = pool_state.reserve1;

    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, swap_amount, pool_specs.lp_fee)?;

    // Dust-swap guard: mirror simple_swap's zero-return rejection so a
    // post-threshold commit that would consume the user's bluechip
    // without yielding any creator tokens fails loudly.
    if return_amt.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    // `assert_max_spread` measures `expected_return - return_amount` against
    // `max_spread`. Pass the gross-of-commission return so the spread check
    // matches the convention used by `pool_core::swap::execute_simple_swap`
    // and `process_threshold_crossing_with_excess`. The previous net-of-
    // commission argument made this path stricter than the equivalent
    // SimpleSwap, causing identical-params commits to revert where swaps
    // succeeded.
    assert_max_spread(
        belief_price,
        max_spread,
        swap_amount,
        return_amt.checked_add(commission_amt)?,
        spread_amt,
    )?;

    update_price_accumulator(pool_state, env.block.time.seconds())?;

    pool_state.reserve0 = offer_pool.checked_add(swap_amount)?;
    pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

    update_pool_fee_growth(pool_fee_state, pool_state, 0, commission_amt)?;
    POOL_FEE_STATE.save(deps.storage, &*pool_fee_state)?;
    POOL_STATE.save(deps.storage, &*pool_state)?;

    if !return_amt.is_zero() {
        messages.push(
            WasmMsg::Execute {
                contract_addr: pool_info.token_address.to_string(),
                msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                    recipient: sender.to_string(),
                    amount: return_amt,
                })?,
                funds: vec![],
            }
            .into(),
        );
    }

    update_commit_info(
        deps.storage,
        &sender,
        &pool_state.pool_contract_address,
        asset.amount,
        usd_value,
        env.block.time,
    )?;

    // Update analytics — `total_commit_count` is incremented and persisted
    // by the dispatcher (`commit::execute_commit_logic`); this handler
    // only mutates the swap-specific fields on the shared `&mut analytics`.
    analytics.total_swap_count += 1;
    analytics.total_volume_0 = analytics.total_volume_0.saturating_add(swap_amount);
    analytics.total_volume_1 = analytics.total_volume_1.saturating_add(return_amt);
    analytics.last_trade_block = env.block.height;
    analytics.last_trade_timestamp = env.block.time.seconds();

    // Effective price: creator tokens received per bluechip spent
    let effective_price = if !swap_amount.is_zero() {
        Decimal::from_ratio(return_amt, swap_amount).to_string()
    } else {
        "0".to_string()
    };

    let base = commit_base_attributes(
        "active",
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
        .add_attribute("swap_amount_bluechip", swap_amount.to_string())
        .add_attribute("tokens_received", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount", commission_amt.to_string())
        .add_attribute("effective_price", effective_price)
        .add_attribute("reserve0_after", pool_state.reserve0.to_string())
        .add_attribute("reserve1_after", pool_state.reserve1.to_string()))
}
