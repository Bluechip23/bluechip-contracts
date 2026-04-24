//! Shared primitives used by swap, liquidity, admin, and (via
//! creator-pool `generic_helpers`) the commit flow.
//!
//! Scope: anything that either touches only shared state
//! (POOL_FEE_STATE, POOL_STATE, USER_LAST_COMMIT) or is pure. Commit-
//! phase helpers — `trigger_threshold_payout`, `process_distribution_batch`,
//! `calculate_effective_batch_size`, `update_commit_info`, `mint_tokens`,
//! `validate_pool_threshold_payments` — stay in the creator-pool crate.

use crate::error::ContractError;
use crate::state::{PoolFeeState, PoolSpecs, PoolState, USER_LAST_COMMIT};
use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, Decimal, Decimal256, DepsMut, Env, StdError, StdResult,
    Timestamp, Uint128,
};

/// Update fee growth based on which token was offered.
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
    let transfer_bank_msg = BankMsg::Send {
        to_address: recipient.into(),
        amount: vec![Coin {
            denom: denom.to_string(),
            amount,
        }],
    };
    let transfer_bank_cosmos_msg: CosmosMsg = transfer_bank_msg.into();
    Ok(transfer_bank_cosmos_msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::Timestamp;

    #[test]
    fn decimal2decimal256_preserves_value() {
        let d = Decimal::percent(37);
        let d256 = decimal2decimal256(d).unwrap();
        // Round-trip through string: the "0.37" output should match.
        assert_eq!(d.to_string(), d256.to_string());
    }

    #[test]
    fn decimal2decimal256_zero() {
        let d256 = decimal2decimal256(Decimal::zero()).unwrap();
        assert_eq!(d256.to_string(), "0");
    }

    #[test]
    fn decimal2decimal256_one() {
        let d256 = decimal2decimal256(Decimal::one()).unwrap();
        assert_eq!(d256.to_string(), "1");
    }

    #[test]
    fn enforce_transaction_deadline_no_deadline_always_ok() {
        let current = Timestamp::from_seconds(1_000_000);
        assert!(enforce_transaction_deadline(current, None).is_ok());
    }

    #[test]
    fn enforce_transaction_deadline_future_is_ok() {
        let current = Timestamp::from_seconds(1_000_000);
        let future = Timestamp::from_seconds(1_000_100);
        assert!(enforce_transaction_deadline(current, Some(future)).is_ok());
    }

    #[test]
    fn enforce_transaction_deadline_equal_is_ok() {
        // Equal is NOT expired: the check is `current > dl`.
        let current = Timestamp::from_seconds(1_000_000);
        assert!(enforce_transaction_deadline(current, Some(current)).is_ok());
    }

    #[test]
    fn enforce_transaction_deadline_past_rejects() {
        let current = Timestamp::from_seconds(1_000_000);
        let past = Timestamp::from_seconds(999_999);
        let r = enforce_transaction_deadline(current, Some(past));
        assert!(matches!(r, Err(ContractError::TransactionExpired {})));
    }
}
