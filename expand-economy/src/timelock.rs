//! Owner-gated timelock flows for both `Config` updates and
//! one-shot `Withdrawal`s. Each flow follows the same shape:
//!   propose  → validate inputs, write pending, emit `unlocks_at`
//!   execute  → load pending, require timelock expired, apply
//!   cancel   → load pending, drop it
//!
//! The cancel arms share an `owner_gated_cancel` helper in
//! `crate::helpers`; propose/apply have flow-specific bodies but use
//! the same helpers (`load_config_as_owner`, `ensure_absent`,
//! `load_or_err`, `require_timelock_expired`).

use cosmwasm_std::{Addr, BankMsg, Coin, DepsMut, Env, MessageInfo, Response, Uint128};

use crate::attrs;
use crate::denom::validate_native_denom;
use crate::error::ContractError;
use crate::helpers::{
    ensure_absent, load_config_as_owner, load_or_err, owner_gated_cancel,
    require_timelock_expired,
};
use crate::state::{
    PendingConfigUpdate, PendingWithdrawal, CONFIG, CONFIG_TIMELOCK_SECONDS,
    PENDING_CONFIG_UPDATE, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS,
};

// ---------------------------------------------------------------------------
// Config update flow
// ---------------------------------------------------------------------------

pub fn execute_propose_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    factory_address: Option<String>,
    owner: Option<String>,
    bluechip_denom: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        ContractError::ConfigUpdateAlreadyPending,
    )?;

    // Validate inputs at propose time — invalid proposals fail here
    // rather than 48h later at apply.
    let validated_factory: Option<Addr> = factory_address
        .as_deref()
        .map(|a| deps.api.addr_validate(a))
        .transpose()?;
    let validated_owner: Option<Addr> = owner
        .as_deref()
        .map(|a| deps.api.addr_validate(a))
        .transpose()?;
    if let Some(d) = bluechip_denom.as_deref() {
        // Full cosmos-sdk denom format validation at propose time.
        // Operator typos surface 48h earlier than they otherwise would
        // (when someone tries to apply and every subsequent
        // `RequestExpansion` breaks).
        validate_native_denom(d)?;
    }

    let unlocks_at = env.block.time.plus_seconds(CONFIG_TIMELOCK_SECONDS);

    PENDING_CONFIG_UPDATE.save(
        deps.storage,
        &PendingConfigUpdate {
            factory_address: validated_factory,
            owner: validated_owner,
            bluechip_denom,
            unlocks_at,
        },
    )?;

    Ok(Response::new()
        .add_attribute(attrs::ACTION, attrs::PROPOSE_CONFIG_UPDATE)
        .add_attribute(attrs::UNLOCKS_AT, unlocks_at.to_string()))
}

pub fn execute_apply_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let mut config = load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        ContractError::NoPendingConfigUpdateToExecute,
    )?;

    require_timelock_expired(env.block.time, pending.unlocks_at)?;

    if let Some(factory) = pending.factory_address {
        config.factory_address = factory;
    }
    if let Some(new_owner) = pending.owner {
        config.owner = new_owner;
    }
    if let Some(new_denom) = pending.bluechip_denom {
        // Denom format was already enforced at propose time; re-check
        // here as defense-in-depth in case a future migration ever
        // inserts a PendingConfigUpdate directly, bypassing propose.
        // Cheap to repeat (no I/O), and locks the invariant on the
        // apply path too.
        validate_native_denom(&new_denom)?;
        config.bluechip_denom = new_denom;
    }

    CONFIG.save(deps.storage, &config)?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute(attrs::ACTION, attrs::EXECUTE_CONFIG_UPDATE)
        .add_attribute(attrs::FACTORY, &config.factory_address)
        .add_attribute(attrs::OWNER, &config.owner)
        .add_attribute(attrs::BLUECHIP_DENOM, &config.bluechip_denom))
}

pub fn execute_cancel_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    owner_gated_cancel(
        deps,
        info,
        &PENDING_CONFIG_UPDATE,
        ContractError::NoPendingConfigUpdateToCancel,
        attrs::CANCEL_CONFIG_UPDATE,
    )
}

// ---------------------------------------------------------------------------
// Withdrawal flow
// ---------------------------------------------------------------------------

pub fn execute_propose_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
    denom: String,
    recipient: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_WITHDRAWAL,
        ContractError::WithdrawalAlreadyPending,
    )?;

    let target = recipient.unwrap_or_else(|| info.sender.to_string());
    let target_addr = deps.api.addr_validate(&target)?;
    validate_native_denom(&denom)?;

    let unlocks_at = env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS);
    PENDING_WITHDRAWAL.save(
        deps.storage,
        &PendingWithdrawal {
            amount,
            denom: denom.clone(),
            recipient: target_addr.clone(),
            unlocks_at,
        },
    )?;

    Ok(Response::new()
        .add_attribute(attrs::ACTION, attrs::PROPOSE_WITHDRAWAL)
        .add_attribute(attrs::RECIPIENT, &target_addr)
        .add_attribute(attrs::AMOUNT, amount)
        .add_attribute(attrs::DENOM, denom)
        .add_attribute(attrs::UNLOCKS_AT, unlocks_at.to_string()))
}

pub fn execute_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_WITHDRAWAL,
        ContractError::NoPendingWithdrawalToExecute,
    )?;

    require_timelock_expired(env.block.time, pending.unlocks_at)?;

    PENDING_WITHDRAWAL.remove(deps.storage);

    // Clamp the requested amount to the contract's current balance so a
    // proposed-but-stale withdrawal (e.g. balance drew down via
    // RequestExpansion between propose and execute) doesn't fail the
    // whole tx at the bank module. Transfer the smaller of (requested,
    // balance) and emit both values so the caller can detect the clamp.
    let balance = deps
        .querier
        .query_balance(&env.contract.address, &pending.denom)?;
    let amount_to_send = pending.amount.min(balance.amount);

    let mut response = Response::new()
        .add_attribute(attrs::ACTION, attrs::EXECUTE_WITHDRAWAL)
        .add_attribute(attrs::RECIPIENT, &pending.recipient)
        .add_attribute(attrs::REQUESTED_AMOUNT, pending.amount)
        .add_attribute(attrs::AMOUNT, amount_to_send)
        .add_attribute(attrs::CONTRACT_BALANCE, balance.amount)
        .add_attribute(attrs::DENOM, &pending.denom);

    if !amount_to_send.is_zero() {
        let send_msg = BankMsg::Send {
            to_address: pending.recipient.into_string(),
            amount: vec![Coin {
                denom: pending.denom,
                amount: amount_to_send,
            }],
        };
        response = response.add_message(send_msg);
    } else {
        response = response.add_attribute(attrs::NOTE, attrs::NOTE_WITHDRAWAL_NO_FUNDS);
    }

    Ok(response)
}

pub fn execute_cancel_withdrawal(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    owner_gated_cancel(
        deps,
        info,
        &PENDING_WITHDRAWAL,
        ContractError::NoPendingWithdrawalToCancel,
        attrs::CANCEL_WITHDRAWAL,
    )
}
