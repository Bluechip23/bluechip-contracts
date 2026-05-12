//! `RequestExpansion` handler, decomposed into one helper per phase so
//! the dispatcher stays under ~25 lines. Each helper is independently
//! unit-testable and documents one concern.
//!
//! Phase ordering (load-bearing, do not reorder without re-validating
//! the cap invariant):
//!   1. authorise the caller as the configured factory
//!   2. cross-validate the factory's `bluechip_denom` against ours
//!   3. zero-amount short-circuit — dormant economy is success, not failure
//!   4. validate the recipient address
//!   5. rolling 24h window cap — checked but NOT persisted yet
//!   6. balance check — graceful skip without burning cap budget
//!   7. persist window debit
//!   8. dispatch BankMsg
//!
//! Steps 5-7 must stay in this order: persisting before the balance
//! check would burn cap budget on skipped requests; persisting after
//! dispatch is atomic in CosmWasm but adds no benefit.

use cosmwasm_std::{Addr, BankMsg, Coin, DepsMut, Env, MessageInfo, Response, Storage, Timestamp, Uint128};

use crate::attrs;
use crate::error::ContractError;
use crate::factory_query::cross_validate_factory_denom;
use crate::msg::ExpandEconomyMsg;
use crate::state::{
    Config, ExpansionEntry, CONFIG, DAILY_EXPANSION_CAP, DAILY_WINDOW_SECONDS, EXPANSION_LOG,
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

    let (pruned_log, new_spent) =
        check_and_compute_window(deps.storage, env.block.time, amount)?;

    if let Some(skip) =
        maybe_skip_for_balance(deps.as_ref(), &env, &config, &recipient_addr, amount)?
    {
        return Ok(skip);
    }

    persist_expansion_state(deps.storage, pruned_log, env.block.time, amount)?;
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

/// Phase 5: rolling 24-hour spend cap. Defense-in-depth against a
/// compromised factory key forwarding huge `RequestExpansion` calls.
/// The legitimate threshold-mint schedule is well below
/// `DAILY_EXPANSION_CAP` per day; an attacker with full factory control
/// can extract at most CAP per any 24-hour window via this path.
///
/// True sliding window: each call prunes entries older than
/// `DAILY_WINDOW_SECONDS` from the persisted log, then sums the
/// remaining entries to get the in-window total. Compared to the prior
/// single-bucket reset, this prevents the boundary-burst case (max
/// the cap just before the bucket flip, then max the fresh bucket
/// immediately after — sliding semantics keep both halves visible
/// in any rolling 24h window across the boundary).
///
/// Returns the pruned log (caller will append the new entry on the
/// success path) plus the new running total for the response.
fn check_and_compute_window(
    storage: &dyn Storage,
    now: Timestamp,
    amount: Uint128,
) -> Result<(Vec<ExpansionEntry>, Uint128), ContractError> {
    let cutoff_secs = now.seconds().saturating_sub(DAILY_WINDOW_SECONDS);
    let mut log = EXPANSION_LOG.may_load(storage)?.unwrap_or_default();
    // Drop entries whose timestamp is at-or-below the cutoff. Using `<=`
    // (i.e. retain `> cutoff_secs`) is intentional: an entry exactly
    // `DAILY_WINDOW_SECONDS` old is no longer "in the last 24 hours"
    // and stops counting. Matches the boundary semantics of the prior
    // single-bucket check (`now - window_start >= 24h` reset).
    log.retain(|e| e.timestamp.seconds() > cutoff_secs);

    let spent = log.iter().try_fold(Uint128::zero(), |acc, e| {
        acc.checked_add(e.amount)
    })?;
    let new_spent = spent.checked_add(amount)?;
    if new_spent > DAILY_EXPANSION_CAP {
        return Err(ContractError::DailyExpansionCapExceeded {
            requested: amount,
            spent_in_window: spent,
            cap: DAILY_EXPANSION_CAP,
        });
    }
    Ok((log, new_spent))
}

/// Phase 6: balance check + graceful skip.
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

/// Phase 7: persist the rolling-window debit.
///
/// Append the new entry to the already-pruned log produced by
/// `check_and_compute_window` and save the combined log. Order matters:
/// skip-for-balance returned earlier without reaching here, so an
/// empty reservoir does not consume window budget. CosmWasm reverts
/// every storage write on Err, so a downstream failure (e.g. BankMsg
/// dispatch error) atomically rolls back this append.
fn persist_expansion_state(
    storage: &mut dyn Storage,
    mut pruned_log: Vec<ExpansionEntry>,
    now: Timestamp,
    amount: Uint128,
) -> Result<(), ContractError> {
    pruned_log.push(ExpansionEntry {
        timestamp: now,
        amount,
    });
    EXPANSION_LOG.save(storage, &pruned_log)?;
    Ok(())
}

/// Phase 8: build the success response with the BankMsg attached.
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
