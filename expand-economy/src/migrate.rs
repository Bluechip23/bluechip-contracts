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
/// - parse the cw2-stored version + the compile-time `CONTRACT_VERSION`
/// as semver; reject any migrate where stored > current
/// - tolerate a missing cw2 entry (legacy / test fixtures), skipping
/// the comparison rather than erroring
/// - bump the cw2 record on success so subsequent migrates see the
/// new version
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

    // M-3.3 migration shim: convert any prior single-bucket
    // EXPANSION_WINDOW Item ("expansion_window") into a one-entry
    // sliding-window EXPANSION_LOG. The legacy bucket carried
    // `(window_start, spent_in_window)`; the equivalent log shape is
    // a single entry timestamped at `window_start` with `amount =
    // spent_in_window`. Subsequent expansion calls naturally age it
    // out once `window_start + DAILY_WINDOW_SECONDS` passes.
    //
    // Read raw via a private alias to avoid keeping the old type in
    // `crate::state`. Idempotent: if EXPANSION_LOG is already
    // populated (re-run migrate or fresh deploy), the old bucket
    // simply doesn't exist and we skip; if the old bucket exists
    // with zero spent, we still clear it so the storage slot
    // doesn't linger.
    migrate_expansion_window_to_log(deps.storage)?;

    match msg {
        MigrateMsg::UpdateVersion {} => Ok(Response::new()
            .add_attribute(attrs::ACTION, attrs::MIGRATE)
            .add_attribute(attrs::VARIANT, attrs::MIGRATE_VARIANT_UPDATE_VERSION)
            .add_attribute(attrs::CONTRACT_NAME, CONTRACT_NAME)
            .add_attribute(attrs::CONTRACT_VERSION, CONTRACT_VERSION)),
    }
}

/// Convert the legacy `EXPANSION_WINDOW` single-bucket Item into the
/// new sliding-window `EXPANSION_LOG`. Idempotent: a missing legacy
/// slot is a no-op; a present-but-zero slot is cleared without
/// writing a log entry; a present-with-spend slot becomes a single
/// log entry preserving the original `window_start` timestamp so it
/// ages out correctly under the new sliding cap arithmetic.
fn migrate_expansion_window_to_log(
    storage: &mut dyn cosmwasm_std::Storage,
) -> Result<(), crate::error::ContractError> {
    use cosmwasm_schema::cw_serde;
    use cosmwasm_std::{Timestamp, Uint128};
    use cw_storage_plus::Item;

    #[cw_serde]
    struct LegacyExpansionWindow {
        window_start: Timestamp,
        spent_in_window: Uint128,
    }
    let legacy: Item<LegacyExpansionWindow> = Item::new("expansion_window");

    if let Some(old) = legacy.may_load(storage)? {
        if !old.spent_in_window.is_zero() {
            // Only seed the log if the new slot is empty; if migrate
            // is re-run, don't double-credit by appending a duplicate.
            if crate::state::EXPANSION_LOG
                .may_load(storage)?
                .is_none()
            {
                crate::state::EXPANSION_LOG.save(
                    storage,
                    &vec![crate::state::ExpansionEntry {
                        timestamp: old.window_start,
                        amount: old.spent_in_window,
                    }],
                )?;
            }
        }
        legacy.remove(storage);
    }
    Ok(())
}
