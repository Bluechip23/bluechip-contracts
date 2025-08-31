use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{to_json_binary, Addr, BankQuery, Deps, QuerierWrapper, QueryRequest, StdResult, Uint128, WasmQuery, BalanceResponse};
use cw20::{Cw20QueryMsg, TokenInfoResponse, BalanceResponse as Cw20BalanceResponse};
use crate::pool::PoolDetails;
use crate::msg::FactoryInstantiateResponse;
use crate::state::FACTORYINSTANTIATEINFO;

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(FactoryInstantiateResponse)]
    Factory {},
    #[returns(PoolDetails)]
    Pool {},
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
