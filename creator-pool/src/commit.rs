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
pub mod post_threshold;
pub mod pre_threshold;
pub mod threshold_crossing;

pub use distribution::execute_continue_distribution;

use cosmwasm_std::{
    Addr, CosmosMsg, Decimal, DepsMut, Env, Fraction, MessageInfo, Response, StdError, Timestamp,
    Uint128,
};

use crate::admin::ensure_not_drained;
use crate::asset::{get_native_denom, TokenInfo, TokenType};
use crate::error::ContractError;
use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, get_bank_transfer_to_msg,
    trigger_threshold_payout, update_commit_info,
};
use crate::msg::CommitFeeInfo;
use crate::state::{
    COMMITFEEINFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO, IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT,
    NATIVE_RAISED_FROM_COMMIT, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE,
    POST_THRESHOLD_COOLDOWN_BLOCKS, POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK, REENTRANCY_LOCK,
    THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::swap_helper::get_oracle_conversion_with_staleness;

use post_threshold::process_post_threshold_commit;
use pre_threshold::process_pre_threshold_commit;
use threshold_crossing::process_threshold_crossing_with_excess;

// Minimum commit value in USD (6 decimals), applied ONLY to pre-threshold
// commits. $5 = 5_000_000. The floor limits pre-threshold ledger bloat
// (an attacker can still cross the $25k threshold with their own money, but
// they can't do it with 25,000 individual $1 entries that balloon the
// distribution queue). Post-threshold commits are functionally AMM swaps —
// they don't add to COMMIT_LEDGER and don't feed distribution — so we keep
// the floor at $1 for them to preserve UX for small-trade users.
pub const MIN_COMMIT_USD_PRE_THRESHOLD: Uint128 = Uint128::new(5_000_000);
pub const MIN_COMMIT_USD_POST_THRESHOLD: Uint128 = Uint128::new(1_000_000);

/// Base attribute set shared by every commit response (pre-threshold,
/// post-threshold, threshold_hit_exact, threshold_crossing). Each caller
/// adds its path-specific attributes on top.
pub(crate) fn commit_base_attributes(
    phase: &'static str,
    sender: &Addr,
    pool_contract: &Addr,
    total_commit_count: u64,
    env: &Env,
) -> Vec<cosmwasm_std::Attribute> {
    vec![
        cosmwasm_std::Attribute::new("action", "commit"),
        cosmwasm_std::Attribute::new("phase", phase),
        cosmwasm_std::Attribute::new("committer", sender.as_str()),
        cosmwasm_std::Attribute::new("total_commit_count", total_commit_count.to_string()),
        cosmwasm_std::Attribute::new("pool_contract", pool_contract.as_str()),
        cosmwasm_std::Attribute::new("block_height", env.block.height.to_string()),
        cosmwasm_std::Attribute::new("block_time", env.block.time.seconds().to_string()),
    ]
}

pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    transaction_deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    ensure_not_drained(deps.storage)?;
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    // Reentrancy protection
    let reentrancy_guard = REENTRANCY_LOCK.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_LOCK.save(deps.storage, &true)?;

    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        REENTRANCY_LOCK.save(deps.storage, &false)?;
        return Err(e);
    }

    let result = execute_commit_logic(
        &mut deps,
        env,
        info,
        asset,
        belief_price,
        max_spread,
    );
    REENTRANCY_LOCK.save(deps.storage, &false)?;
    result
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

    if !asset.info.equal(&pool_info.pool_info.asset_infos[0])
        && !asset.info.equal(&pool_info.pool_info.asset_infos[1])
    {
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
        MIN_COMMIT_USD_POST_THRESHOLD
    } else {
        MIN_COMMIT_USD_PRE_THRESHOLD
    };
    if usd_value < min_commit {
        let (phase, dollars) = if threshold_already_hit {
            ("post-threshold", "1")
        } else {
            ("pre-threshold", "5")
        };
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Commit too small: ${} USD (minimum ${} USD {})",
            usd_value, dollars, phase
        ))));
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
                ContractError::Std(StdError::generic_err(format!(
                    "Invalid commit funds: {}. Commit must attach exactly the bluechip \
                     denom — additional denoms (e.g., gas tokens, IBC assets) would be \
                     stranded in the pool with no withdrawal path.",
                    e
                )))
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

            let mut messages = build_fee_messages(
                &fee_info,
                denom,
                commit_fee_bluechip_amt,
                commit_fee_creator_amt,
            )?;

            // Load `POOL_ANALYTICS` once for this dispatch path; the
            // `total_commit_count` bump is universal to every commit
            // branch below, so we increment here and let each handler
            // mutate swap-specific fields on the shared `&mut analytics`.
            // The single save at the bottom of the Native arm subsumes
            // what was previously a load+save inside each handler.
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
                        return Err(ContractError::Std(StdError::generic_err(
                            "THRESHOLD_PROCESSING is stuck = true; should be \
                             unreachable in normal operation. Use the factory's \
                             RecoverPoolStuckStates with StuckThreshold to \
                             clear it (waits 1 hour from LAST_THRESHOLD_ATTEMPT), \
                             then retry the commit.",
                        )));
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
                        // Threshold hit exactly
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default().checked_add(usd_value)?)
                        })?;
                        let final_usd =
                            new_total.min(commit_config.commit_amount_for_threshold_usd);
                        USD_RAISED_FROM_COMMIT.save(deps.storage, &final_usd)?;
                        NATIVE_RAISED_FROM_COMMIT
                            .update::<_, ContractError>(deps.storage, |r| {
                                Ok(r.checked_add(asset.amount)?)
                            })?;
                        IS_THRESHOLD_HIT.save(deps.storage, &true)?;
                        // Arm the post-threshold cooldown so other actors
                        // can't atomically sandwich the freshly-seeded pool
                        // in the same block (or the next two). Crossing tx
                        // itself is unaffected — the writes here land
                        // before the next tx ever runs the cooldown check.
                        POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK.save(
                            deps.storage,
                            &(env.block.height + POST_THRESHOLD_COOLDOWN_BLOCKS + 1),
                        )?;

                        let payout = trigger_threshold_payout(
                            deps.storage,
                            &pool_info,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            &env,
                        )?;
                        messages.extend(payout.other_msgs);
                        update_commit_info(
                            deps.storage,
                            &sender,
                            &pool_state.pool_contract_address,
                            asset.amount,
                            usd_value,
                            env.block.time,
                        )?;
                        THRESHOLD_PROCESSING.save(deps.storage, &false)?;

                        // Analytics counter is incremented and persisted by
                        // the dispatcher (see the `analytics` binding above
                        // and the `POOL_ANALYTICS.save` below the cascade);
                        // this branch only needs to read the already-bumped
                        // `total_commit_count` for response attributes.

                        // `payout.factory_notify` is attached as a SubMsg so a
                        // factory-side failure lands in the pool's reply handler
                        // rather than reverting the commit.
                        let base = commit_base_attributes(
                            "threshold_hit_exact",
                            &sender,
                            &pool_state.pool_contract_address,
                            analytics.total_commit_count,
                            &env,
                        );
                        Response::new()
                            .add_submessage(payout.factory_notify)
                            .add_messages(messages)
                            .add_attributes(base)
                            .add_attribute("commit_amount_bluechip", asset.amount.to_string())
                            .add_attribute("commit_amount_usd", usd_value.to_string())
                            .add_attribute("total_usd_raised_after", new_total.to_string())
                    }
                } else {
                    process_pre_threshold_commit(
                        deps,
                        env,
                        sender,
                        &asset,
                        usd_value,
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

            // Single analytics save subsumes what each handler used to do
            // individually. If anything above returned `Err`, the whole
            // tx aborts (CosmWasm storage is transactional), so this save
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
