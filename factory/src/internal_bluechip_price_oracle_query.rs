// `query_oracle_price` and `query_oracle_state` both deserialize the
// entire `BlueChipPriceInternalOracle` even when only a few fields are
// read. The `INTERNAL_ORACLE` Item lives in a single storage slot so
// per-field accessors would not avoid the deserialization cost; if
// query gas ever becomes a concern, the right shape is to split the
// type into `INTERNAL_ORACLE_LIGHTWEIGHT` (price cache only) and
// `INTERNAL_ORACLE_HEAVY` (selected pools, snapshots) backed by two
// `Item`s, which is a migration-bearing change rather than a local
// refactor.

use crate::internal_bluechip_price_oracle::{bluechip_to_usd, usd_to_bluechip, INTERNAL_ORACLE};
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{to_json_binary, Binary, Deps, Env, StdResult, Uint128};
// Referenced only by `#[returns(ConversionResponse)]` annotations on the
// `QueryMsg` variants below. cosmwasm-schema's `QueryResponses` derive
// reads it when the `schema` feature is active; the wasm release build
// drops the derive and sees the import as unused. Per-import allow is
// the convention shared with `standard-pool/src/msg.rs` — see that
// file for the cargo-fix safety note.
#[allow(unused_imports)]
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
        QueryMsg::GetOracleState {} => to_json_binary(&query_oracle_state(deps)?),
        QueryMsg::ConvertBluechipToUsd { amount } => {
            to_json_binary(&bluechip_to_usd(deps, amount, &env)?)
        }
        QueryMsg::ConvertUsdToBluechip { amount } => {
            to_json_binary(&usd_to_bluechip(deps, amount, &env)?)
        }
    }
}

pub fn query_oracle_price(deps: Deps) -> StdResult<OraclePriceResponse> {
    let oracle = INTERNAL_ORACLE.load(deps.storage)?;
    Ok(OraclePriceResponse {
        twap_price: oracle.bluechip_price_cache.last_price,
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
        observation_count: oracle.bluechip_price_cache.twap_observations.len() as u32,
    })
}

/// Single round's published oracle price + freshness metadata. The
/// `twap_price` and `observation_count` field names are aligned with
/// [`OracleStateResponse`] so consumers can use the same parser for
/// both response shapes (the state response just adds rotation /
/// pool-set fields on top).
#[cw_serde]
pub struct OraclePriceResponse {
    pub twap_price: Uint128,
    pub last_update: u64,
    pub observation_count: u32,
}

/// Full oracle state — superset of [`OraclePriceResponse`] plus the
/// rotation schedule and the currently-sampled pool set.
#[cw_serde]
pub struct OracleStateResponse {
    pub selected_pools: Vec<String>,
    pub last_rotation: u64,
    pub rotation_interval: u64,
    pub update_interval: u64,
    pub twap_price: Uint128,
    pub last_update: u64,
    pub observation_count: u32,
}
