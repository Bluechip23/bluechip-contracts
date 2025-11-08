use crate::{
    error::ContractError,
    state::{FACTORYINSTANTIATEINFO, FIRST_POOL_TIMESTAMP},
};
use cosmwasm_std::{BankMsg, Coin, CosmosMsg, DepsMut, Env, StdError, StdResult, Uint128};

pub fn calculate_mint_amount(seconds_elapsed: u64, pools_created: u64) -> StdResult<Uint128> {
    // Formula: 500 - (((5x * x^2) / ((s/6) + 333x))

    let x = pools_created as u128;
    let s = seconds_elapsed as u128;

    let five_x_squared = 5u128
        .checked_mul(x)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator"))?
        .checked_mul(x)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator"))?;

    let numerator = five_x_squared
        .checked_add(x)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator addition"))?;
    //number of bluechips minted by chain since first pool creation
    let s_div_6 = s / 6;
    let denominator = s_div_6
        .checked_add(
            333u128
                .checked_mul(x)
                .ok_or_else(|| StdError::generic_err("Overflow in denominator"))?,
        )
        .ok_or_else(|| StdError::generic_err("Overflow in denominator"))?;

    if denominator == 0 {
        return Ok(Uint128::new(500_000_000));
    }

    let division_result = numerator / denominator;

    let base_amount = 500_000_000u128;

    if division_result >= base_amount {
        return Ok(Uint128::zero());
    }

    Ok(Uint128::new(base_amount - division_result))
}

pub fn calculate_and_mint_bluechip(
    deps: &mut DepsMut,
    env: Env,
    pool_count: u64,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut messages = vec![];

    let first_pool_time = match FIRST_POOL_TIMESTAMP.may_load(deps.storage)? {
        Some(time) => time,
        None => {
            FIRST_POOL_TIMESTAMP.save(deps.storage, &env.block.time)?;
            env.block.time
        }
    };

    let seconds_elapsed = env.block.time.seconds() - first_pool_time.seconds();

    let mint_amount = calculate_mint_amount(seconds_elapsed, pool_count)?;

    if !mint_amount.is_zero() {
        let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

        messages.push(CosmosMsg::Bank(BankMsg::Send {
            to_address: config.bluechip_wallet_address.to_string(),
            amount: vec![Coin {
                denom: "bluechip".to_string(),
                amount: mint_amount,
            }],
        }));
    }

    Ok(messages)
}
