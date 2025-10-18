use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{to_json_binary, Addr, BalanceResponse, BankQuery, Binary, Deps, Env, QuerierWrapper, QueryRequest, StdError, StdResult, Uint128, WasmQuery};
use cw20::{Cw20QueryMsg, TokenInfoResponse, BalanceResponse as Cw20BalanceResponse};
use pool_factory_interfaces::{FactoryQueryMsg, PoolStateResponseForFactory};
use crate::internal_bluechip_price_oracle::{bluechip_to_usd, get_bluechip_usd_price, usd_to_bluechip};
use crate::pool_struct::PoolDetails;
use crate::msg::FactoryInstantiateResponse;
use crate::pyth_types::{PythQueryMsg, PythPriceFeedResponse};
use crate::state::{FACTORYINSTANTIATEINFO, MAX_PRICE_AGE_SECONDS_BEFORE_STALE, POOLS_BY_CONTRACT_ADDRESS};

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(FactoryInstantiateResponse)]
    Factory {},
    #[returns(PoolDetails)]
    Pool { pool_address: String },
    #[returns(cosmwasm_std::Binary)]
    InternalBlueChipOracleQuery (FactoryQueryMsg),
}

pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Factory {} => to_json_binary(&query_active_factory(deps)?),
        QueryMsg::Pool { pool_address } => to_json_binary(&query_pool(deps, pool_address)?),
        QueryMsg::InternalBlueChipOracleQuery(oracle_msg) => handle_internal_bluechip_oracle_query(deps, env, oracle_msg),
    }
}

pub fn query_pool(deps: Deps, pool_address: String) -> StdResult<PoolStateResponseForFactory> {
    // Validate the address
    let pool_addr = deps.api.addr_validate(&pool_address)?;
    
    // Load from storage - you'll need a map by address
    let pool_details = POOLS_BY_CONTRACT_ADDRESS.load(deps.storage, pool_addr)?;
    Ok(pool_details)
}

// Add oracle query handler
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

pub fn query_pyth_atom_usd_price(deps: Deps, env: Env) -> StdResult<Uint128> {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    
    // Query Pyth for ATOM/USD price
    let query_msg = PythQueryMsg::PythConversionPriceFeed {
        id: config.pyth_atom_usd_price_feed_id.clone(),
    };
    
    let query = QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: config.pyth_contract_addr_for_conversions.to_string(),
        msg: to_json_binary(&query_msg)?,
    });
    
    let response: PythPriceFeedResponse = deps.querier.query(&query)?;
    let price_feed = response.price_feed;
    
    // Check if price is fresh enough
    let current_time = env.block.time.seconds() as i64;
    let price_age = current_time - price_feed.price.publish_time;
    
    if price_age > MAX_PRICE_AGE_SECONDS_BEFORE_STALE as i64 {
        return Err(StdError::generic_err(format!(
            "Price is too stale. Age: {} seconds, Max allowed: {} seconds",
            price_age, MAX_PRICE_AGE_SECONDS_BEFORE_STALE
        )));
    }
    
    // Convert Pyth price to Uint128
    // Pyth uses signed integers with an exponent
    if price_feed.price.price <= 0 {
        return Err(StdError::generic_err("Invalid negative or zero price"));
    }
    
    let price_u128 = price_feed.price.price as u128;
    let expo = price_feed.price.expo;
    
    // Normalize to 6 decimal places (micro-USD)
    // If expo = -8, price has 8 decimals, we want 6
    // If expo = -5, price has 5 decimals, we want 6
    let normalized_price = if expo == -6 {
        Uint128::from(price_u128)
    } else if expo < -6 {
        // More decimals than needed, divide
        let divisor = 10u128.pow((expo.abs() - 6) as u32);
        Uint128::from(price_u128 / divisor)
    } else {
        // Fewer decimals than needed, multiply
        let multiplier = 10u128.pow((6 - expo.abs()) as u32);
        Uint128::from(price_u128 * multiplier)
    };
    
    Ok(normalized_price)
}