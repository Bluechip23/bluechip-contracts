//! `migrate` entry point + downgrade guard. Mirrors the same shape used
//! by the pool / factory contracts: tolerate a missing cw2 record
//! (legacy / test fixtures), refuse stored > current, otherwise bump
//! the cw2 record on success.

use cosmwasm_std::{DepsMut, Env, Response};
use cw2::set_contract_version;

use crate::attrs;
use crate::error::ContractError;
use crate::msg::MigrateMsg;
use crate::{CONTRACT_NAME, CONTRACT_VERSION};

/// Without this entry point the chain rejects every
/// `MsgMigrateContract` at runtime with "no migrate function exported",
/// which would leave this contract effectively immutable despite cw2
/// being initialised at instantiate time. Mirrors the downgrade
/// guard the pool / factory contracts already use:
///
///   - parse the cw2-stored version + the compile-time `CONTRACT_VERSION`
///     as semver; reject any migrate where stored > current
///   - tolerate a missing cw2 entry (legacy / test fixtures), skipping
///     the comparison rather than erroring
///   - bump the cw2 record on success so subsequent migrates see the
///     new version
///
/// `MigrateMsg::UpdateVersion {}` is the no-op variant — it exists so
/// operators have an explicit "just bump the version, no other state
/// changes" path. Future variants can carry parameters; the downgrade
/// guard runs unconditionally first so they all benefit.
pub fn migrate(
    deps: DepsMut,
    _env: Env,
    msg: MigrateMsg,
) -> Result<Response, ContractError> {
    if let Ok(stored_version) = cw2::get_contract_version(deps.storage) {
        let stored_semver: semver::Version = stored_version.version.parse().map_err(|e: semver::Error| {
            ContractError::StoredVersionInvalid {
                version: stored_version.version.clone(),
                msg: e.to_string(),
            }
        })?;
        let current_semver: semver::Version = CONTRACT_VERSION.parse().map_err(|e: semver::Error| {
            ContractError::CurrentVersionInvalid {
                version: CONTRACT_VERSION.to_string(),
                msg: e.to_string(),
            }
        })?;
        if stored_semver > current_semver {
            return Err(ContractError::DowngradeRefused {
                stored: stored_semver.to_string(),
                current: current_semver.to_string(),
            });
        }
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    match msg {
        MigrateMsg::UpdateVersion {} => Ok(Response::new()
            .add_attribute(attrs::ACTION, attrs::MIGRATE)
            .add_attribute(attrs::VARIANT, attrs::MIGRATE_VARIANT_UPDATE_VERSION)
            .add_attribute(attrs::CONTRACT_NAME, CONTRACT_NAME)
            .add_attribute(attrs::CONTRACT_VERSION, CONTRACT_VERSION)),
    }
}
