//! Shared admin handlers: pause/unpause, cancel-emergency-withdraw,
//! factory config updates, and the two-phase emergency withdraw split.
//!
//! `execute_emergency_withdraw` is factored into
//! `execute_emergency_withdraw_initiate` (Phase 1: pause + arm the 24h
//! timelock) and `execute_emergency_withdraw_core_drain` (Phase 2: drain
//! reserves+fee_reserves+CREATOR_FEE_POT, write the audit record, flip
//! EMERGENCY_DRAINED). The creator-pool crate wraps these with its
//! commit-only bookkeeping (pre-threshold rejection, CREATOR_EXCESS_POSITION
//! sweep, DISTRIBUTION_STATE halt); standard-pool calls them directly
//! with no extras.

use crate::asset::{TokenInfo, TokenInfoPoolExt};
use crate::error::ContractError;
use crate::msg::PoolConfigUpdate;
use crate::state::{
    EmergencyWithdrawalInfo, COMMITFEEINFO, CREATOR_FEE_POT, EMERGENCY_DRAINED,
    EMERGENCY_WITHDRAWAL, EMERGENCY_WITHDRAW_DELAY_SECONDS, ORACLE_INFO, PENDING_EMERGENCY_WITHDRAW,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_PAUSED_AUTO, POOL_SPECS, POOL_STATE,
};
use cosmwasm_std::{
    Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo, Response, StdError, StdResult, Storage,
    Uint128,
};

/// Bundle returned by `execute_emergency_withdraw_core_drain`. Callers
/// turn it into a `Response` — either directly (standard-pool) or after
/// adding commit-only bookkeeping (creator-pool).
pub struct CoreDrainResult {
    pub messages: Vec<CosmosMsg>,
    pub total_0: Uint128,
    pub total_1: Uint128,
    pub recipient: Addr,
    pub total_liquidity_at_withdrawal: Uint128,
}

/// Checks that the pool has not been permanently drained. Returns
/// `ContractError::EmergencyDrained` if it has.
pub fn ensure_not_drained(storage: &dyn Storage) -> Result<(), ContractError> {
    if EMERGENCY_DRAINED.may_load(storage)?.unwrap_or(false) {
        return Err(ContractError::EmergencyDrained {});
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pause / Unpause
// ---------------------------------------------------------------------------

pub fn execute_pause(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    let pool_contract = pool_info.pool_info.contract_addr.to_string();
    POOL_PAUSED.save(deps.storage, &true)?;
    // M-2: explicit admin pause is "hard" — clear any prior auto-pause
    // state so a deposit-driven auto-unpause can't override the admin's
    // intent. If reserves happen to be low at admin-pause time, recovery
    // requires explicit Unpause, not an opportunistic deposit.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "pause")
        .add_attribute("pool_contract", pool_contract)
        .add_attribute("paused_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

pub fn execute_unpause(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    let pool_contract = pool_info.pool_info.contract_addr.to_string();
    POOL_PAUSED.save(deps.storage, &false)?;
    // M-2: clearing admin pause also clears the auto-flag. The pool is
    // now unpaused regardless of reason — the next swap/remove that
    // drains reserves below MIN will re-arm the auto-pause cleanly.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "unpause")
        .add_attribute("pool_contract", pool_contract)
        .add_attribute("unpaused_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — Phase 1: initiate (pause + 24h timelock)
// ---------------------------------------------------------------------------

pub fn execute_emergency_withdraw_initiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    ensure_not_drained(deps.storage)?;

    // Re-initiating while a timelock is already armed is a caller error —
    // the ExecuteMsg::EmergencyWithdraw handler in each pool kind is what
    // decides whether to dispatch here (Phase 1) or to core_drain (Phase 2).
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "Emergency withdraw already initiated; wait for the timelock to elapse or cancel.",
        )));
    }

    let now = env.block.time;
    POOL_PAUSED.save(deps.storage, &true)?;
    // M-2: emergency_withdraw_initiate is a "hard" pause — must not be
    // recoverable via opportunistic deposit. Override any prior auto-flag
    // so the 24h timelock can't be circumvented by a low-liquidity
    // bystander pushing reserves above MIN.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    let effective_after = now.plus_seconds(EMERGENCY_WITHDRAW_DELAY_SECONDS);
    PENDING_EMERGENCY_WITHDRAW.save(deps.storage, &effective_after)?;

    Ok(Response::new()
        .add_attribute("action", "emergency_withdraw_initiated")
        .add_attribute("effective_after", effective_after.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("initiated_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — Phase 2: core drain
// ---------------------------------------------------------------------------

/// Drains the pool-held balances that this module can see: reserves,
/// fee_reserves, and CREATOR_FEE_POT. Callers pass `accumulation_drain_0`
/// and `accumulation_drain_1` as pre-credited amounts that should be
/// folded into the grand total before the audit record is written and
/// the two per-asset transfer messages are built.
///
/// Creator-pool's wrapper passes `CREATOR_EXCESS_POSITION.bluechip_amount`
/// and `.token_amount` (it removes the storage item beforehand — atomic
/// tx semantics mean both writes land together or neither does).
/// Standard-pool passes `Uint128::zero()` on both sides.
///
/// Writes `EMERGENCY_WITHDRAWAL` with the grand total, zeroes
/// pool_state reserves + total_liquidity, zeroes fee_reserves, removes
/// CREATOR_FEE_POT, and flips `EMERGENCY_DRAINED` to true. After a
/// successful call the pool is permanently drained — any subsequent
/// `ensure_not_drained()` check rejects further admin actions.
pub fn execute_emergency_withdraw_core_drain(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    accumulation_drain_0: Uint128,
    accumulation_drain_1: Uint128,
) -> Result<CoreDrainResult, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    ensure_not_drained(deps.storage)?;

    let effective_after = PENDING_EMERGENCY_WITHDRAW
        .may_load(deps.storage)?
        .ok_or_else(|| {
            ContractError::Std(StdError::generic_err(
                "Emergency withdraw has not been initiated.",
            ))
        })?;

    if env.block.time < effective_after {
        return Err(ContractError::EmergencyTimelockPending { effective_after });
    }

    PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    let mut total0 = pool_state
        .reserve0
        .checked_add(pool_fee_state.fee_reserve_0)?;
    let mut total1 = pool_state
        .reserve1
        .checked_add(pool_fee_state.fee_reserve_1)?;

    // CREATOR_FEE_POT — shared pot that holds the clip slice from
    // `calculate_fee_size_multiplier`. Sweep and remove unconditionally.
    if let Some(pot) = CREATOR_FEE_POT.may_load(deps.storage)? {
        total0 = total0.checked_add(pot.amount_0)?;
        total1 = total1.checked_add(pot.amount_1)?;
        CREATOR_FEE_POT.remove(deps.storage);
    }

    // Caller-supplied pre-credited amounts (e.g. creator-pool's
    // CREATOR_EXCESS_POSITION, which this crate can't see).
    total0 = total0.checked_add(accumulation_drain_0)?;
    total1 = total1.checked_add(accumulation_drain_1)?;

    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let recipient = fee_info.bluechip_wallet_address.clone();

    let withdrawal_info = EmergencyWithdrawalInfo {
        withdrawn_at: env.block.time.seconds(),
        recipient: recipient.clone(),
        amount0: total0,
        amount1: total1,
        total_liquidity_at_withdrawal: pool_state.total_liquidity,
    };
    EMERGENCY_WITHDRAWAL.save(deps.storage, &withdrawal_info)?;

    let total_liquidity_at_withdrawal = pool_state.total_liquidity;

    pool_state.reserve0 = Uint128::zero();
    pool_state.reserve1 = Uint128::zero();
    pool_state.total_liquidity = Uint128::zero();
    POOL_STATE.save(deps.storage, &pool_state)?;

    pool_fee_state.fee_reserve_0 = Uint128::zero();
    pool_fee_state.fee_reserve_1 = Uint128::zero();
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    EMERGENCY_DRAINED.save(deps.storage, &true)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    if !total0.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[0].clone(),
                amount: total0,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }
    if !total1.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[1].clone(),
                amount: total1,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }

    Ok(CoreDrainResult {
        messages,
        total_0: total0,
        total_1: total1,
        recipient,
        total_liquidity_at_withdrawal,
    })
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — cancel (pre-drain only)
// ---------------------------------------------------------------------------

pub fn execute_cancel_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_none() {
        return Err(ContractError::NoPendingEmergencyWithdraw {});
    }
    PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);
    POOL_PAUSED.save(deps.storage, &false)?;
    // M-2: emergency cancel clears any auto-flag (the cancel returns
    // the pool to fully open state).
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "emergency_withdraw_cancelled")
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("cancelled_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Config update (factory-only)
// ---------------------------------------------------------------------------

pub fn execute_update_config_from_factory(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    update: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let mut attributes = vec![("action", "update_config")];
    let mut specs = POOL_SPECS.load(deps.storage)?;
    let mut specs_changed = false;

    if let Some(fee) = update.lp_fee {
        let max_lp_fee = Decimal::percent(10);
        let min_lp_fee = Decimal::permille(1); // 0.1%
        if fee > max_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must not exceed 10% (0.1)",
            )));
        }
        if fee < min_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must be at least 0.1% (0.001)",
            )));
        }
        specs.lp_fee = fee;
        specs_changed = true;
        attributes.push(("lp_fee", "updated"));
    }

    if let Some(interval) = update.min_commit_interval {
        const MAX_COMMIT_INTERVAL: u64 = 86_400; // 24 hours
        if interval > MAX_COMMIT_INTERVAL {
            return Err(ContractError::Std(StdError::generic_err(
                "min_commit_interval must not exceed 86400 seconds (1 day)",
            )));
        }
        specs.min_commit_interval = interval;
        specs_changed = true;
        attributes.push(("min_commit_interval", "updated"));
    }

    if specs_changed {
        POOL_SPECS.save(deps.storage, &specs)?;
    }

    if let Some(oracle_addr) = update.oracle_address {
        ORACLE_INFO.update(deps.storage, |mut info| -> StdResult<_> {
            info.oracle_addr = deps.api.addr_validate(&oracle_addr)?;
            Ok(info)
        })?;
        attributes.push(("oracle_address", "updated"));
    }

    Ok(Response::new()
        .add_attributes(attributes)
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("updated_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}
