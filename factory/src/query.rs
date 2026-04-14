use crate::asset::TokenType;
use crate::internal_bluechip_price_oracle::{
    bluechip_to_usd, get_bluechip_usd_price, usd_to_bluechip,
};
use crate::msg::FactoryInstantiateResponse;
use crate::state::{FACTORYINSTANTIATEINFO, POOLS_BY_ID};
use cosmwasm_schema::{cw_serde, QueryResponses};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Binary, Deps, Env, QueryRequest, StdResult, Uint128, WasmQuery,
};
use cw20::{Cw20QueryMsg, TokenInfoResponse};
use pool_factory_interfaces::FactoryQueryMsg;

#[cw_serde]
pub struct CreatorTokenInfoResponse {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub total_supply: Uint128,
    pub token_address: Addr,
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(FactoryInstantiateResponse)]
    Factory {},
    #[returns(CreatorTokenInfoResponse)]
    CreatorTokenInfo { pool_id: u64 },
    #[returns(cosmwasm_std::Binary)]
    InternalBlueChipOracleQuery(FactoryQueryMsg),
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Factory {} => to_json_binary(&query_active_factory(deps)?),
        QueryMsg::CreatorTokenInfo { pool_id } => {
            to_json_binary(&query_creator_token_info(deps, pool_id)?)
        }
        QueryMsg::InternalBlueChipOracleQuery(oracle_msg) => {
            handle_internal_bluechip_oracle_query(deps, env, oracle_msg)
        }
    }
}

pub fn query_creator_token_info(deps: Deps, pool_id: u64) -> StdResult<CreatorTokenInfoResponse> {
    let pool = POOLS_BY_ID.load(deps.storage, pool_id)?;

    let token_addr = pool
        .pool_token_info
        .iter()
        .find_map(|t| match t {
            TokenType::CreatorToken { contract_addr } => Some(contract_addr.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            cosmwasm_std::StdError::generic_err("No creator token found for this pool")
        })?;

    let token_info: TokenInfoResponse =
        deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20QueryMsg::TokenInfo {})?,
        }))?;

    Ok(CreatorTokenInfoResponse {
        name: token_info.name,
        symbol: token_info.symbol,
        decimals: token_info.decimals,
        total_supply: token_info.total_supply,
        token_address: token_addr,
    })
}

pub fn handle_internal_bluechip_oracle_query(
    deps: Deps,
    env: Env,
    msg: FactoryQueryMsg,
) -> StdResult<Binary> {
    match msg {
        FactoryQueryMsg::GetBluechipUsdPrice {} => {
            to_json_binary(&get_bluechip_usd_price(deps, env)?)
        }
        FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
            to_json_binary(&bluechip_to_usd(deps, amount, env)?)
        }
        FactoryQueryMsg::ConvertUsdToBluechip { amount } => {
            to_json_binary(&usd_to_bluechip(deps, amount, env)?)
        }
    }
}

pub fn query_active_factory(deps: Deps) -> StdResult<FactoryInstantiateResponse> {
    let factory = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    Ok(FactoryInstantiateResponse { factory })
}
