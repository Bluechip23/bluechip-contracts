//! Factory- and pool-level config propose/apply/cancel handlers.
//!
//! Every handler in this module is admin-only (gated through
//! [`super::ensure_admin`]) and, for the propose/apply pairs, subject to
//! the standard 48h [`ADMIN_TIMELOCK_SECONDS`] timelock so the community
//! has a full two-day observability window before a mutation lands.

use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, MessageInfo, Response, StdError, WasmMsg,
};

use crate::error::ContractError;
use crate::pool_struct::PoolConfigUpdate;
use crate::state::{
    FactoryInstantiate, PendingConfig, PendingPoolConfig, ADMIN_TIMELOCK_SECONDS,
    FACTORYINSTANTIATEINFO, PENDING_CONFIG, PENDING_POOL_CONFIG, POOLS_BY_ID,
};

use super::ensure_admin;

/// Validates every caller-supplied address + the bluechip_denom on a
/// `FactoryInstantiate` payload. Shared between `instantiate` and
/// `execute_propose_factory_config_update` so the same rules apply to
/// the initial config and any subsequent config proposal.
///
/// When called from the propose-update path, additionally enforces the
/// strict anchor-pool shape check (registry presence, `PoolKind::Standard`,
/// exact `[Native(bluechip), Native(atom)]` pair) — but only once the
/// one-shot `SetAnchorPool` has fired (`INITIAL_ANCHOR_SET == true`).
/// At instantiate time the anchor address is the deploy-time placeholder
/// and `INITIAL_ANCHOR_SET` is `false`, so the strict check is skipped
/// (it would fail by design — placeholder isn't a pool).
pub(crate) fn validate_factory_config(
    deps: cosmwasm_std::Deps,
    config: &FactoryInstantiate,
) -> Result<(), ContractError> {
    deps.api.addr_validate(config.factory_admin_address.as_str())?;
    deps.api.addr_validate(config.bluechip_wallet_address.as_str())?;
    deps.api
        .addr_validate(config.atom_bluechip_anchor_pool_address.as_str())?;
    if let Some(ref mint_addr) = config.bluechip_mint_contract_address {
        deps.api.addr_validate(mint_addr.as_str())?;
    }
    if config.bluechip_denom.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "bluechip_denom must be non-empty",
        )));
    }
    // `atom_denom` is the bank denom for the non-bluechip side of the
    // ATOM/bluechip anchor pool. Required at instantiate so SetAnchorPool
    // can enforce that the anchor pool actually references it. Empty would
    // either lock SetAnchorPool out indefinitely or — worse, if the empty
    // check were skipped — let the admin point the anchor at any bluechip/X
    // pool with no relation to the Pyth ATOM/USD feed.
    if config.atom_denom.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "atom_denom must be non-empty (e.g. \"uatom\" on Cosmos Hub, or the chain's \
             IBC-wrapped atom denom). Set this before instantiate or via ProposeConfigUpdate.",
        )));
    }
    if config.atom_denom == config.bluechip_denom {
        return Err(ContractError::Std(StdError::generic_err(
            "atom_denom must differ from bluechip_denom",
        )));
    }

    // Strict anchor-pool validation on the post-bootstrap path. Without
    // this gate, the propose/update flow would let an admin point the
    // anchor at any well-formed address — including a non-pool contract
    // or a pool that isn't a (bluechip, atom) Native/Native pair.
    // `execute_set_anchor_pool` enforces the same invariants on its
    // one-shot path; this runs the equivalent check on the timelocked
    // path so the two flows can't disagree on what an "anchor" is.
    let initial_anchor_set = crate::state::INITIAL_ANCHOR_SET
        .may_load(deps.storage)?
        .unwrap_or(false);
    if initial_anchor_set {
        // Compare against the currently-stored anchor; only validate when
        // the proposal actually tries to change it. Same-anchor proposals
        // (e.g., changes to other fields like fees) skip the round-trip.
        let current = FACTORYINSTANTIATEINFO
            .may_load(deps.storage)?
            .map(|c| c.atom_bluechip_anchor_pool_address);
        let changing = current
            .as_ref()
            .map(|a| a != &config.atom_bluechip_anchor_pool_address)
            .unwrap_or(true);
        if changing {
            let pool_details = super::oracle::lookup_pool_by_addr(
                deps,
                &config.atom_bluechip_anchor_pool_address,
            )?
            .ok_or_else(|| {
                ContractError::Std(StdError::generic_err(format!(
                    "Proposed anchor pool address {} is not a registered pool",
                    config.atom_bluechip_anchor_pool_address
                )))
            })?;
            super::oracle::validate_anchor_pool_choice(
                &pool_details,
                &config.bluechip_denom,
                &config.atom_denom,
            )?;
        }
    }
    Ok(())
}

pub fn execute_update_factory_config(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    let pending = PENDING_CONFIG.load(deps.storage)?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: pending.effective_after,
        });
    }

    // Re-validate at apply time. Between propose (48h ago) and apply,
    // on-chain state can have moved — most notably, `SetAnchorPool`
    // may have fired in the meantime, so the validation that ran at
    // propose time used a different `current` anchor than what is now
    // stored. Re-running it here catches stale-proposal hazards before
    // the state lands.
    validate_factory_config(deps.as_ref(), &pending.new_config)?;

    // Detect anchor change against the currently-stored anchor and, if
    // the apply will mutate it, refresh `INTERNAL_ORACLE` so it samples
    // the new anchor and clears the stale price cache. Without this,
    // the oracle would keep querying the old anchor address (which may
    // be defunct) until the next rotation interval and could either
    // freeze with `MissingAtomPool` or serve stale/wrong prices.
    let prior_anchor = FACTORYINSTANTIATEINFO
        .may_load(deps.storage)?
        .map(|c| c.atom_bluechip_anchor_pool_address);
    let new_anchor = pending.new_config.atom_bluechip_anchor_pool_address.clone();
    let anchor_changed = prior_anchor.as_ref() != Some(&new_anchor);

    FACTORYINSTANTIATEINFO.save(deps.storage, &pending.new_config)?;
    PENDING_CONFIG.remove(deps.storage);

    let mut response = Response::new().add_attribute("action", "execute_update_config");
    if anchor_changed {
        let pools_in_oracle = super::oracle::refresh_internal_oracle_for_anchor_change(
            &mut deps,
            &env,
            &new_anchor,
        )?;
        response = response
            .add_attribute("anchor_changed", "true")
            .add_attribute("new_anchor_addr", new_anchor.to_string())
            .add_attribute("pools_in_oracle_after_refresh", pools_in_oracle.to_string());
    }
    Ok(response)
}

pub fn execute_propose_factory_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    // Validate at propose time so any mistake surfaces 48h earlier than it
    // otherwise would (the existing config keeps flowing until the timelock
    // elapses and the admin calls UpdateConfig, but a malformed proposal
    // should fail loudly now, not then).
    validate_factory_config(deps.as_ref(), &config)?;

    let pending = PendingConfig {
        new_config: config,
        effective_after: env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS),
    };
    PENDING_CONFIG.save(deps.storage, &pending)?;
    Ok(Response::new()
        .add_attribute("action", "propose_config_update")
        .add_attribute("effective_after", pending.effective_after.to_string()))
}

pub fn execute_cancel_factory_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;
    PENDING_CONFIG.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

pub fn execute_propose_pool_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
    update_msg: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    // Verify pool exists. `.has()` skips deserializing PoolDetails since
    // we only need the existence check, not the value.
    if !POOLS_BY_ID.has(deps.storage, pool_id) {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Pool {} not found in registry",
            pool_id
        ))));
    }

    if PENDING_POOL_CONFIG
        .may_load(deps.storage, pool_id)?
        .is_some()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "A pool config update is already pending for this pool. Cancel it first.",
        )));
    }

    let effective_after = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS);

    PENDING_POOL_CONFIG.save(
        deps.storage,
        pool_id,
        &PendingPoolConfig {
            pool_id,
            update: update_msg,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_apply_pool_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    let pending = PENDING_POOL_CONFIG
        .load(deps.storage, pool_id)
        .map_err(|_| {
            ContractError::Std(StdError::generic_err(
                "No pending pool config update for this pool",
            ))
        })?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::TimelockNotExpired {
            effective_after: pending.effective_after,
        });
    }

    let pool_addr = POOLS_BY_ID.load(deps.storage, pool_id)?.creator_pool_addr;

    #[derive(serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum PoolExecuteMsg {
        UpdateConfigFromFactory { update: PoolConfigUpdate },
    }
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&PoolExecuteMsg::UpdateConfigFromFactory {
            update: pending.update,
        })?,
        funds: vec![],
    });

    PENDING_POOL_CONFIG.remove(deps.storage, pool_id);

    Ok(Response::new()
        .add_message(msg)
        .add_attribute("action", "execute_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string()))
}

pub fn execute_cancel_pool_config_update(
    deps: DepsMut,
    info: MessageInfo,
    pool_id: u64,
) -> Result<Response, ContractError> {
    ensure_admin(deps.as_ref(), &info)?;

    if PENDING_POOL_CONFIG
        .may_load(deps.storage, pool_id)?
        .is_none()
    {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending pool config update to cancel",
        )));
    }

    PENDING_POOL_CONFIG.remove(deps.storage, pool_id);

    Ok(Response::new()
        .add_attribute("action", "cancel_pool_config_update")
        .add_attribute("pool_id", pool_id.to_string()))
}
