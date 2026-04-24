use crate::asset::TokenType;
use crate::internal_bluechip_price_oracle::{
    bluechip_to_usd, get_bluechip_usd_price, usd_to_bluechip,
};
use crate::msg::FactoryInstantiateResponse;
use crate::state::{
    CreationStatus, DISTRIBUTION_BOUNTY_USD, FACTORYINSTANTIATEINFO, ORACLE_UPDATE_BOUNTY_USD,
    POOLS_BY_ID, POOL_CREATION_CONTEXT,
};
use cosmwasm_schema::{cw_serde, QueryResponses};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Binary, Deps, Env, QueryRequest, StdResult, Timestamp, Uint128,
    WasmQuery,
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
pub struct BountyResponse {
    /// Configured bounty in USD (6 decimals: 1_000_000 = $1.00).
    pub bounty_usd: Uint128,
}

/// Per-pool creation diagnostics. Useful for off-chain tooling that watches
/// for stuck or repeatedly-failing pool creations and surfaces them to
/// operators. Returns `None` when the pool's creation state was already
/// cleaned up (i.e. creation succeeded end-to-end).
#[cw_serde]
pub struct PoolCreationStatusResponse {
    pub pool_id: u64,
    pub creator: Addr,
    pub creator_token_address: Option<Addr>,
    pub mint_new_position_nft_address: Option<Addr>,
    pub pool_address: Option<Addr>,
    pub creation_time: Timestamp,
    pub status: CreationStatus,
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
    #[returns(BountyResponse)]
    OracleUpdateBounty {},
    #[returns(BountyResponse)]
    DistributionBounty {},
    /// Returns the in-flight creation status for a given pool_id, or None
    /// when creation completed cleanly and the entry was reaped.
    #[returns(Option<PoolCreationStatusResponse>)]
    PoolCreationStatus { pool_id: u64 },
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
        QueryMsg::OracleUpdateBounty {} => to_json_binary(&query_oracle_update_bounty(deps)?),
        QueryMsg::DistributionBounty {} => to_json_binary(&query_distribution_bounty(deps)?),
        QueryMsg::PoolCreationStatus { pool_id } => {
            to_json_binary(&query_pool_creation_status(deps, pool_id)?)
        }
    }
}

pub fn query_pool_creation_status(
    deps: Deps,
    pool_id: u64,
) -> StdResult<Option<PoolCreationStatusResponse>> {
    let ctx = match POOL_CREATION_CONTEXT.may_load(deps.storage, pool_id)? {
        Some(c) => c,
        None => return Ok(None),
    };
    let crate::state::PoolCreationContext { temp, state } = ctx;
    Ok(Some(PoolCreationStatusResponse {
        pool_id: state.pool_id,
        creator: state.creator,
        // Prefer ctx.temp (the single source of truth for new contexts);
        // fall back to ctx.state for pre-consolidation records that still
        // carry the mirror field.
        creator_token_address: temp
            .creator_token_addr
            .or(state.creator_token_address),
        mint_new_position_nft_address: temp
            .nft_addr
            .or(state.mint_new_position_nft_address),
        pool_address: state.pool_address,
        creation_time: state.creation_time,
        status: state.status,
    }))
}

pub fn query_oracle_update_bounty(deps: Deps) -> StdResult<BountyResponse> {
    let bounty_usd = ORACLE_UPDATE_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();
    Ok(BountyResponse { bounty_usd })
}

pub fn query_distribution_bounty(deps: Deps) -> StdResult<BountyResponse> {
    let bounty_usd = DISTRIBUTION_BOUNTY_USD
        .may_load(deps.storage)?
        .unwrap_or_default();
    Ok(BountyResponse { bounty_usd })
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
            to_json_binary(&get_bluechip_usd_price(deps, &env)?)
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
