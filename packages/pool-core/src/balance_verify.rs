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
//! Why a strict equality check (`post + outgoing == pre + actual_in`)
//! rather than `>=`:
//! - Inflation: a CW20 that mints to the pool mid-transfer would push
//! the post-balance above expected. Crediting the full delta would
//! let an attacker grow the pool's reserve without paying for it.
//! Rejecting forces the deposit to revert; the attacker can re-deposit
//! with the inflated amount intentionally.
//! - Shortfall: fee-on-transfer / negative-rebase pushes post-balance
//! below expected. Crediting the requested amount would let the pool's
//! reserve accounting drift above its actual balance — a
//! swap-then-drain vector. Reject and revert.
//!
//! The `outgoing` term accounts for CW20 outflows that ride in the same
//! Response (e.g., `add_to_position` fee payouts). Without it the
//! verify would falsely flag every legitimate add-to-position whose
//! position has prior CW20-side fee accrual — the pool's CW20-balance
//! change is `actual_in - fee_out`, not just `actual_in`. With the
//! outgoing term, the equality stays exact while letting the in-tx
//! outflow happen.

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

    // For each CW20 side: query post-balance, assert the net-flow
    // invariant
    //     post + outgoing == pre + actual_in
    // holds. Equivalently `(post - pre) == (actual_in - outgoing)` —
    // i.e., the pool's actual CW20-balance change must match the
    // declared net of inflows minus outflows. This shape preserves the
    // strict equality defense (rejects both fee-on-transfer shortfalls
    // AND mint-mid-transfer inflation) while correctly handling
    // outflows dispatched as part of the same Response — see
    // `finalize_deposit_response::outgoing_amounts` for the
    // `add_to_position` motivating case (Finding 12.1).
    //
    // Both sides of the equality are computed via `checked_add` so an
    // overflowing pre+actual (impossible at any plausible scale, but
    // defensive) errors out cleanly rather than wrapping.
    if let Some(cw20_addr) = &ctx.cw20_side0_addr {
        let post = query_token_balance_strict(&deps.querier, cw20_addr, &ctx.pool_addr)?;
        let lhs = post.checked_add(ctx.outgoing_amount0).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "side-0 CW20 ({}) post+outgoing overflow: {}",
                cw20_addr, e
            )))
        })?;
        let rhs = ctx
            .pre_balance0
            .checked_add(ctx.expected_delta0)
            .map_err(|e| {
                ContractError::Std(StdError::generic_err(format!(
                    "side-0 CW20 ({}) pre+actual overflow: {}",
                    cw20_addr, e
                )))
            })?;
        if lhs != rhs {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "side-0 CW20 ({}) net-balance invariant violated: \
                 post {} + outgoing {} != pre {} + actual_in {}. Likely cause: \
                 fee-on-transfer or rebasing CW20 (post lower than expected), \
                 or unsolicited mint to the pool during the deposit (post \
                 higher than expected). Transaction reverted to keep pool \
                 reserves consistent with on-chain balances.",
                cw20_addr, post, ctx.outgoing_amount0, ctx.pre_balance0, ctx.expected_delta0
            ))));
        }
    }
    if let Some(cw20_addr) = &ctx.cw20_side1_addr {
        let post = query_token_balance_strict(&deps.querier, cw20_addr, &ctx.pool_addr)?;
        let lhs = post.checked_add(ctx.outgoing_amount1).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "side-1 CW20 ({}) post+outgoing overflow: {}",
                cw20_addr, e
            )))
        })?;
        let rhs = ctx
            .pre_balance1
            .checked_add(ctx.expected_delta1)
            .map_err(|e| {
                ContractError::Std(StdError::generic_err(format!(
                    "side-1 CW20 ({}) pre+actual overflow: {}",
                    cw20_addr, e
                )))
            })?;
        if lhs != rhs {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "side-1 CW20 ({}) net-balance invariant violated: \
                 post {} + outgoing {} != pre {} + actual_in {}.",
                cw20_addr, post, ctx.outgoing_amount1, ctx.pre_balance1, ctx.expected_delta1
            ))));
        }
    }

    Ok(Response::new()
        .add_attribute("action", "deposit_balance_verified")
        .add_attribute("pool_contract", ctx.pool_addr.to_string()))
}
