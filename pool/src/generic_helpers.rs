use crate::error::ContractError;
use crate::liquidity_helpers::integer_sqrt;
use crate::msg::CommitFeeInfo;
use crate::state::{
    CommitLimitInfo, Committing, PoolFeeState, PoolInfo, PoolSpecs, ThresholdPayoutAmounts,
    COMMIT_INFO, COMMIT_LEDGER, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
    MAX_DISTRIBUTIONS_PER_TX, MAX_DISTRIBUTION_BOUNTY_RESERVE, POOL_FEE_STATE, POOL_STATE,
    USER_LAST_COMMIT,
};
use crate::state::{
    CreatorExcessLiquidity, DistributionState, PoolState, CREATOR_EXCESS_POSITION,
    DISTRIBUTION_STATE,
};
use cosmwasm_std::{
    to_json_binary, Addr, Coin, CosmosMsg, Decimal, Decimal256, DepsMut, Env, Order, StdError,
    StdResult, Storage, Timestamp, Uint128, Uint256, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_storage_plus::Bound;

// Update fee growth based on which token was offered
pub fn update_pool_fee_growth(
    pool_fee_state: &mut PoolFeeState,
    pool_state: &PoolState,
    offer_index: usize,
    commission_amt: Uint128,
) -> Result<(), ContractError> {
    if pool_state.total_liquidity.is_zero() || commission_amt.is_zero() {
        return Ok(());
    }

    let fee_growth = Decimal::from_ratio(commission_amt, pool_state.total_liquidity);

    if offer_index == 0 {
        // Token0 offered → Token1 is ask → fees in token1
        pool_fee_state.fee_growth_global_1 = pool_fee_state
            .fee_growth_global_1
            .checked_add(fee_growth)
            .map_err(|_| ContractError::Std(StdError::generic_err("Fee growth overflow")))?;
        pool_fee_state.total_fees_collected_1 = pool_fee_state
            .total_fees_collected_1
            .checked_add(commission_amt)?;
        pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_add(commission_amt)?;
    } else {
        // Token1 offered → Token0 is ask → fees in token0
        pool_fee_state.fee_growth_global_0 = pool_fee_state
            .fee_growth_global_0
            .checked_add(fee_growth)
            .map_err(|_| ContractError::Std(StdError::generic_err("Fee growth overflow")))?;
        pool_fee_state.total_fees_collected_0 = pool_fee_state
            .total_fees_collected_0
            .checked_add(commission_amt)?;
        pool_fee_state.fee_reserve_0 = pool_fee_state.fee_reserve_0.checked_add(commission_amt)?;
    }

    Ok(())
}

pub fn check_rate_limit(
    deps: &mut DepsMut,
    env: &Env,
    pool_specs: &PoolSpecs,
    sender: &Addr,
) -> Result<(), ContractError> {
    if let Some(last_commit_time) = USER_LAST_COMMIT.may_load(deps.storage, sender)? {
        let time_since_last = env.block.time.seconds().saturating_sub(last_commit_time);

        if time_since_last < pool_specs.min_commit_interval {
            let wait_time = pool_specs
                .min_commit_interval
                .saturating_sub(time_since_last);
            return Err(ContractError::TooFrequentCommits { wait_time });
        }
    }

    USER_LAST_COMMIT.save(deps.storage, sender, &env.block.time.seconds())?;

    Ok(())
}

pub fn enforce_transaction_deadline(
    current: Timestamp,
    transaction_deadline: Option<Timestamp>,
) -> Result<(), ContractError> {
    if let Some(dl) = transaction_deadline {
        if current > dl {
            return Err(ContractError::TransactionExpired {});
        }
    }
    Ok(())
}

// Helper function to calculate liquidity for deposits
pub fn validate_factory_address(
    stored_factory_addr: &Addr,
    candidate_factory_addr: &Addr,
) -> Result<(), ContractError> {
    if stored_factory_addr != candidate_factory_addr {
        return Err(ContractError::InvalidFactory {});
    }
    Ok(())
}

pub fn validate_pool_threshold_payments(
    params: &ThresholdPayoutAmounts,
) -> Result<(), ContractError> {
    const EXPECTED_CREATOR: u128 = 325_000_000_000;
    const EXPECTED_BLUECHIP: u128 = 25_000_000_000;
    const EXPECTED_POOL: u128 = 350_000_000_000;
    const EXPECTED_COMMIT: u128 = 500_000_000_000;
    const EXPECTED_TOTAL: u128 = 1_200_000_000_000;

    if params.creator_reward_amount != Uint128::new(EXPECTED_CREATOR) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Creator amount must be {}", EXPECTED_CREATOR),
        });
    }
    if params.bluechip_reward_amount != Uint128::new(EXPECTED_BLUECHIP) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("BlueChip amount must be {}", EXPECTED_BLUECHIP),
        });
    }
    if params.pool_seed_amount != Uint128::new(EXPECTED_POOL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", EXPECTED_POOL),
        });
    }
    if params.commit_return_amount != Uint128::new(EXPECTED_COMMIT) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Commit amount must be {}", EXPECTED_COMMIT),
        });
    }

    // Verify total
    let total = params
        .creator_reward_amount
        .checked_add(params.bluechip_reward_amount)?
        .checked_add(params.pool_seed_amount)?
        .checked_add(params.commit_return_amount)?;
    if total != Uint128::new(EXPECTED_TOTAL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Total must equal {} (got {})", EXPECTED_TOTAL, total),
        });
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn trigger_threshold_payout(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    pool_state: &mut PoolState,
    pool_fee_state: &mut PoolFeeState,
    commit_config: &CommitLimitInfo,
    payout: &ThresholdPayoutAmounts,
    fee_info: &CommitFeeInfo,
    env: &Env,
) -> StdResult<Vec<CosmosMsg>> {
    let mut msgs = Vec::new();

    msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: pool_info.factory_addr.to_string(),
        msg: to_json_binary(
            &pool_factory_interfaces::FactoryExecuteMsg::NotifyThresholdCrossed {
                pool_id: pool_info.pool_id,
            },
        )?,
        funds: vec![],
    }));

    let total = payout
        .creator_reward_amount
        .checked_add(payout.bluechip_reward_amount)
        .map_err(StdError::overflow)?
        .checked_add(payout.pool_seed_amount)
        .map_err(StdError::overflow)?
        .checked_add(payout.commit_return_amount)
        .map_err(StdError::overflow)?;

    if total != Uint128::new(1_200_000_000_000) {
        return Err(StdError::generic_err(
            "Threshold payout corruption detected",
        ));
    }

    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.creator_wallet_address,
        payout.creator_reward_amount,
    )?);

    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.bluechip_wallet_address,
        payout.bluechip_reward_amount,
    )?);

    msgs.push(mint_tokens(
        &pool_info.token_address,
        &env.contract.address,
        payout.pool_seed_amount,
    )?);

    let has_committers = COMMIT_LEDGER
        .range(storage, None, None, Order::Ascending)
        .next()
        .is_some();

    if has_committers {
        let dist_state = DistributionState {
            is_distributing: true,
            total_to_distribute: payout.commit_return_amount,
            total_committed_usd: commit_config.commit_amount_for_threshold_usd,
            last_processed_key: None,
            // u32::MAX: batch loop terminates via cursor exhaustion, not this counter.
            distributions_remaining: u32::MAX,
            estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
            max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
            last_successful_batch_size: None,
            consecutive_failures: 0,
            started_at: env.block.time,
            last_updated: env.block.time,
            bounty_reserve: MAX_DISTRIBUTION_BOUNTY_RESERVE,
        };
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
    }

    let total_fee_rate = fee_info
        .commit_fee_bluechip
        .checked_add(fee_info.commit_fee_creator)
        .map_err(|_| StdError::generic_err("Fee rate overflow"))?;
    let total_bluechip_raised = crate::state::NATIVE_RAISED_FROM_COMMIT.load(storage)?;
    let one_minus_fee = Decimal::one()
        .checked_sub(total_fee_rate)
        .map_err(|_| StdError::generic_err("Fee rate >= 100%"))?;
    let pools_bluechip_seed = total_bluechip_raised
        .checked_mul_floor(one_minus_fee)
        .map_err(|_| StdError::generic_err("Fee deduction overflow"))?;

    let bounty_allocation = if has_committers {
        // Only allocate if we actually have enough; otherwise take what we can.
        pools_bluechip_seed.min(MAX_DISTRIBUTION_BOUNTY_RESERVE)
    } else {
        Uint128::zero()
    };
    let pools_bluechip_seed = pools_bluechip_seed
        .checked_sub(bounty_allocation)
        .map_err(StdError::overflow)?;

    if pools_bluechip_seed > commit_config.max_bluechip_lock_per_pool {
        let excess_bluechip = pools_bluechip_seed
            .checked_sub(commit_config.max_bluechip_lock_per_pool)
            .map_err(StdError::overflow)?;

        let excess_creator_tokens = payout
            .pool_seed_amount
            .multiply_ratio(excess_bluechip, pools_bluechip_seed);

        CREATOR_EXCESS_POSITION.save(
            storage,
            &CreatorExcessLiquidity {
                creator: fee_info.creator_wallet_address.clone(),
                bluechip_amount: excess_bluechip,
                token_amount: excess_creator_tokens,
                unlock_time: env
                    .block
                    .time
                    .plus_seconds(commit_config.creator_excess_liquidity_lock_days * 86400),
                excess_nft_id: None,
            },
        )?;

        pool_state.reserve0 = commit_config.max_bluechip_lock_per_pool;
        pool_state.reserve1 = payout
            .pool_seed_amount
            .checked_sub(excess_creator_tokens)
            .map_err(StdError::overflow)?;
    } else {
        pool_state.reserve0 = pools_bluechip_seed;
        pool_state.reserve1 = payout.pool_seed_amount;
    }
    // Virtual "unowned" seed liquidity prevents first-depositor share inflation.
    let seed_liquidity = integer_sqrt(pool_state.reserve0.checked_mul(pool_state.reserve1)?);
    pool_state.total_liquidity = seed_liquidity;

    pool_fee_state.fee_growth_global_0 = Decimal::zero();
    pool_fee_state.fee_growth_global_1 = Decimal::zero();
    pool_fee_state.total_fees_collected_0 = Uint128::zero();
    pool_fee_state.total_fees_collected_1 = Uint128::zero();

    POOL_STATE.save(storage, pool_state)?;
    POOL_FEE_STATE.save(storage, pool_fee_state)?;

    Ok(msgs)
}

pub fn process_distribution_batch(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    env: &Env,
) -> StdResult<Vec<CosmosMsg>> {
    let mut msgs = Vec::new();
    let mut dist_state = match DISTRIBUTION_STATE.may_load(storage)? {
        Some(state) => state,
        None => return Ok(vec![]), // No distribution in progress
    };
    let time_since_update = env
        .block
        .time
        .seconds()
        .saturating_sub(dist_state.last_updated.seconds());
    if time_since_update > 7200 {
        // 2 hours timeout
        dist_state.consecutive_failures = 99; // Mark as failed
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
        return Err(StdError::generic_err(
            "Distribution timeout - requires manual recovery",
        ));
    }
    let start_after = dist_state.last_processed_key.as_ref().map(Bound::exclusive);

    let effective_batch_size = calculate_effective_batch_size(&dist_state);
    let mut processed_count = 0u32;
    let mut last_processed = None;
    let batch_result: StdResult<Vec<(Addr, Uint128)>> = {
        COMMIT_LEDGER
            .range(storage, start_after, None, Order::Ascending)
            .take(effective_batch_size as usize)
            .collect::<StdResult<Vec<_>>>()
    };

    match batch_result {
        Ok(batch) => {
            for (payer, usd_paid) in batch.iter() {
                let reward_result = calculate_committer_reward(
                    *usd_paid,
                    dist_state.total_to_distribute,
                    dist_state.total_committed_usd,
                );

                match reward_result {
                    Ok(reward) => {
                        if !reward.is_zero() {
                            msgs.push(mint_tokens(&pool_info.token_address, payer, reward)?);
                        }
                        COMMIT_LEDGER.remove(storage, payer);
                        last_processed = Some(payer.clone());
                        processed_count += 1;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            let new_remaining = dist_state
                .distributions_remaining
                .saturating_sub(processed_count);

            if new_remaining == 0 {
                DISTRIBUTION_STATE.remove(storage);
            } else if processed_count > 0 {
                let updated_state = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: last_processed,
                    distributions_remaining: new_remaining,
                    estimated_gas_per_distribution: dist_state.estimated_gas_per_distribution,
                    max_gas_per_tx: dist_state.max_gas_per_tx,
                    last_successful_batch_size: Some(processed_count),
                    consecutive_failures: 0,
                    started_at: dist_state.started_at,
                    last_updated: env.block.time,
                    bounty_reserve: dist_state.bounty_reserve,
                };
                DISTRIBUTION_STATE.save(storage, &updated_state)?;
            } else {
                let recheck_start = dist_state.last_processed_key.as_ref().map(Bound::exclusive);
                let remaining_entries: Vec<_> = COMMIT_LEDGER
                    .range(storage, recheck_start, None, Order::Ascending)
                    .take(1)
                    .collect::<StdResult<Vec<_>>>()?;

                if remaining_entries.is_empty() {
                    DISTRIBUTION_STATE.remove(storage);
                } else {
                    dist_state.consecutive_failures += 1;

                    if dist_state.consecutive_failures >= 5 {
                        dist_state.is_distributing = false;
                        DISTRIBUTION_STATE.save(storage, &dist_state)?;

                        return Err(StdError::generic_err(
                            "Distribution failed too many times - manual recovery needed",
                        ));
                    } else {
                        dist_state.last_updated = env.block.time;
                        DISTRIBUTION_STATE.save(storage, &dist_state)?;
                    }
                }
            }
        }
        Err(e) => {
            dist_state.consecutive_failures += 1;

            if dist_state.consecutive_failures >= 5 {
                dist_state.is_distributing = false;
                DISTRIBUTION_STATE.save(storage, &dist_state)?;

                return Err(StdError::generic_err(format!(
                    "Distribution batch read failed: {}. Manual recovery needed after {} failures",
                    e, dist_state.consecutive_failures
                )));
            } else {
                DISTRIBUTION_STATE.save(storage, &dist_state)?;
                return Err(StdError::generic_err(format!(
                    "Batch processing failed (attempt {}): {}",
                    dist_state.consecutive_failures, e
                )));
            }
        }
    }

    Ok(msgs)
}

pub fn calculate_effective_batch_size(dist_state: &DistributionState) -> u32 {
    let base_batch_size = if dist_state.estimated_gas_per_distribution == 0 {
        1u32
    } else {
        let raw = dist_state.max_gas_per_tx / dist_state.estimated_gas_per_distribution;
        if raw > u32::MAX as u64 {
            u32::MAX
        } else {
            raw as u32
        }
    };

    base_batch_size.min(MAX_DISTRIBUTIONS_PER_TX).max(1)
}

fn calculate_committer_reward(
    usd_paid: Uint128,
    total_to_distribute: Uint128,
    total_committed_usd: Uint128,
) -> StdResult<Uint128> {
    if total_committed_usd.is_zero() {
        return Ok(Uint128::zero());
    }

    let reward = Uint128::try_from(
        Uint256::from(usd_paid)
            .checked_mul(Uint256::from(total_to_distribute))?
            .checked_div(Uint256::from(total_committed_usd))?,
    )?;

    Ok(reward)
}

pub fn decimal2decimal256(dec_value: Decimal) -> StdResult<Decimal256> {
    Decimal256::from_atomics(dec_value.atomics(), dec_value.decimal_places()).map_err(|_| {
        StdError::generic_err(format!(
            "Failed to convert Decimal {} to Decimal256",
            dec_value
        ))
    })
}

pub fn get_bank_transfer_to_msg(
    recipient: &Addr,
    denom: &str,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_bank_msg = cosmwasm_std::BankMsg::Send {
        to_address: recipient.into(),
        amount: vec![Coin {
            denom: denom.to_string(),
            amount,
        }],
    };
    let transfer_bank_cosmos_msg: CosmosMsg = transfer_bank_msg.into();
    Ok(transfer_bank_cosmos_msg)
}

pub fn mint_tokens(token_addr: &Addr, recipient: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    let mint_msg = Cw20ExecuteMsg::Mint {
        recipient: recipient.to_string(),
        amount,
    };
    let exec = WasmMsg::Execute {
        contract_addr: token_addr.to_string(),
        msg: to_json_binary(&mint_msg)?,
        funds: vec![],
    };

    Ok(exec.into())
}

pub fn update_commit_info(
    storage: &mut dyn Storage,
    sender: &Addr,
    pool_contract_address: Addr,
    bluechip_amount: Uint128,
    usd_amount: Uint128,
    timestamp: Timestamp,
) -> Result<(), ContractError> {
    COMMIT_INFO.update(
        storage,
        sender,
        |maybe_committing| -> Result<_, ContractError> {
            match maybe_committing {
                Some(mut committing) => {
                    committing.total_paid_bluechip = committing
                        .total_paid_bluechip
                        .checked_add(bluechip_amount)?;
                    committing.total_paid_usd =
                        committing.total_paid_usd.checked_add(usd_amount)?;
                    committing.last_payment_bluechip = bluechip_amount;
                    committing.last_payment_usd = usd_amount;
                    committing.last_committed = timestamp;
                    Ok(committing)
                }
                None => Ok(Committing {
                    pool_contract_address,
                    committer: sender.clone(),
                    total_paid_bluechip: bluechip_amount,
                    total_paid_usd: usd_amount,
                    last_committed: timestamp,
                    last_payment_bluechip: bluechip_amount,
                    last_payment_usd: usd_amount,
                }),
            }
        },
    )?;
    Ok(())
}
