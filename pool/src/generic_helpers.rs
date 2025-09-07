#![allow(non_snake_case)]
use crate::asset::{TokenType,};
use crate::error::ContractError;
use crate::msg::CommitFeeInfo;
use crate::state::{
    CommitLimitInfo, PoolFeeState, PoolInfo, PoolSpecs, ThresholdPayoutAmounts,  COMMIT_LEDGER, POOL_FEE_STATE, POOL_STATE, USER_LAST_COMMIT
};
use crate::state::{PoolState};
use cosmwasm_std::{
   to_json_binary, Addr, Coin, CosmosMsg, Decimal, Decimal256,  DepsMut, Env, Order, StdError, StdResult, Storage, Timestamp, Uint128, Uint256, WasmMsg
};
use cw20::{Cw20ExecuteMsg, };
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

pub fn enforce_transaction_deadline(current: Timestamp, transaction_deadline: Option<Timestamp>) -> Result<(), ContractError> {
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
    let mut msgs = Vec::<CosmosMsg>::new();
    let total =
        payout.creator_reward_amount + payout.bluechip_reward_amount + payout.pool_seed_amount + payout.commit_return_amount;

    if total != Uint128::new(1_200_000_000_000) {
        return Err(StdError::generic_err(
            "Threshold payout corruption detected",
        ));
    }

    let creator_ratio = payout.creator_reward_amount.multiply_ratio(100u128, total);
    if creator_ratio < Uint128::new(26) || creator_ratio > Uint128::new(28) {
        return Err(StdError::generic_err("Invalid creator ratio"));
    }
    //mint tokens directly to the desired places
    //to creator
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.creator_address,
        payout.creator_reward_amount,
    )?);
    //to BlueChip
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.bluechip_address,
        payout.bluechip_reward_amount,
    )?);
    //to the creator pool + the amount of bluechips used to cross the threshold
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &env.contract.address,
        payout.pool_seed_amount + commit_config.commit_amount_for_threshold,
    )?);
    //calculate return to pre threshold commiters
    let held_amount = payout.commit_return_amount;
    //find each payer inside the ledger
    for payer_res in COMMIT_LEDGER.keys(storage, None, None, Order::Ascending) {
        let payer: Addr = payer_res?;
        //how much they commited
        let usd_paid = COMMIT_LEDGER.load(storage, &payer)?;
        let reward = Uint128::try_from(
            (Uint256::from(usd_paid) * Uint256::from(held_amount))
                / Uint256::from(commit_config.commit_amount_for_threshold_usd),
        )?;

        if !reward.is_zero() {
            msgs.push(mint_tokens(&pool_info.token_address, &payer, reward)?);
        }
    }
    COMMIT_LEDGER.clear(storage);

    let denom = match &pool_info.pool_info.asset_infos[0] {
        TokenType::Bluechip { denom, .. } => denom,
        _ => "stake", // fallback if first asset isn't bluechip
    };
    //mint and push amount to each pre threshold commiter based on their portion of the "bluechip seed"
    let bluechip_seed = Uint128::new(23_500);
    msgs.push(get_bank_transfer_to_msg(
        &env.contract.address,
        denom,
        bluechip_seed,
    )?);

    pool_state.reserve0 = bluechip_seed; // No LP positions created yet
    pool_state.reserve1 = payout.pool_seed_amount; // No LP positions created yet
    //Initial seed liquidity is not owned by anyone and cannot be withdrawn. This is intentional to prevent pool draining attacks and unneccesary pool rewards
    pool_state.total_liquidity = Uint128::zero();
    pool_fee_state.fee_growth_global_0 = Decimal::zero();
    pool_fee_state.fee_growth_global_1 = Decimal::zero();
    pool_fee_state.total_fees_collected_0 = Uint128::zero();
    pool_fee_state.total_fees_collected_1 = Uint128::zero();

    POOL_STATE.save(storage, pool_state)?;
    POOL_FEE_STATE.save(storage, pool_fee_state)?;

    Ok(msgs)
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