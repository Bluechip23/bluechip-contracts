//! Administrative operations: pause/unpause, emergency withdraw, config
//! updates, and stuck-state recovery.
//!
//! All functions in this module are privileged and require the caller to be
//! the factory admin (or, for recovery, the factory contract itself).

use crate::asset::{TokenInfo, TokenInfoPoolExt};
use crate::error::ContractError;
use crate::msg::PoolConfigUpdate;
use crate::state::{
    DistributionState, EmergencyWithdrawalInfo, RecoveryType, COMMITFEEINFO, COMMIT_LEDGER,
    CREATOR_EXCESS_POSITION, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
    DISTRIBUTION_STATE, EMERGENCY_DRAINED, EMERGENCY_WITHDRAWAL, EMERGENCY_WITHDRAW_DELAY_SECONDS,
    EXPECTED_FACTORY, LAST_THRESHOLD_ATTEMPT, ORACLE_INFO, PENDING_EMERGENCY_WITHDRAW,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE, REENTRANCY_GUARD,
    THRESHOLD_PROCESSING,
};
use cosmwasm_std::{
    Decimal, DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult, Storage, Timestamp,
    Uint128,
};

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
    Ok(Response::new()
        .add_attribute("action", "unpause")
        .add_attribute("pool_contract", pool_contract)
        .add_attribute("unpaused_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw (two-phase: initiate → execute after 24h timelock)
// ---------------------------------------------------------------------------

pub fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let now = env.block.time;

    // Phase 2: execute if timelock has elapsed
    if let Some(effective_after) = PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)? {
        if now < effective_after {
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

        if let Ok(excess) = CREATOR_EXCESS_POSITION.load(deps.storage) {
            total0 = total0.checked_add(excess.bluechip_amount)?;
            total1 = total1.checked_add(excess.token_amount)?;
            CREATOR_EXCESS_POSITION.remove(deps.storage);
        }

        let fee_info = COMMITFEEINFO.load(deps.storage)?;
        let recipient = fee_info.bluechip_wallet_address.clone();

        let withdrawal_info = EmergencyWithdrawalInfo {
            withdrawn_at: now.seconds(),
            recipient: recipient.clone(),
            amount0: total0,
            amount1: total1,
            total_liquidity_at_withdrawal: pool_state.total_liquidity,
        };
        EMERGENCY_WITHDRAWAL.save(deps.storage, &withdrawal_info)?;

        pool_state.reserve0 = Uint128::zero();
        pool_state.reserve1 = Uint128::zero();
        pool_state.total_liquidity = Uint128::zero();
        POOL_STATE.save(deps.storage, &pool_state)?;

        pool_fee_state.fee_reserve_0 = Uint128::zero();
        pool_fee_state.fee_reserve_1 = Uint128::zero();
        POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

        EMERGENCY_DRAINED.save(deps.storage, &true)?;

        if let Ok(mut dist_state) = DISTRIBUTION_STATE.load(deps.storage) {
            total0 = total0.checked_add(dist_state.bounty_reserve)?;
            dist_state.bounty_reserve = Uint128::zero();
            dist_state.is_distributing = false;
            dist_state.distributions_remaining = 0;
            DISTRIBUTION_STATE.save(deps.storage, &dist_state)?;
        }

        let mut messages = vec![];
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

        return Ok(Response::new()
            .add_messages(messages)
            .add_attribute("action", "emergency_withdraw")
            .add_attribute("recipient", recipient)
            .add_attribute("amount0", total0)
            .add_attribute("amount1", total1)
            .add_attribute(
                "total_liquidity",
                withdrawal_info.total_liquidity_at_withdrawal,
            )
            .add_attribute("pool_contract", env.contract.address.to_string())
            .add_attribute("block_height", env.block.height.to_string())
            .add_attribute("block_time", env.block.time.seconds().to_string()));
    }

    // Phase 1: initiate — pause pool and set timelock
    POOL_PAUSED.save(deps.storage, &true)?;
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

    if let Some(tolerance) = update.usd_payment_tolerance_bps {
        if tolerance > 1000 {
            return Err(ContractError::Std(StdError::generic_err(
                "usd_payment_tolerance_bps must not exceed 1000 (10%)",
            )));
        }
        specs.usd_payment_tolerance_bps = tolerance;
        specs_changed = true;
        attributes.push(("usd_payment_tolerance_bps", "updated"));
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

// ---------------------------------------------------------------------------
// Stuck-state recovery (factory-only)
// ---------------------------------------------------------------------------

pub fn execute_recover_stuck_states(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recovery_type: RecoveryType,
) -> Result<Response, ContractError> {
    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;
    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    let mut attributes = vec![("action", "recover_stuck_states".to_string())];
    let mut recovered_items = vec![];

    match recovery_type {
        RecoveryType::StuckThreshold => {
            recover_threshold(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckDistribution => {
            recover_distribution(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckReentrancyGuard => {
            recover_reentrancy_guard(deps.storage, &mut recovered_items)?;
        }
        RecoveryType::Both => {
            let _ = recover_threshold(deps.storage, &env, &mut recovered_items);
            let _ = recover_distribution(deps.storage, &env, &mut recovered_items);
            let _ = recover_reentrancy_guard(deps.storage, &mut recovered_items);
        }
    }

    if recovered_items.is_empty() {
        return Err(ContractError::NothingToRecover {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    attributes.push(("recovered", recovered_items.join(",")));
    attributes.push((
        "pool_contract",
        pool_info.pool_info.contract_addr.to_string(),
    ));
    attributes.push(("recovered_by", info.sender.to_string()));
    attributes.push(("block_height", env.block.height.to_string()));
    attributes.push(("block_time", env.block.time.seconds().to_string()));
    Ok(Response::new().add_attributes(attributes))
}

fn recover_threshold(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    let last_threshold_time = LAST_THRESHOLD_ATTEMPT
        .may_load(storage)?
        .unwrap_or(Timestamp::from_seconds(0));

    if env.block.time.seconds() >= last_threshold_time.seconds() + 3600 {
        let was_stuck = THRESHOLD_PROCESSING.may_load(storage)?.unwrap_or(false);
        if was_stuck {
            THRESHOLD_PROCESSING.save(storage, &false)?;
            recovered.push("threshold".to_string());
        }
    }
    Ok(())
}

fn recover_distribution(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    if let Some(dist_state) = DISTRIBUTION_STATE.may_load(storage)? {
        let time_since_update = env
            .block
            .time
            .seconds()
            .saturating_sub(dist_state.last_updated.seconds());

        if time_since_update >= 3600 || dist_state.consecutive_failures >= 5 {
            let remaining_committers = COMMIT_LEDGER
                .keys(storage, None, None, Order::Ascending)
                .count() as u32;

            if remaining_committers == 0 {
                DISTRIBUTION_STATE.remove(storage);
            } else {
                let restarted = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: None,
                    distributions_remaining: remaining_committers,
                    estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
                    max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
                    last_successful_batch_size: None,
                    consecutive_failures: 0,
                    started_at: env.block.time,
                    last_updated: env.block.time,
                    bounty_reserve: dist_state.bounty_reserve,
                };
                DISTRIBUTION_STATE.save(storage, &restarted)?;
            }

            recovered.push(format!(
                "distribution_restarted_{}_remaining",
                remaining_committers
            ));
        }
    }
    Ok(())
}

fn recover_reentrancy_guard(
    storage: &mut dyn Storage,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    let guard = REENTRANCY_GUARD.may_load(storage)?.unwrap_or(false);
    if guard {
        REENTRANCY_GUARD.save(storage, &false)?;
        recovered.push("reentrancy_guard".to_string());
    }
    Ok(())
}
