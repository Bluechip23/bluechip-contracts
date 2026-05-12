//! CosmWasm entry points + dispatcher. The bulk of the handler logic
//! lives in topic-aligned submodules:
//!
//!   - [`crate::expand`]        — the `RequestExpansion` flow, decomposed
//!                                into one helper per phase.
//!   - [`crate::timelock`]      — propose / apply / cancel for both
//!                                config and withdrawal (48h timelock).
//!   - [`crate::migrate`]       — migrate handler + downgrade guard.
//!   - [`crate::denom`]         — cosmos-sdk denom format validator.
//!   - [`crate::factory_query`] — wire types + cross-validation helper.
//!   - [`crate::helpers`]       — shared owner-gating + storage helpers.
//!   - [`crate::attrs`]         — every attribute key / action value.

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError,
};
use cw2::set_contract_version;

use crate::attrs;
use crate::denom::validate_native_denom;
use crate::error::ContractError;
use crate::expand::execute_expand_economy;
use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};
use crate::state::{Config, CONFIG, DEFAULT_BLUECHIP_DENOM};
use crate::timelock::{
    execute_apply_config_update, execute_cancel_config_update, execute_cancel_withdrawal,
    execute_propose_config_update, execute_propose_withdrawal, execute_withdrawal,
};
use crate::{CONTRACT_NAME, CONTRACT_VERSION};

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let bluechip_denom = msg
        .bluechip_denom
        .unwrap_or_else(|| DEFAULT_BLUECHIP_DENOM.to_string());
    // Validate against the cosmos-sdk denom format rules at
    // instantiate so a typo'd denom fails here rather than 48 hours
    // later via the timelocked propose / apply path.
    validate_native_denom(&bluechip_denom)?;

    let config = Config {
        factory_address: deps.api.addr_validate(&msg.factory_address)?,
        owner: deps
            .api
            .addr_validate(&msg.owner.unwrap_or_else(|| info.sender.to_string()))?,
        bluechip_denom,
    };

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute(attrs::ACTION, attrs::INSTANTIATE)
        .add_attribute(attrs::FACTORY, &config.factory_address)
        .add_attribute(attrs::OWNER, &config.owner)
        .add_attribute(attrs::BLUECHIP_DENOM, &config.bluechip_denom))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    // Every execute path on this contract is non-payable.
    //   - `RequestExpansion` is sent by the factory with funds: vec![];
    //     attaching coins inflates the contract's bank balance without
    //     adding an entry to EXPANSION_LOG, biasing the cap accounting.
    //   - All Propose / Execute / Cancel timelock arms have no semantic
    //     reason to accept funds; attached funds would be orphaned in
    //     the contract's bank balance until rescued via the 48h
    //     ProposeWithdrawal flow.
    //
    // Centralised at the dispatch top so every existing AND every
    // future variant inherits the guard without per-arm boilerplate.
    cw_utils::nonpayable(&info)?;
    match msg {
        ExecuteMsg::ExpandEconomy(expand_economy_msg) => {
            execute_expand_economy(deps, env, info, expand_economy_msg)
        }
        ExecuteMsg::ProposeConfigUpdate {
            factory_address,
            owner,
            bluechip_denom,
        } => execute_propose_config_update(deps, env, info, factory_address, owner, bluechip_denom),
        ExecuteMsg::ExecuteConfigUpdate {} => execute_apply_config_update(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_config_update(deps, info),
        ExecuteMsg::ProposeWithdrawal {
            amount,
            denom,
            recipient,
        } => execute_propose_withdrawal(deps, env, info, amount, denom, recipient),
        ExecuteMsg::ExecuteWithdrawal {} => execute_withdrawal(deps, env, info),
        ExecuteMsg::CancelWithdrawal {} => execute_cancel_withdrawal(deps, info),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> Result<Binary, ContractError> {
    match msg {
        QueryMsg::GetConfig {} => to_json_binary(&query_config(deps)?).map_err(map_std),
        QueryMsg::GetBalance { denom } => {
            // Pass `&env.contract.address` (a `&Addr` reference) consistently
            // with the rest of this crate's `query_balance` call sites.
            to_json_binary(&deps.querier.query_balance(&env.contract.address, denom)?)
                .map_err(map_std)
        }
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(
    deps: DepsMut,
    env: Env,
    msg: MigrateMsg,
) -> Result<Response, ContractError> {
    crate::migrate::migrate(deps, env, msg)
}

fn query_config(deps: Deps) -> Result<ConfigResponse, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        factory_address: config.factory_address,
        owner: config.owner,
        bluechip_denom: config.bluechip_denom,
    })
}

#[inline]
fn map_std(e: StdError) -> ContractError {
    ContractError::Std(e)
}

// ---------------------------------------------------------------------------
// Test-only re-exports
// ---------------------------------------------------------------------------
//
// The test suite needs to exercise `validate_native_denom` and the
// factory-response subset structs directly. Both are crate-private in
// production; expose them through a `cfg(test)` module rather than
// weakening their visibility for the live build.

#[cfg(test)]
pub mod testing {
    use crate::error::ContractError;
    use serde::Deserialize;

    /// Re-export of the private subset struct so tests can deserialize
    /// a synthetic factory response directly.
    #[derive(Deserialize)]
    pub struct FactoryConfigSubsetForTest {
        pub bluechip_denom: String,
    }

    #[derive(Deserialize)]
    pub struct FactoryInstantiateResponseSubsetForTest {
        pub factory: FactoryConfigSubsetForTest,
    }

    /// Re-export of the validator for unit tests.
    pub fn validate_native_denom_for_test(denom: &str) -> Result<(), ContractError> {
        crate::denom::validate_native_denom(denom)
    }
}
