use crate::{
    error::ContractError,
    state::{FACTORYINSTANTIATEINFO, FIRST_POOL_TIMESTAMP},
};
use cosmwasm_std::{BankMsg, Coin, CosmosMsg, DepsMut, Env, StdError, StdResult, Uint128};

pub fn calculate_mint_amount(seconds_elapsed: u64, pools_created: u64) -> StdResult<Uint128> {
    // Formula: 500 - (((5x^2 + x) / ((s/6) + 333x))

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

    // Scale numerator by 1_000_000 before dividing to preserve fractional precision,
    // since the formula produces values in the 0..500 range but base_amount is 500_000_000.
    let scaled_numerator = numerator
        .checked_mul(1_000_000)
        .ok_or_else(|| StdError::generic_err("Overflow in scaled numerator"))?;

    let division_result = scaled_numerator / denominator;

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
    let messages = vec![];

    // Still track the first pool timestamp for future use
    let first_pool_time = match FIRST_POOL_TIMESTAMP.may_load(deps.storage)? {
        Some(time) => time,
        None => {
            FIRST_POOL_TIMESTAMP.save(deps.storage, &env.block.time)?;
            env.block.time
        }
    };

    // Check Mock/Direct Mode - Skip minting if enabled
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    if config.atom_bluechip_anchor_pool_address == config.factory_admin_address {
        return Ok(messages);
    }

    let seconds_elapsed = env.block.time.seconds() - first_pool_time.seconds();

    let mint_amount = calculate_mint_amount(seconds_elapsed, pool_count)?;
    let mut msgs = Vec::new();

    if !mint_amount.is_zero() {
        if let Some(expand_economy_contract) = config.bluechip_mint_contract_address {
            msgs.push(CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
                contract_addr: expand_economy_contract.to_string(),
                msg: cosmwasm_std::to_json_binary(
                    &pool_factory_interfaces::ExpandEconomyExecuteMsg::ExpandEconomy(
                        pool_factory_interfaces::ExpandEconomyMsg::RequestExpansion {
                            recipient: config.bluechip_wallet_address.to_string(),
                            amount: mint_amount,
                        },
                    ),
                )?,
                funds: vec![],
            }));
        } else {
            msgs.push(CosmosMsg::Bank(BankMsg::Send {
                to_address: config.bluechip_wallet_address.to_string(),
                amount: vec![Coin {
                    denom: "ubluechip".to_string(),
                    amount: mint_amount,
                }],
            }));
        }
        return Ok(msgs);
    }

    Ok(messages)
}
