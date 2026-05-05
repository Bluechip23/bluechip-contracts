//! Deposit balance-verification reply handler.
//!
//! Standard-pool's `reply` entry point dispatches `DEPOSIT_VERIFY_REPLY_ID`
//! into [`handle_deposit_verify_reply`]. The verify-aware deposit /
//! add-to-position paths in `crate::liquidity` snapshot the pool's
//! pre-balance for every CW20 side and emit the final outgoing message
//! as a `SubMsg::reply_on_success` with this id; here we re-query each
//! CW20 side, confirm the post-balance delta matches the credited
//! amount, and either clear the transient context (success) or return
//! an `Err` (rolling the entire tx back).
//!
//! Errors propagate as `ContractError::Std`. Standard-pool wraps the
//! reply result in `StdResult<Response>`, so a Err here is converted
//! back into the chain-level failure that triggers full state rollback.
//!
//! Why a strict equality check (delta == expected) rather than `>=`:
//!   - Inflation: a CW20 that mints to the pool mid-transfer would
//!     produce delta > expected. Crediting the full delta would let an
//!     attacker grow the pool's reserve without paying for it. Rejecting
//!     it forces the deposit to revert; the attacker can re-deposit
//!     with the inflated amount intentionally.
//!   - Shortfall: fee-on-transfer / negative-rebase. delta < expected.
//!     Crediting the requested amount would let the pool's reserve
//!     accounting drift above its actual balance — a swap-then-drain
//!     vector. Reject and revert.
//!
//! Both directions reduce to a clean equality assert.

use cosmwasm_std::{DepsMut, Env, Reply, Response, StdError, SubMsgResult};
use pool_factory_interfaces::asset::query_token_balance_strict;

use crate::error::ContractError;
use crate::state::{DEPOSIT_VERIFY_CTX, DEPOSIT_VERIFY_REPLY_ID};

/// Public reply handler. Match this id in your contract's `reply`
/// entry point and forward the `Reply` here.
pub fn handle_deposit_verify_reply(
    deps: DepsMut,
    _env: Env,
    msg: Reply,
) -> Result<Response, ContractError> {
    debug_assert_eq!(msg.id, DEPOSIT_VERIFY_REPLY_ID);

    // We only register `reply_on_success`, so a SubMsgResult::Err here is
    // unreachable in normal flow — but if a future call site adds an
    // ALWAYS reply or similar, fail closed.
    if let SubMsgResult::Err(e) = msg.result {
        DEPOSIT_VERIFY_CTX.remove(deps.storage);
        return Err(ContractError::Std(StdError::generic_err(format!(
            "deposit submessage failed before balance verification could run: {}",
            e
        ))));
    }

    // Load + immediately remove the context so a future deposit can never
    // see a stale snapshot.
    let ctx = DEPOSIT_VERIFY_CTX
        .may_load(deps.storage)?
        .ok_or_else(|| {
            ContractError::Std(StdError::generic_err(
                "deposit verify reply fired without a saved context — \
                 indicates the parent handler returned without saving \
                 DEPOSIT_VERIFY_CTX before emitting the reply_on_success \
                 SubMsg, or a stale reply id collided",
            ))
        })?;
    DEPOSIT_VERIFY_CTX.remove(deps.storage);

    // For each CW20 side: query post-balance, assert delta equals expected.
    if let Some(cw20_addr) = &ctx.cw20_side0_addr {
        let post = query_token_balance_strict(&deps.querier, cw20_addr, &ctx.pool_addr)?;
        let delta = post.checked_sub(ctx.pre_balance0).map_err(|_| {
            ContractError::Std(StdError::generic_err(format!(
                "side-0 CW20 ({}) post-balance {} is BELOW pre-balance {} — \
                 the token contract must have transferred OUT of the pool during \
                 the deposit, which contradicts a normal TransferFrom flow",
                cw20_addr, post, ctx.pre_balance0
            )))
        })?;
        if delta != ctx.expected_delta0 {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "side-0 CW20 ({}) balance delta {} does not match the \
                 credited deposit amount {}. Likely cause: fee-on-transfer or \
                 rebasing CW20. Pool reserves were about to drift away from the \
                 contract's actual balance; transaction reverted to keep them \
                 consistent.",
                cw20_addr, delta, ctx.expected_delta0
            ))));
        }
    }
    if let Some(cw20_addr) = &ctx.cw20_side1_addr {
        let post = query_token_balance_strict(&deps.querier, cw20_addr, &ctx.pool_addr)?;
        let delta = post.checked_sub(ctx.pre_balance1).map_err(|_| {
            ContractError::Std(StdError::generic_err(format!(
                "side-1 CW20 ({}) post-balance {} is BELOW pre-balance {}",
                cw20_addr, post, ctx.pre_balance1
            )))
        })?;
        if delta != ctx.expected_delta1 {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "side-1 CW20 ({}) balance delta {} does not match the \
                 credited deposit amount {}. Likely cause: fee-on-transfer or \
                 rebasing CW20. Transaction reverted to keep pool reserves \
                 consistent with on-chain balances.",
                cw20_addr, delta, ctx.expected_delta1
            ))));
        }
    }

    Ok(Response::new()
        .add_attribute("action", "deposit_balance_verified")
        .add_attribute("pool_contract", ctx.pool_addr.to_string()))
}
