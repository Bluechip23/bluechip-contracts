//! `RequestExpansion` handler, decomposed into one helper per phase so
//! the dispatcher stays under ~25 lines. Each helper is independently
//! unit-testable and documents one concern.
//!
//! Phase ordering (load-bearing, do not reorder without re-validating
//! the cap and rate-limit invariants):
//!   1. authorise the caller as the configured factory
//!   2. cross-validate the factory's `bluechip_denom` against ours
//!   3. zero-amount short-circuit — dormant economy is success, not failure
//!   4. validate the recipient address
//!   5. per-recipient rate-limit gate (cheapest gate, blocks retry storms
//!      before they touch the cap)
//!   6. rolling 24h window cap — checked but NOT persisted yet
//!   7. balance check — graceful skip without burning cap or rate-limit
//!   8. persist window debit + recipient stamp atomically
//!   9. dispatch BankMsg
//!
//! Steps 6-8 must stay in this order: persisting before the balance
//! check would burn cap budget on skipped requests; persisting after
//! dispatch is atomic in CosmWasm but adds no benefit.

use cosmwasm_std::{Addr, BankMsg, Coin, DepsMut, Env, MessageInfo, Response, Storage, Timestamp, Uint128};

use crate::attrs;
use crate::error::ContractError;
use crate::factory_query::cross_validate_factory_denom;
use crate::msg::ExpandEconomyMsg;
use crate::state::{
    Config, ExpansionWindow, CONFIG, DAILY_EXPANSION_CAP, DAILY_WINDOW_SECONDS, EXPANSION_WINDOW,
    LAST_EXPANSION_AT_RECIPIENT, RECIPIENT_EXPANSION_RATE_LIMIT_SECONDS,
};

/// Top-level entry point. See file-level comment for phase ordering.
pub fn execute_expand_economy(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExpandEconomyMsg,
) -> Result<Response, ContractError> {
    let config = require_factory_caller(deps.storage, &info.sender)?;
    cross_validate_factory_denom(deps.as_ref(), &config)?;

    let ExpandEconomyMsg::RequestExpansion { recipient, amount } = msg;

    if amount.is_zero() {
        return Ok(dormant_response());
    }

    let recipient_addr = deps.api.addr_validate(&recipient)?;
    enforce_recipient_rate_limit(deps.storage, &recipient_addr, env.block.time)?;

    let (window, new_spent) =
        check_and_compute_window(deps.storage, env.block.time, amount)?;

    if let Some(skip) =
        maybe_skip_for_balance(deps.as_ref(), &env, &config, &recipient_addr, amount)?
    {
        return Ok(skip);
    }

    persist_expansion_state(deps.storage, &recipient_addr, env.block.time, window, new_spent)?;
    Ok(payout_response(&config, &recipient_addr, amount, new_spent))
}

/// Phase 1: load `CONFIG` and require the sender to match the
/// configured factory address.
fn require_factory_caller(
    storage: &dyn Storage,
    sender: &Addr,
) -> Result<Config, ContractError> {
    let config = CONFIG.load(storage)?;
    if sender != config.factory_address {
        return Err(ContractError::Unauthorized {});
    }
    Ok(config)
}

/// Phase 3: response for the zero-amount path. The factory's bluechip
/// mint-decay polynomial drops to zero once the curve crossover is
/// passed; once it does, this contract is "dormant" by design — there
/// is no more bluechip-economy expansion to dispense, and the
/// mechanism's job is done. Surface that explicitly so operators and
/// monitoring can distinguish "skipped because schedule has expired"
/// from "skipped because of a bug".
fn dormant_response() -> Response {
    Response::new()
        .add_attribute(attrs::ACTION, attrs::REQUEST_REWARD_SKIPPED)
        .add_attribute(attrs::REASON, attrs::REASON_ECONOMY_DORMANT)
        .add_attribute(attrs::NOTE, attrs::NOTE_ECONOMY_DORMANT)
}

/// Phase 5: per-recipient rate limit. Defends against
/// `RetryFactoryNotify` storms compressing many threshold-mint payouts
/// into a single burst that empties the rolling daily budget.
/// Per-pool would require including the pool's controlling identity
/// to be effective, eliminating retry permissionlessness; per-recipient
/// keeps retry permissionless while bounding the worst-case rate at
/// one payout per `RECIPIENT_EXPANSION_RATE_LIMIT_SECONDS` to any
/// single bluechip wallet. Stamped only on the success path in
/// `persist_expansion_state` below.
fn enforce_recipient_rate_limit(
    storage: &dyn Storage,
    recipient_addr: &Addr,
    now: Timestamp,
) -> Result<(), ContractError> {
    if let Some(last) =
        LAST_EXPANSION_AT_RECIPIENT.may_load(storage, recipient_addr.as_str())?
    {
        let next_allowed = last.plus_seconds(RECIPIENT_EXPANSION_RATE_LIMIT_SECONDS);
        if now < next_allowed {
            return Err(ContractError::RecipientRateLimited {
                recipient: recipient_addr.to_string(),
                next_allowed,
                last,
                cooldown_seconds: RECIPIENT_EXPANSION_RATE_LIMIT_SECONDS,
            });
        }
    }
    Ok(())
}

/// Phase 6: rolling 24-hour spend cap. Defense-in-depth against a
/// compromised factory key forwarding huge `RequestExpansion` calls.
/// The legitimate threshold-mint schedule is well below
/// `DAILY_EXPANSION_CAP` per day; an attacker with full factory control
/// can extract at most CAP per 24-hour window via this path.
///
/// Window resets opportunistically on the first call after expiry
/// rather than continuously, which is fine for cap semantics — see
/// [`crate::state::ExpansionWindow`] doc.
///
/// Returns the window record we'll persist (with the new
/// `spent_in_window` already computed) plus the new running total so
/// the response can include it without recomputing.
fn check_and_compute_window(
    storage: &dyn Storage,
    now: Timestamp,
    amount: Uint128,
) -> Result<(ExpansionWindow, Uint128), ContractError> {
    let window = match EXPANSION_WINDOW.may_load(storage)? {
        Some(w)
            if now.seconds().saturating_sub(w.window_start.seconds())
                < DAILY_WINDOW_SECONDS =>
        {
            w
        }
        _ => ExpansionWindow {
            window_start: now,
            spent_in_window: Uint128::zero(),
        },
    };
    let new_spent = window.spent_in_window.checked_add(amount)?;
    if new_spent > DAILY_EXPANSION_CAP {
        return Err(ContractError::DailyExpansionCapExceeded {
            requested: amount,
            spent_in_window: window.spent_in_window,
            cap: DAILY_EXPANSION_CAP,
        });
    }
    Ok((window, new_spent))
}

/// Phase 7: balance check + graceful skip.
///
/// Running out of expand-economy funds is the INTENDED end-state: the
/// contract is a finite "bluechip mint boost" reservoir that drains as
/// the early ecosystem grows, tapering rewards toward zero by design.
/// A failed BankMsg here would propagate up through
/// `NotifyThresholdCrossed` and revert the entire factory tx, which
/// would in turn leave the pool's `IS_THRESHOLD_HIT = true` state in
/// place but force operators to chase the failed mint via
/// `RetryFactoryNotify` forever. Instead, log the skip and return
/// `Some(skip_response)` so threshold crossings continue to settle
/// cleanly even when the reservoir is empty.
///
/// Returns `None` when the contract has enough balance to dispense
/// `amount`; the caller continues to the persist + dispatch phases.
fn maybe_skip_for_balance(
    deps: cosmwasm_std::Deps,
    env: &Env,
    config: &Config,
    recipient_addr: &Addr,
    amount: Uint128,
) -> Result<Option<Response>, ContractError> {
    let balance = deps
        .querier
        .query_balance(&env.contract.address, &config.bluechip_denom)?;
    if balance.amount < amount {
        return Ok(Some(
            Response::new()
                .add_attribute(attrs::ACTION, attrs::REQUEST_REWARD_SKIPPED)
                .add_attribute(attrs::REASON, attrs::REASON_INSUFFICIENT_BALANCE)
                .add_attribute(attrs::RECIPIENT, recipient_addr)
                .add_attribute(attrs::REQUESTED_AMOUNT, amount)
                .add_attribute(attrs::CONTRACT_BALANCE, balance.amount)
                .add_attribute(attrs::DENOM, &config.bluechip_denom),
        ));
    }
    Ok(None)
}

/// Phase 8: persist the rolling-window debit + the per-recipient
/// rate-limit timestamp.
///
/// Order matters: skip-for-balance returned earlier without reaching
/// here, so the recipient is not penalized for outages of the
/// reservoir. CosmWasm reverts every storage write on Err, so a
/// downstream failure (e.g. BankMsg dispatch error) atomically rolls
/// back this stamp along with the window debit.
fn persist_expansion_state(
    storage: &mut dyn Storage,
    recipient_addr: &Addr,
    now: Timestamp,
    window: ExpansionWindow,
    new_spent: Uint128,
) -> Result<(), ContractError> {
    EXPANSION_WINDOW.save(
        storage,
        &ExpansionWindow {
            window_start: window.window_start,
            spent_in_window: new_spent,
        },
    )?;
    LAST_EXPANSION_AT_RECIPIENT.save(storage, recipient_addr.as_str(), &now)?;
    Ok(())
}

/// Phase 9: build the success response with the BankMsg attached.
fn payout_response(
    config: &Config,
    recipient_addr: &Addr,
    amount: Uint128,
    new_spent: Uint128,
) -> Response {
    let send_msg = BankMsg::Send {
        to_address: recipient_addr.to_string(),
        amount: vec![Coin {
            denom: config.bluechip_denom.clone(),
            amount,
        }],
    };
    Response::new()
        .add_message(send_msg)
        .add_attribute(attrs::ACTION, attrs::REQUEST_REWARD)
        .add_attribute(attrs::RECIPIENT, recipient_addr)
        .add_attribute(attrs::AMOUNT, amount)
        .add_attribute(attrs::DENOM, &config.bluechip_denom)
        .add_attribute(attrs::SPENT_IN_WINDOW_AFTER, new_spent)
        .add_attribute(attrs::DAILY_CAP, DAILY_EXPANSION_CAP)
}
