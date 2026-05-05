//! Shared owner-gating + storage-item helpers used by `instantiate`,
//! `execute`, and the timelock flows. Lifted out of `contract.rs` so
//! every entry-point handler reaches them at the same depth.

use cosmwasm_std::{Addr, DepsMut, MessageInfo, Response, Storage, Timestamp};
use cw_storage_plus::Item;
use serde::{de::DeserializeOwned, Serialize};

use crate::attrs;
use crate::error::ContractError;
use crate::state::{Config, CONFIG};

/// Load `CONFIG` and require the sender to match `config.owner`.
///
/// `CONFIG` is set in `instantiate` and never removed — `load` (rather
/// than `may_load`) is correct here, and produces a `StdError::NotFound`
/// only as a programmer-error backstop. Every owner-gated handler
/// funnels through this helper so the owner check exists in exactly one
/// place.
pub fn load_config_as_owner(
    storage: &dyn Storage,
    sender: &Addr,
) -> Result<Config, ContractError> {
    let config = CONFIG.load(storage)?;
    if sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }
    Ok(config)
}

/// Error with `err` if `item` is already populated. Used by the propose
/// handlers to refuse a second pending update before the existing one is
/// either applied or cancelled.
pub fn ensure_absent<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err: ContractError,
) -> Result<(), ContractError>
where
    T: Serialize + DeserializeOwned,
{
    if item.may_load(storage)?.is_some() {
        return Err(err);
    }
    Ok(())
}

/// Load `item` or return `err`. Replacement for `may_load + ok_or` at
/// the apply / cancel sites so each timelock helper has one well-typed
/// error per missing-pending case.
pub fn load_or_err<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err: ContractError,
) -> Result<T, ContractError>
where
    T: Serialize + DeserializeOwned,
{
    item.may_load(storage)?.ok_or(err)
}

/// Reject if `now < ready_at`. Single source of truth for the
/// "Timelock not expired" check shared by `ExecuteConfigUpdate` and
/// `ExecuteWithdrawal`.
pub fn require_timelock_expired(
    now: Timestamp,
    ready_at: Timestamp,
) -> Result<(), ContractError> {
    if now < ready_at {
        return Err(ContractError::TimelockNotExpired { ready_at });
    }
    Ok(())
}

/// Owner-gated cancel for any timelocked `Item<T>`. Centralizes the
/// shape:
///
///     load_config_as_owner -> load_or_err(missing) -> item.remove -> Response
///
/// shared by `CancelConfigUpdate` and `CancelWithdrawal`. Adding a
/// third cancel path becomes a one-line call to this helper.
pub fn owner_gated_cancel<T>(
    deps: DepsMut,
    info: MessageInfo,
    item: &Item<T>,
    missing_err: ContractError,
    action: &'static str,
) -> Result<Response, ContractError>
where
    T: Serialize + DeserializeOwned,
{
    load_config_as_owner(deps.storage, &info.sender)?;
    load_or_err(deps.storage, item, missing_err)?;
    item.remove(deps.storage);
    Ok(Response::new().add_attribute(attrs::ACTION, action))
}
