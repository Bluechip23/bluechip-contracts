#![allow(non_snake_case)]
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg};
use crate::state::{
    CommitLimitInfo, PoolFeeState, PoolInfo, PoolSpecs, ThresholdPayoutAmounts, COMMIT_LEDGER,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX, POOL_FEE_STATE, POOL_STATE,
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
use std::vec;

// Update fee growth based on which token was offered
pub fn update_pool_fee_growth(
    pool_fee_state: &mut PoolFeeState,
    pool_state: &PoolState,
    offer_contract_addressx: usize,
    commission_amt: Uint128,
) -> Result<(), ContractError> {
    if pool_state.total_liquidity.is_zero() || commission_amt.is_zero() {
        return Ok(());
    }

    let fee_growth = Decimal::from_ratio(commission_amt, pool_state.total_liquidity);

    if offer_contract_addressx == 0 {
        // Token0 offered → Token1 is ask → fees in token1
        pool_fee_state.fee_growth_global_1 += fee_growth;
        pool_fee_state.total_fees_collected_1 += commission_amt;
        pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_add(commission_amt)?;
    } else {
        // Token1 offered → Token0 is ask → fees in token0
        pool_fee_state.fee_growth_global_0 += fee_growth;
        pool_fee_state.total_fees_collected_0 += commission_amt;
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
        let time_since_last = env.block.time.seconds() - last_commit_time;

        if time_since_last < pool_specs.min_commit_interval {
            let wait_time = pool_specs.min_commit_interval - time_since_last;
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
    // the ONLY acceptable values
    const EXPECTED_CREATOR: u128 = 325_000_000_000;
    const EXPECTED_BLUECHIP: u128 = 25_000_000_000;
    const EXPECTED_POOL: u128 = 350_000_000_000;
    const EXPECTED_COMMIT: u128 = 500_000_000_000;
    const EXPECTED_TOTAL: u128 = 1_200_000_000_000;

    // verify each amount specifically - creator amount
    if params.creator_reward_amount != Uint128::new(EXPECTED_CREATOR) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Creator amount must be {}", EXPECTED_CREATOR),
        });
    }
    //bluechip amount
    if params.bluechip_reward_amount != Uint128::new(EXPECTED_BLUECHIP) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("BlueChip amount must be {}", EXPECTED_BLUECHIP),
        });
    }
    //pool seeding amount
    if params.pool_seed_amount != Uint128::new(EXPECTED_POOL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", EXPECTED_POOL),
        });
    }
    //amount sent back to origincal commiters
    if params.commit_return_amount != Uint128::new(EXPECTED_COMMIT) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Commit amount must be {}", EXPECTED_COMMIT),
        });
    }

    // Verify total
    let total = params.creator_reward_amount
        + params.bluechip_reward_amount
        + params.pool_seed_amount
        + params.commit_return_amount;
    //throw error if anything of them is off - there is also a max mint number to help with the exactness
    if total != Uint128::new(EXPECTED_TOTAL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Total must equal {} (got {})", EXPECTED_TOTAL, total),
        });
    }

    Ok(())
}

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

    let total = payout.creator_reward_amount
        + payout.bluechip_reward_amount
        + payout.pool_seed_amount
        + payout.commit_return_amount;

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

    let total_committers = COMMIT_LEDGER
        .keys(storage, None, None, Order::Ascending)
        .count();

    let (estimated_gas_per_distribution, max_gas_per_tx) = {
        let default_estimated = DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION;
        let default_max_gas = DEFAULT_MAX_GAS_PER_TX;
        (default_estimated, default_max_gas)
    };

    let batch_size = if estimated_gas_per_distribution == 0 {
        1u32
    } else {
        (max_gas_per_tx / estimated_gas_per_distribution).max(1) as u32
    };

    if total_committers == 0 {
        // No committers to pay
    } else if total_committers <= batch_size as usize {
        let committers: Vec<(Addr, Uint128)> = COMMIT_LEDGER
            .range(storage, None, None, Order::Ascending)
            .map(|r| r.map_err(|e| StdError::generic_err(e.to_string())))
            .collect::<StdResult<Vec<_>>>()?;

        for (payer, usd_paid) in committers {
            let reward = calculate_committer_reward(
                usd_paid,
                payout.commit_return_amount,
                commit_config.commit_amount_for_threshold_usd,
            )?;

            if !reward.is_zero() {
                msgs.push(mint_tokens(&pool_info.token_address, &payer, reward)?);
            }

            COMMIT_LEDGER.remove(storage, &payer);
        }
    } else {
        // Too many committers, need batched distribution

        let test_batch: Vec<_> = COMMIT_LEDGER
            .range(storage, None, None, Order::Ascending)
            .take(1)
            .collect::<StdResult<Vec<_>>>()
            .map_err(|e| StdError::generic_err(format!("Failed to read committers: {}", e)))?;

        if test_batch.is_empty() {
            // No committers but count said there were - data inconsistency
            return Err(StdError::generic_err("Committer count mismatch"));
        }
        let dist_state = DistributionState {
            is_distributing: true,
            total_to_distribute: payout.commit_return_amount,
            total_committed_usd: commit_config.commit_amount_for_threshold_usd,
            last_processed_key: None,
            distributions_remaining: total_committers as u32,
            estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
            max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
            last_successful_batch_size: None,
            consecutive_failures: 0,
            started_at: env.block.time,
            last_updated: env.block.time,
        };
        let save_result = DISTRIBUTION_STATE.save(storage, &dist_state);
        if save_result.is_err() {
            // If we can't save state, don't try to process
            return Err(StdError::generic_err(
                "Failed to initialize distribution state",
            ));
        }

        match process_distribution_batch(storage, pool_info, env) {
            Ok(batch_msgs) => {
                msgs.extend(batch_msgs);
            }
            Err(e) => {
                DISTRIBUTION_STATE.remove(storage);
                return Err(StdError::generic_err(format!(
                    "Distribution initialization failed: {}. Threshold crossed but distributions pending manual recovery.", 
                    e
                )));
            }
        }
    }

    let total_fee_rate = fee_info.commit_fee_bluechip + fee_info.commit_fee_creator;
    let total_bluechip_raised = crate::state::NATIVE_RAISED_FROM_COMMIT.load(storage)?;
    let pools_bluechip_seed = total_bluechip_raised * (Decimal::one() - total_fee_rate);

    if pools_bluechip_seed > commit_config.max_bluechip_lock_per_pool {
        let excess_bluechip = pools_bluechip_seed - commit_config.max_bluechip_lock_per_pool;

        // Calculate proportional creator tokens for the excess
        // (excess_bluechip / total_bluechip) * total_creator_tokens
        let excess_creator_tokens = payout
            .pool_seed_amount
            .multiply_ratio(excess_bluechip, pools_bluechip_seed);

        // Store creator's locked excess position
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

        // Set capped reserves for dead liquidity
        pool_state.reserve0 = commit_config.max_bluechip_lock_per_pool;
        pool_state.reserve1 = payout.pool_seed_amount - excess_creator_tokens;

        // Add excess to reserves (tracked but owned by creator after unlock)
        pool_state.reserve0 += excess_bluechip;
        pool_state.reserve1 += excess_creator_tokens;
    } else {
        pool_state.reserve0 = pools_bluechip_seed;
        pool_state.reserve1 = payout.pool_seed_amount;
    }
    pool_state.total_liquidity = Uint128::zero();

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
    let time_since_update = env.block.time.seconds() - dist_state.last_updated.seconds();
    if time_since_update > 7200 {
        // 2 hours timeout
        dist_state.consecutive_failures = 99; // Mark as failed
        DISTRIBUTION_STATE.save(storage, &dist_state)?;
        return Err(StdError::generic_err(
            "Distribution timeout - requires manual recovery",
        ));
    }
    // Determine starting point
    let start_after = dist_state
        .last_processed_key
        .as_ref()
        .map(|addr| Bound::exclusive(addr));

    let effective_batch_size = calculate_effective_batch_size(&dist_state);
    // Track what we actually process
    let mut processed_count = 0u32;
    let mut last_processed = None;
    let batch_result = (|| -> StdResult<Vec<(Addr, Uint128)>> {
        COMMIT_LEDGER
            .range(storage, start_after, None, Order::Ascending)
            .take(effective_batch_size as usize)
            .collect::<StdResult<Vec<_>>>()
    })();

    match batch_result {
        Ok(batch) => {
            // Process each committer
            for (payer, usd_paid) in batch.iter() {
                // Calculate reward with error handling
                let reward_result = calculate_committer_reward(
                    *usd_paid,
                    dist_state.total_to_distribute,
                    dist_state.total_committed_usd,
                );

                match reward_result {
                    Ok(reward) => {
                        if !reward.is_zero() {
                            // Try to create mint message
                            match mint_tokens(&pool_info.token_address, payer, reward) {
                                Ok(msg) => msgs.push(msg),
                                Err(_e) => {
                                    continue;
                                }
                            }
                        }
                        COMMIT_LEDGER.remove(storage, payer);
                        last_processed = Some(payer.clone());
                        processed_count += 1;
                    }
                    Err(_) => {
                        continue;
                    }
                }
            }
            // Update state based on what we actually processed
            let new_remaining = dist_state
                .distributions_remaining
                .saturating_sub(processed_count);

            if new_remaining == 0 {
                // All done - remove state
                DISTRIBUTION_STATE.remove(storage);
            } else if processed_count > 0 {
                // Made progress - update state
                let updated_state = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: last_processed,
                    distributions_remaining: new_remaining,
                    estimated_gas_per_distribution: dist_state.estimated_gas_per_distribution,
                    max_gas_per_tx: dist_state.max_gas_per_tx,
                    last_successful_batch_size: Some(processed_count),
                    consecutive_failures: 0, // Reset failures on success
                    started_at: dist_state.started_at,
                    last_updated: env.block.time, // Update timestamp
                };
                DISTRIBUTION_STATE.save(storage, &updated_state)?;

                // Trigger continuation
                msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: env.contract.address.to_string(),
                    msg: to_json_binary(&ExecuteMsg::ContinueDistribution {})?,
                    funds: vec![],
                }));
            } else {
                // No progress made - increment failure counter
                dist_state.consecutive_failures += 1;

                // Check if we should give up
                if dist_state.consecutive_failures >= 5 {
                    // Too many failures, mark for manual recovery
                    dist_state.is_distributing = false; // Pause distribution
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;

                    return Err(StdError::generic_err(
                        "Distribution failed too many times - manual recovery needed",
                    ));
                } else {
                    // Save with incremented failure count
                    dist_state.last_updated = env.block.time;
                    DISTRIBUTION_STATE.save(storage, &dist_state)?;

                    // Still try to continue (maybe next batch will work)
                    msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
                        contract_addr: env.contract.address.to_string(),
                        msg: to_json_binary(&ExecuteMsg::ContinueDistribution {})?,
                        funds: vec![],
                    }));
                }
            }
        }
        Err(e) => {
            // Failed to even read the batch
            dist_state.consecutive_failures += 1;

            if dist_state.consecutive_failures >= 5 {
                // Give up after too many failures
                dist_state.is_distributing = false;
                DISTRIBUTION_STATE.save(storage, &dist_state)?;

                return Err(StdError::generic_err(format!(
                    "Distribution batch read failed: {}. Manual recovery needed after {} failures",
                    e, dist_state.consecutive_failures
                )));
            } else {
                // Save failure state but try to continue
                DISTRIBUTION_STATE.save(storage, &dist_state)?;

                // Return error but don't completely fail
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
    // Base calculation from gas estimates
    let base_batch_size = if dist_state.estimated_gas_per_distribution == 0 {
        1u32
    } else {
        (dist_state.max_gas_per_tx / dist_state.estimated_gas_per_distribution) as u32
    };

    // If record of successful batch size, use it as reference
    if let Some(last_successful) = dist_state.last_successful_batch_size {
        // Use 90% of last successful to be safe
        let safe_size = (last_successful * 9) / 10;
        base_batch_size.min(safe_size).max(1)
    } else {
        // First run or no history - be conservative
        base_batch_size.min(10).max(1)
    }
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

//converts decimal to decimal256 for higher precision
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
