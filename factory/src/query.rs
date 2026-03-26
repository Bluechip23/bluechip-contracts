use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{entry_point, to_json_binary, Addr, BalanceResponse, BankQuery, Binary, Deps, Env, QuerierWrapper, QueryRequest, StdResult, Uint128, WasmQuery};
use cw20::{Cw20QueryMsg, TokenInfoResponse, BalanceResponse as Cw20BalanceResponse};
use pool_factory_interfaces::{FactoryQueryMsg, PoolStateResponseForFactory};
use crate::internal_bluechip_price_oracle::{bluechip_to_usd, get_bluechip_usd_price, usd_to_bluechip};
use crate::pool_struct::PoolDetails;
use crate::msg::FactoryInstantiateResponse;
use crate::state::{FACTORYINSTANTIATEINFO, PENDING_CONFIG, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, PendingConfig};
use crate::asset::TokenType;

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
    #[returns(PoolDetails)]
    Pool { pool_address: String },
    #[returns(CreatorTokenInfoResponse)]
    CreatorTokenInfo { pool_id: u64 },
    #[returns(cosmwasm_std::Binary)]
    InternalBlueChipOracleQuery (FactoryQueryMsg),
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Factory {} => to_json_binary(&query_active_factory(deps)?),
        QueryMsg::Pool { pool_address } => to_json_binary(&query_pool(deps, pool_address)?),
        QueryMsg::CreatorTokenInfo { pool_id } => to_json_binary(&query_creator_token_info(deps, pool_id)?),
        QueryMsg::InternalBlueChipOracleQuery(oracle_msg) => handle_internal_bluechip_oracle_query(deps, env, oracle_msg),
    }
}

pub fn query_pool(deps: Deps, pool_address: String) -> StdResult<PoolStateResponseForFactory> {
    let pool_addr = deps.api.addr_validate(&pool_address)?;

    let pool_details = POOLS_BY_CONTRACT_ADDRESS.load(deps.storage, pool_addr)?;
    Ok(pool_details)
}

pub fn query_creator_token_info(deps: Deps, pool_id: u64) -> StdResult<CreatorTokenInfoResponse> {
    let pool = POOLS_BY_ID.load(deps.storage, pool_id)?;

    // Find the creator token address from the pool's token info
    let token_addr = pool
        .pool_token_info
        .iter()
        .find_map(|t| match t {
            TokenType::CreatorToken { contract_addr } => Some(contract_addr.clone()),
            _ => None,
        })
        .ok_or_else(|| cosmwasm_std::StdError::generic_err("No creator token found for this pool"))?;

    // Query the CW20 token contract for its metadata
    let token_info: TokenInfoResponse = deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
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

pub fn handle_internal_bluechip_oracle_query(deps: Deps, env: Env, msg: FactoryQueryMsg) -> StdResult<Binary> {
    match msg {
        FactoryQueryMsg::GetBluechipUsdPrice {} => {
            to_json_binary(&get_bluechip_usd_price(deps, env)?)
        },
        FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
            to_json_binary(&bluechip_to_usd(deps, amount, env)?)
        },
        FactoryQueryMsg::ConvertUsdToBluechip { amount } => {
            to_json_binary(&usd_to_bluechip(deps, amount, env)?)
        },
    }
    
}

pub fn query_active_factory(deps: Deps) -> StdResult<FactoryInstantiateResponse> {
    let factory = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    Ok(FactoryInstantiateResponse { factory })
}

pub fn query_token_balance(
    querier: &QuerierWrapper,
    contract_addr: Addr,
    account_addr: Addr,
) -> StdResult<Uint128> {
    // load balance from the token contract
    let res: Cw20BalanceResponse = querier
        .query(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: String::from(contract_addr),
            msg: to_json_binary(&Cw20QueryMsg::Balance {
                address: String::from(account_addr),
            })?,
        }))
        .unwrap_or_else(|_| Cw20BalanceResponse {
            balance: Uint128::zero(),
        });

    Ok(res.balance)
}

pub fn query_token_ticker(querier: &QuerierWrapper, contract_addr: Addr) -> StdResult<String> {
    let res: TokenInfoResponse = querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: String::from(contract_addr),
        msg: to_json_binary(&Cw20QueryMsg::TokenInfo {})?,
    }))?;

    Ok(res.symbol)
}

pub fn query_pending_config(deps: Deps) -> StdResult<Option<PendingConfig>> {
    PENDING_CONFIG.may_load(deps.storage)
}

pub fn query_balance(
    querier: &QuerierWrapper,
    account_addr: Addr,
    denom: String,
) -> StdResult<Uint128> {
    let balance: BalanceResponse = querier.query(&QueryRequest::Bank(BankQuery::Balance {
        address: String::from(account_addr),
        denom,
    }))?;
    Ok(balance.amount.amount)
}

// Pyth ATOM/USD price queries are handled exclusively through the oracle
// module (internal_bluechip_price_oracle::query_pyth_atom_usd_price) which
// includes full validation: staleness, confidence interval, and exponent range.