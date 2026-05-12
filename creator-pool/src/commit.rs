//! Commit entry point + dispatcher, plus shared per-commit helpers
//! (fee split, fee-message builder, response-attribute base).
//!
//! The four handler bodies — pre-threshold funding, post-threshold AMM
//! swap, threshold-crossing split, and distribution batch processing —
//! live in submodules:
//!   - [`pre_threshold`]       — commits while the pool is still funding
//!   - [`post_threshold`]      — commits after the pool is fully funded
//!   - [`threshold_crossing`]  — the commit that carries the pool across
//!   - [`distribution`]        — post-threshold keeper-driven payout batches
//!
//! This file keeps:
//!   - `commit` / `execute_commit_logic` — the entry point + dispatcher
//!   - `commit_base_attributes`          — shared by all four response paths
//!   - `calculate_commit_fees` / `build_fee_messages`
//!   - `MIN_COMMIT_USD_*` constants
//!
//! and re-exports `execute_continue_distribution` so the pool's entry
//! points don't need to know about the submodule structure.

pub mod distribution;
pub mod distribution_batch;
pub mod post_threshold;
pub mod pre_threshold;
pub mod threshold_crossing;
pub mod threshold_payout;

pub use distribution::execute_continue_distribution;

use cosmwasm_std::{
    Addr, CosmosMsg, Decimal, DepsMut, Env, Fraction, MessageInfo, Response, Timestamp, Uint128,
};

use crate::admin::ensure_not_drained;
use crate::asset::{get_native_denom, TokenInfo, TokenType};
use crate::error::ContractError;
use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, get_bank_transfer_to_msg,
    with_reentrancy_guard,
};
use crate::msg::CommitFeeInfo;
use crate::state::{
    COMMITFEEINFO, COMMIT_LIMIT_INFO, IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE, THRESHOLD_PAYOUT_AMOUNTS,
    THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::swap_helper::get_oracle_conversion_with_staleness;

use post_threshold::process_post_threshold_commit;
use pre_threshold::process_pre_threshold_commit;
use threshold_crossing::{process_threshold_crossing_with_excess, process_threshold_hit_exact};

// Minimum commit-value floors moved to per-pool state. Defaults are
// `crate::state::DEFAULT_MIN_COMMIT_USD_{PRE,POST}_THRESHOLD` and the
// active values are stored on `CommitLimitInfo.min_commit_usd_pre_threshold`
// / `min_commit_usd_post_threshold`. The floor still limits pre-threshold
// ledger bloat (an attacker can cross the threshold with their own
// money, but not via thousands of micro-entries that balloon the
// distribution queue); post-threshold commits stay looser since they're
// AMM swaps that don't touch COMMIT_LEDGER.

/// Base attribute set shared by every commit response (pre-threshold,
/// post-threshold, threshold_hit_exact, threshold_crossing). Each caller
/// adds its path-specific attributes on top via `Response::add_attributes`.
///
/// Returned as `Vec<(&str, String)>` for consistency with the
/// tuple-vec form used elsewhere in this crate (admin response
/// builders, liquidity_helpers claim handlers). `Response::add_attributes`
/// accepts any `IntoIterator<Item = impl Into<Attribute>>` so the
/// consuming sites are unchanged.
pub(crate) fn commit_base_attributes(
    phase: &'static str,
    sender: &Addr,
    pool_contract: &Addr,
    total_commit_count: u64,
    env: &Env,
) -> Vec<(&'static str, String)> {
    vec![
        ("action", "commit".to_string()),
        ("phase", phase.to_string()),
        ("committer", sender.to_string()),
        ("total_commit_count", total_commit_count.to_string()),
        ("pool_contract", pool_contract.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
    ]
}

pub fn commit(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    transaction_deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    ensure_not_drained(deps.storage)?;
    // M-4.1 audit fix: admin (or auto-low-liquidity) pause halts ALL
    // commit branches, not just the post-threshold AMM-swap path.
    // POOL_PAUSED is true whenever the pool is paused for any reason
    // (admin Pause, emergency-withdraw Phase 1, or auto-pause from
    // reserves dipping below MINIMUM_LIQUIDITY); POOL_PAUSED_AUTO is
    // a discriminator that doesn't matter at the commit gate. Without
    // this check, a paused pool would continue to bank pre-threshold
    // funds and to cross the threshold while admin investigates —
    // a fire-alarm-with-foot-still-on-the-gas failure mode. The
    // existing redundant check in `process_post_threshold_commit`
    // is kept as defense-in-depth. Reuses the existing
    // `PoolPausedLowLiquidity` error variant for consistency with
    // the swap and post-threshold callers; the name is a residual
    // from when the only pause path was the auto-low-liquidity one.
    if POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    with_reentrancy_guard(deps, |mut deps| {
        let pool_specs = POOL_SPECS.load(deps.storage)?;
        let sender = info.sender.clone();
        check_rate_limit(&mut deps, &env, &pool_specs, &sender)?;
        execute_commit_logic(&mut deps, env, info, asset, belief_price, max_spread)
    })
}

fn execute_commit_logic(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let amount = asset.amount;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let threshold_payout = THRESHOLD_PAYOUT_AMOUNTS.load(deps.storage)?;
    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let sender = info.sender.clone();

    // M-4.3 audit fix: commits flow only in the bluechip direction.
    // `validate_pool_token_info` pins `asset_infos[0]` to the canonical
    // bluechip Native denom and `asset_infos[1]` to the creator-token
    // CW20, so accepting the creator-token side here was dead-code —
    // the inner `match` below only handles bluechip Native and returns
    // `AssetMismatch` for everything else. Tighten the outer check to
    // bluechip-only so a caller passing the creator-token side surfaces
    // the clearer error earlier and skips the oracle-conversion +
    // min-commit + analytics work that would otherwise run before the
    // inner reject. The inner `_ => AssetMismatch` arm is preserved as
    // defense-in-depth against config corruption.
    if !asset.info.equal(&pool_info.pool_info.asset_infos[0]) {
        return Err(ContractError::AssetMismatch {});
    }
    if asset.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    // Snapshot the oracle rate once at commit entry and thread it through
    // every conversion that happens during this handler. Prevents
    // mid-tx drift where the USD valuation at the top of the handler could
    // disagree with the bluechip_to_threshold conversion computed later in
    // process_threshold_crossing_with_excess. No current path allows
    // drift within a single tx — the factory's cached price doesn't change
    // during a commit — but threading one rate explicitly makes the
    // invariant load-bearing rather than incidental.
    let oracle_snapshot =
        get_oracle_conversion_with_staleness(deps.as_ref(), asset.amount, env.block.time.seconds())?;
    let usd_value = oracle_snapshot.amount;
    let oracle_rate = oracle_snapshot.rate_used;
    if oracle_rate.is_zero() {
        return Err(ContractError::InvalidOraclePrice {});
    }
    if usd_value.is_zero() {
        return Err(ContractError::InvalidOraclePrice {});
    }
    // Load IS_THRESHOLD_HIT once and thread it through both the minimum-
    // commit check here and the main branching below (used later as
    // `threshold_already_hit`). Previously the load was duplicated.
    let threshold_already_hit = IS_THRESHOLD_HIT.load(deps.storage)?;
    let min_commit = if threshold_already_hit {
        commit_config.min_commit_usd_post_threshold
    } else {
        commit_config.min_commit_usd_pre_threshold
    };
    if usd_value < min_commit {
        let phase: &'static str = if threshold_already_hit {
            "post-threshold"
        } else {
            "pre-threshold"
        };
        return Err(ContractError::CommitTooSmall {
            got: usd_value,
            min: min_commit,
            phase,
        });
    }

    let bluechip_denom = get_native_denom(&pool_info.pool_info.asset_infos)?;

    match &asset.info {
        TokenType::Native { denom } if denom == &bluechip_denom => {
            // Strict exact-match on attached funds via `cw_utils::must_pay`.
            //
            // `must_pay` enforces:
            //   1. Funds list must be exactly one coin (rejects multi-denom).
            //      An attacker (or careless frontend) attaching
            //      `[ubluechip: amount, ibc/...: Y]` would otherwise have the
            //      IBC denom silently absorbed into the pool's bank balance
            //      with no recovery path.
            //   2. Coin amount must be non-zero.
            //   3. Coin denom must match the canonical bluechip denom.
            //
            // The post-condition `sent == amount` then catches under/
            // overpayment in the bluechip side, preserving the
            // exact-amount semantics that `simple_swap` already enforces
            // via `confirm_sent_native_balance` (which delegates to
            // must_pay too).
            let sent = cw_utils::must_pay(&info, denom.as_str()).map_err(|e| {
                ContractError::InvalidCommitFunds {
                    reason: e.to_string(),
                }
            })?;
            if sent != amount {
                return Err(ContractError::MismatchAmount {});
            }

            let (commit_fee_bluechip_amt, commit_fee_creator_amt) =
                calculate_commit_fees(amount, &fee_info)?;
            let total_fees = commit_fee_bluechip_amt.checked_add(commit_fee_creator_amt)?;
            if total_fees >= amount {
                return Err(ContractError::InvalidFee {});
            }
            let amount_after_fees = amount.checked_sub(total_fees)?;
            if amount_after_fees.is_zero() {
                return Err(ContractError::InvalidFee {});
            }

            let messages = build_fee_messages(
                &fee_info,
                denom,
                commit_fee_bluechip_amt,
                commit_fee_creator_amt,
            )?;

            // Load `POOL_ANALYTICS` once for this dispatch path; the
            // `total_commit_count` bump is universal to every commit
            // branch below, so we increment here and let each handler
            // mutate swap-specific fields on the shared `&mut analytics`.
            // A single save at the bottom of the Native arm persists the
            // result for all four phase handlers.
            let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
            analytics.total_commit_count += 1;

            // `threshold_already_hit` was loaded above alongside the
            // minimum-commit check — reuse it here instead of re-reading.
            let response = if !threshold_already_hit {
                let current_usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
                let new_total = current_usd_raised.checked_add(usd_value)?;

                if new_total >= commit_config.commit_amount_for_threshold_usd {
                    LAST_THRESHOLD_ATTEMPT.save(deps.storage, &env.block.time)?;

                    // THRESHOLD_PROCESSING is set to `true` immediately
                    // below, then cleared at the end of the threshold-
                    // crossing path (excess or exact-hit branch). If the
                    // crossing handler errors, the entire tx reverts —
                    // including this `save(true)` — so the storage
                    // reverts to whatever it was before this tx (which
                    // was `false`). REENTRANCY_LOCK separately blocks
                    // any in-tx reentry. Net: under normal operation,
                    // `THRESHOLD_PROCESSING == true` at this point is
                    // structurally unreachable.
                    //
                    // The only way to observe a stuck `true` is genuine
                    // storage corruption (unrecoverable bug) or an
                    // interrupted prior tx that somehow committed without
                    // clearing the flag (would also indicate a bug).
                    // Rather than silently downgrading the user's intended
                    // threshold-crossing commit into a pre/post-threshold
                    // commit (the prior fallback behavior, which violated
                    // user intent and hid the underlying corruption),
                    // surface the stuck state with an explicit error
                    // pointing operators at the recovery path.
                    if THRESHOLD_PROCESSING
                        .may_load(deps.storage)?
                        .unwrap_or(false)
                    {
                        return Err(ContractError::StuckThresholdProcessing);
                    }
                    THRESHOLD_PROCESSING.save(deps.storage, &true)?;

                    let usd_to_threshold = commit_config
                        .commit_amount_for_threshold_usd
                        .checked_sub(current_usd_raised)
                        .unwrap_or(Uint128::zero());

                    if usd_value > usd_to_threshold && usd_to_threshold > Uint128::zero() {
                        // Split commit: part goes to threshold, excess becomes swap
                        process_threshold_crossing_with_excess(
                            deps,
                            env,
                            sender,
                            &asset,
                            amount,
                            amount_after_fees,
                            usd_value,
                            usd_to_threshold,
                            oracle_rate,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &pool_specs,
                            &pool_info,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            messages,
                            belief_price,
                            max_spread,
                            &mut analytics,
                        )?
                    } else {
                        // Threshold hit exactly — extracted to
                        // `commit::threshold_crossing::process_threshold_hit_exact`
                        // so all four phase handlers sit at the same module
                        // depth (pre / post / threshold-with-excess /
                        // threshold-hit-exact / distribution batch).
                        process_threshold_hit_exact(
                            deps,
                            env,
                            sender,
                            &asset,
                            amount_after_fees,
                            usd_value,
                            new_total,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &pool_info,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            messages,
                            &analytics,
                        )?
                    }
                } else {
                    process_pre_threshold_commit(
                        deps,
                        env,
                        sender,
                        &asset,
                        usd_value,
                        // Net-of-fees bluechip that actually enters the
                        // contract bank balance from this commit (audit
                        // fix; see pre_threshold.rs).
                        amount_after_fees,
                        messages,
                        &pool_state,
                        &mut analytics,
                    )?
                }
            } else {
                process_post_threshold_commit(
                    deps,
                    env,
                    sender,
                    asset,
                    amount_after_fees,
                    usd_value,
                    messages,
                    belief_price,
                    max_spread,
                    &pool_info,
                    &pool_specs,
                    &mut pool_state,
                    &mut pool_fee_state,
                    &mut analytics,
                )?
            };

            // Single analytics save covers every commit branch. If
            // anything above returned `Err`, the whole tx aborts
            // (CosmWasm storage is transactional), so this save
            // never persists in error paths.
            POOL_ANALYTICS.save(deps.storage, &analytics)?;
            Ok(response)
        }
        _ => Err(ContractError::AssetMismatch {}),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Calculate both fee portions for a commit. Returns (bluechip_fee, creator_fee).
fn calculate_commit_fees(
    amount: Uint128,
    fee_info: &CommitFeeInfo,
) -> Result<(Uint128, Uint128), ContractError> {
    let bluechip_fee = amount
        .checked_mul(fee_info.commit_fee_bluechip.numerator())?
        .checked_div(fee_info.commit_fee_bluechip.denominator())
        .map_err(|_| ContractError::DivideByZero)?;
    let creator_fee = amount
        .checked_mul(fee_info.commit_fee_creator.numerator())?
        .checked_div(fee_info.commit_fee_creator.denominator())
        .map_err(|_| ContractError::DivideByZero)?;
    Ok((bluechip_fee, creator_fee))
}

/// Build bank-send messages for the two fee recipients.
fn build_fee_messages(
    fee_info: &CommitFeeInfo,
    denom: &str,
    bluechip_fee: Uint128,
    creator_fee: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut messages = Vec::new();
    if !bluechip_fee.is_zero() {
        messages.push(get_bank_transfer_to_msg(
            &fee_info.bluechip_wallet_address,
            denom,
            bluechip_fee,
        )?);
    }
    if !creator_fee.is_zero() {
        messages.push(get_bank_transfer_to_msg(
            &fee_info.creator_wallet_address,
            denom,
            creator_fee,
        )?);
    }
    Ok(messages)
}
