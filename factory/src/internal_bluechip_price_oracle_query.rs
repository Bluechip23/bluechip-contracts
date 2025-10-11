use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{to_json_binary,Binary, Deps, Env, StdResult, Uint128};
use crate::internal_bluechip_price_oracle::{bluechip_to_usd, usd_to_bluechip, INTERNAL_ORACLE};
use pool_factory_interfaces::ConversionResponse;

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(OraclePriceResponse)]
    GetOraclePrice {},
    #[returns(OracleStateResponse)]
    GetOracleState {},
    #[returns(ConversionResponse)]
    ConvertBluechipToUsd { amount: Uint128 },
    #[returns(ConversionResponse)]
    ConvertUsdToBluechip { amount: Uint128 },
}

pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetOraclePrice {} => to_json_binary(&query_oracle_price(deps)?),
        //stale vs fresh price
        QueryMsg::GetOracleState {} => to_json_binary(&query_oracle_state(deps)?),
        QueryMsg::ConvertBluechipToUsd { amount } => {
            to_json_binary(&bluechip_to_usd(deps, amount, env)?)
        },
        QueryMsg::ConvertUsdToBluechip { amount } => {
            to_json_binary(&usd_to_bluechip(deps, amount, env)?)
        },
    }
}

pub fn query_oracle_price(deps: Deps) -> StdResult<OraclePriceResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    Ok(OraclePriceResponse {
        price: oracle.bluechip_price_cache.last_price,
        last_update: oracle.bluechip_price_cache.last_update,
        observation_count: oracle.bluechip_price_cache.twap_observations.len() as u32,
    })
}

pub fn query_oracle_state(deps: Deps) -> StdResult<OracleStateResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    Ok(OracleStateResponse {
        selected_pools: oracle.selected_pools,
        last_rotation: oracle.last_rotation,
        rotation_interval: oracle.rotation_interval,
        update_interval: oracle.update_interval,
        twap_price: oracle.bluechip_price_cache.last_price,
        last_update: oracle.bluechip_price_cache.last_update,
        observations: oracle.bluechip_price_cache.twap_observations.len() as u32,
    })
}

#[cw_serde]
pub struct OraclePriceResponse {
    pub price: Uint128,
    pub last_update: u64,
    pub observation_count: u32,
}

#[cw_serde]
pub struct OracleStateResponse {
    pub selected_pools: Vec<String>,
    pub last_rotation: u64,
    pub rotation_interval: u64,
    pub update_interval: u64,
    pub twap_price: Uint128,
    pub last_update: u64,
    pub observations: u32,
}