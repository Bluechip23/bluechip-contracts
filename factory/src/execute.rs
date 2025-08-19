use std::env;

use crate::error::ContractError;
use crate::msg::{ ExecuteMsg,};
use crate::state::{
    FactoryInstantiate, CONFIG, NEXT_POOL_ID,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    Deps, DepsMut, Env, MessageInfo,  Response,
    StdError, StdResult, 
};



const CONTRACT_NAME: &str = "bluechip_factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");


#[cfg_attr(not(feature = "library"), entry_point)]
//Pools use the factory as almost a launch pad. I guess the best way to think of it is as a literal factory. 
//it creates a template and holds logic for each new pool and gives the newly created pool new abilities like minting rights and other things. 
//It takes in parameters set by a json file once, so it becomes easy to set standards across all pools. Basically making it very repeatable.
//the factory is also the central entity pools can potentially recieve upgrades from. factory = bill gates pools = pcs running on windows.


//instantiates the factory for pools to use. 
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    //saves the factory parameters set in the json file
    CONFIG.save(deps.storage, &msg)?;
    //sets the first pool created by this factory to 1
    NEXT_POOL_ID.save(deps.storage, &1u64)?;
    //viola
    Ok(Response::new().add_attribute("action", "init_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        //edit factory parameters - only bluechip can - does not touch existing pools unless we do a chain wide change
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
}
}
//make sure the factory sent the message to instantiate the pool
fn assert_is_admin(deps: Deps, info: MessageInfo) -> StdResult<bool> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.admin {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.admin, info.sender
        )));
    }

    Ok(true)
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info)?;

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}
