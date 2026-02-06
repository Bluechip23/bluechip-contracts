use crate::{
    error::ContractError,
    state::{FACTORYINSTANTIATEINFO, FIRST_POOL_TIMESTAMP},
};
use cosmwasm_std::{CosmosMsg, DepsMut, Env, StdResult, Uint128};

pub fn calculate_mint_amount(seconds_elapsed: u64, pools_created: u64) -> StdResult<Uint128> {
    pool_factory_interfaces::calculate_mint_amount(seconds_elapsed, pools_created)
}

pub fn calculate_and_mint_bluechip(
    deps: &mut DepsMut,
    env: Env,
    pool_count: u64,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let first_pool_time = match FIRST_POOL_TIMESTAMP.may_load(deps.storage)? {
        Some(time) => time,
        None => {
            FIRST_POOL_TIMESTAMP.save(deps.storage, &env.block.time)?;
            env.block.time
        }
    };

    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    // let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    // if config.atom_bluechip_anchor_pool_address == config.factory_admin_address {
    //     return Ok(vec![]);
    // }

    let seconds_elapsed = env.block.time.seconds() - first_pool_time.seconds();
    let mint_amount = calculate_mint_amount(seconds_elapsed, pool_count)?;

    if mint_amount.is_zero() {
        return Ok(vec![]);
    }

    let Some(expand_economy_contract) = config.bluechip_mint_contract_address else {
        return Ok(vec![]);
    };

    Ok(vec![CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
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
    })])
}
