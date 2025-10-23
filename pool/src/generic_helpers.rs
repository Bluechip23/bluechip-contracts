#![allow(non_snake_case)]
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg};
use crate::state::{DistributionState, PoolState, DISTRIBUTION_STATE, MAX_DISTRIBUTIONS_PER_TX};
use crate::state::{
    CommitLimitInfo, PoolFeeState, PoolInfo, PoolSpecs, ThresholdPayoutAmounts, COMMIT_LEDGER,
    POOL_FEE_STATE, POOL_STATE, USER_LAST_COMMIT,
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
        // Token0 offered, fees collected in token0
        pool_fee_state.fee_growth_global_0 += fee_growth;
        pool_fee_state.total_fees_collected_0 += commission_amt;
    } else {
        // Token1 offered, fees collected in token1
        pool_fee_state.fee_growth_global_1 += fee_growth;
        pool_fee_state.total_fees_collected_1 += commission_amt;
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

    // Validate total payout integrity
    let total = payout.creator_reward_amount
        + payout.bluechip_reward_amount
        + payout.pool_seed_amount
        + payout.commit_return_amount;

    if total != Uint128::new(1_200_000_000_000) {
        return Err(StdError::generic_err("Threshold payout corruption detected"));
    }

    // --- Single transfers (creator, bluechip, pool seed) ---
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
        payout.pool_seed_amount + commit_config.commit_amount_for_threshold,
    )?);

    // --- Committer payouts ---
    let total_committers = COMMIT_LEDGER
        .keys(storage, None, None, Order::Ascending)
        .count();

    if total_committers == 0 {
        // No committers to pay — nothing to do
    } else if total_committers <= MAX_DISTRIBUTIONS_PER_TX as usize {
        // Collect all committers first (immutable borrow)
        let committers: Vec<(Addr, Uint128)> = COMMIT_LEDGER
            .range(storage, None, None, Order::Ascending)
            .map(|r| r.map_err(|e| StdError::generic_err(e.to_string())))
            .collect::<StdResult<Vec<_>>>()?;

        // Now safely mutate storage
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
        let dist_state = DistributionState {
            is_distributing: true,
            total_to_distribute: payout.commit_return_amount,
            total_committed_usd: commit_config.commit_amount_for_threshold_usd,
            last_processed_key: None,
            distributions_remaining: total_committers as u32,
        };
        DISTRIBUTION_STATE.save(storage, &dist_state)?;

        // Process first batch immediately
        let batch_msgs = process_distribution_batch(
            storage,
            pool_info,
            env,
            MAX_DISTRIBUTIONS_PER_TX,
        )?;
        msgs.extend(batch_msgs);
    }

    // --- Update pool and fee state ---
    let total_fee_rate = fee_info.commit_fee_bluechip + fee_info.commit_fee_creator;
    let pools_bluechip_seed =
        commit_config.commit_amount_for_threshold * (Decimal::one() - total_fee_rate);

    pool_state.reserve0 = pools_bluechip_seed;
    pool_state.reserve1 = payout.pool_seed_amount;
    pool_state.total_liquidity = Uint128::zero();

    // Reset fee growth and collection tracking
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
    batch_size: u32,
) -> StdResult<Vec<CosmosMsg>> {
    let mut msgs = Vec::new();
    let dist_state = DISTRIBUTION_STATE.load(storage)?;
    
    // Determine starting point
    let start_after = dist_state.last_processed_key
        .as_ref()
        .map(|addr| Bound::exclusive(addr));
    
    // Process batch
    let batch: Vec<_> = COMMIT_LEDGER
        .range(storage, start_after, None, Order::Ascending)
        .take(batch_size as usize)
        .collect::<StdResult<Vec<_>>>()?;
    
    let actual_batch_size = batch.len() as u32;
    let mut last_processed = None;
    
    for (payer, usd_paid) in batch {
        let reward = calculate_committer_reward(
            usd_paid,
            dist_state.total_to_distribute,
            dist_state.total_committed_usd,
        )?;
        
        if !reward.is_zero() {
            msgs.push(mint_tokens(&pool_info.token_address, &payer, reward)?);
        }
        
        COMMIT_LEDGER.remove(storage, &payer);
        last_processed = Some(payer);
    }
    
    // Update state
    let new_remaining = dist_state.distributions_remaining
        .saturating_sub(actual_batch_size);
    
    if new_remaining == 0 {
        // All done
        DISTRIBUTION_STATE.remove(storage);
    } else {
        // Save progress
        let updated_state = DistributionState {
            is_distributing: true,
            total_to_distribute: dist_state.total_to_distribute,
            total_committed_usd: dist_state.total_committed_usd,
            last_processed_key: last_processed,
            distributions_remaining: new_remaining,
        };
        DISTRIBUTION_STATE.save(storage, &updated_state)?;
        
        // Trigger continuation
        msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&ExecuteMsg::ContinueDistribution {})?,
            funds: vec![],
        }));
    }
    
    Ok(msgs)
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
            .checked_div(Uint256::from(total_committed_usd))?
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

// creates a bank transfer message for sending bluechip tokens
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
    //mint the tokens and send them to the correct contract witht he correct amounts
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
