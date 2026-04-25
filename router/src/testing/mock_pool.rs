//! Test-only mock pool contract.
//!
//! Implements just enough of the bluechip pool surface for the router
//! to interact with: native and CW20 swap entry points, and the three
//! queries the router consumes (`Pair`, `IsFullyCommited`, `Simulation`).
//! XYK math runs live against the pool's actual bank/cw20 balances so
//! reserve state cannot drift from on-chain truth.
//!
//! This contract is not part of the production router build -- it lives
//! under `#[cfg(test)]` solely so the integration tests can stand up
//! pools without dragging the entire factory + oracle + threshold flow
//! into every test.

#![allow(clippy::too_many_arguments)]

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, BankMsg, Binary, Coin, CosmosMsg, Decimal, Deps,
    DepsMut, Empty, Env, MessageInfo, Response, StdError, StdResult, Timestamp, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use cw_storage_plus::Item;
use pool_factory_interfaces::asset::{query_pools, PoolPairType, TokenInfo, TokenType};

#[cw_serde]
pub struct MockPoolState {
    pub asset_infos: [TokenType; 2],
    pub fully_committed: bool,
}

const STATE: Item<MockPoolState> = Item::new("mock_pool_state");

#[cw_serde]
pub struct InstantiateMsg {
    pub asset_infos: [TokenType; 2],
    pub fully_committed: bool,
}

/// JSON-compatible with `pool_factory_interfaces::routing::PoolSwapExecuteMsg`
/// (the `SimpleSwap` variant) plus a `Receive` variant for cw20-offered swaps.
#[cw_serde]
pub enum ExecuteMsg {
    SimpleSwap {
        offer_asset: TokenInfo,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
    Receive(Cw20ReceiveMsg),
}

/// Body of `cw20::Send.msg` -- JSON-compatible with
/// `pool_factory_interfaces::routing::PoolSwapCw20HookMsg::Swap`.
#[cw_serde]
pub enum HookMsg {
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
        transaction_deadline: Option<Timestamp>,
    },
}

/// JSON-compatible with `pool_factory_interfaces::routing::PoolSwapQueryMsg`.
#[cw_serde]
pub enum QueryMsg {
    Pair {},
    Simulation { offer_asset: TokenInfo },
    IsFullyCommited {},
}

#[cw_serde]
pub struct PairResponse {
    pub asset_infos: [TokenType; 2],
    pub contract_addr: Addr,
    pub pair_type: PoolPairType,
    pub assets: [TokenInfo; 2],
}

#[cw_serde]
pub struct SimulationResponse {
    pub return_amount: Uint128,
    pub spread_amount: Uint128,
    pub commission_amount: Uint128,
}

#[cw_serde]
pub enum CommitStatus {
    InProgress { raised: Uint128, target: Uint128 },
    FullyCommitted,
}

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    STATE.save(
        deps.storage,
        &MockPoolState {
            asset_infos: msg.asset_infos,
            fully_committed: msg.fully_committed,
        },
    )?;
    Ok(Response::new())
}

#[entry_point]
pub fn execute(deps: DepsMut, env: Env, info: MessageInfo, msg: ExecuteMsg) -> StdResult<Response> {
    match msg {
        ExecuteMsg::SimpleSwap {
            offer_asset, to, ..
        } => execute_native_swap(deps, env, info, offer_asset, to),
        ExecuteMsg::Receive(cw20_msg) => execute_cw20_swap(deps, env, info, cw20_msg),
    }
}

fn execute_native_swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    offer_asset: TokenInfo,
    to: Option<String>,
) -> StdResult<Response> {
    let state = STATE.load(deps.storage)?;
    if !state.fully_committed {
        return Err(StdError::generic_err("pool is in commit phase"));
    }

    let denom = match &offer_asset.info {
        TokenType::Native { denom } => denom.clone(),
        TokenType::CreatorToken { .. } => {
            return Err(StdError::generic_err(
                "SimpleSwap requires a native offer; use cw20 Send for token swaps",
            ));
        }
    };

    let funds_amount = info
        .funds
        .iter()
        .find(|c| c.denom == denom)
        .map(|c| c.amount)
        .unwrap_or_default();
    if funds_amount != offer_asset.amount {
        return Err(StdError::generic_err(format!(
            "funds amount {} does not match offer_asset amount {}",
            funds_amount, offer_asset.amount
        )));
    }

    // info.funds are already credited to the contract by the time execute()
    // runs, so the live balance includes the deposit. Subtract it back out
    // to get the pre-swap reserve.
    let (offer_reserve, ask_reserve, ask_info) =
        load_reserves(deps.as_ref(), &env, &state.asset_infos, &offer_asset.info)?;
    let offer_reserve = offer_reserve.checked_sub(offer_asset.amount).map_err(|_| {
        StdError::generic_err("offer reserve underflow; pool not seeded with declared funds")
    })?;

    let recipient = match to {
        Some(t) => deps.api.addr_validate(&t)?,
        None => info.sender.clone(),
    };

    let (return_amount, _spread) = xyk_swap(offer_reserve, ask_reserve, offer_asset.amount)?;
    let send_msg = build_transfer_msg(&ask_info, &recipient, return_amount)?;

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "mock_simple_swap")
        .add_attribute("offer_amount", offer_asset.amount)
        .add_attribute("return_amount", return_amount))
}

fn execute_cw20_swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> StdResult<Response> {
    let state = STATE.load(deps.storage)?;
    if !state.fully_committed {
        return Err(StdError::generic_err("pool is in commit phase"));
    }

    let hook: HookMsg = from_json(&cw20_msg.msg)?;
    let HookMsg::Swap { to, .. } = hook;

    let offer_info = TokenType::CreatorToken {
        contract_addr: info.sender.clone(),
    };
    if !state.asset_infos.iter().any(
        |t| matches!(t, TokenType::CreatorToken { contract_addr } if *contract_addr == info.sender),
    ) {
        return Err(StdError::generic_err(format!(
            "cw20 sender {} is not in this pool's pair",
            info.sender
        )));
    }

    let (offer_reserve, ask_reserve, ask_info) =
        load_reserves(deps.as_ref(), &env, &state.asset_infos, &offer_info)?;
    // The cw20 transfer has already credited the pool, so live balance
    // includes the deposit; subtract to get the pre-swap reserve.
    let offer_reserve = offer_reserve.checked_sub(cw20_msg.amount).map_err(|_| {
        StdError::generic_err("offer reserve underflow; pool not seeded with declared funds")
    })?;

    let recipient = match to {
        Some(t) => deps.api.addr_validate(&t)?,
        None => deps.api.addr_validate(&cw20_msg.sender)?,
    };

    let (return_amount, _spread) = xyk_swap(offer_reserve, ask_reserve, cw20_msg.amount)?;
    let send_msg = build_transfer_msg(&ask_info, &recipient, return_amount)?;

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "mock_cw20_swap")
        .add_attribute("offer_amount", cw20_msg.amount)
        .add_attribute("return_amount", return_amount))
}

#[entry_point]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Pair {} => {
            let state = STATE.load(deps.storage)?;
            let assets = query_pools(
                &state.asset_infos,
                &deps.querier,
                env.contract.address.clone(),
            )?;
            to_json_binary(&PairResponse {
                asset_infos: state.asset_infos.clone(),
                contract_addr: env.contract.address,
                pair_type: PoolPairType::Xyk {},
                assets,
            })
        }
        QueryMsg::IsFullyCommited {} => {
            let state = STATE.load(deps.storage)?;
            let status = if state.fully_committed {
                CommitStatus::FullyCommitted
            } else {
                CommitStatus::InProgress {
                    raised: Uint128::zero(),
                    target: Uint128::new(1_000_000),
                }
            };
            to_json_binary(&status)
        }
        QueryMsg::Simulation { offer_asset } => {
            let state = STATE.load(deps.storage)?;
            let (offer_reserve, ask_reserve, _ask_info) =
                load_reserves(deps, &env, &state.asset_infos, &offer_asset.info)?;
            let (return_amount, spread_amount) =
                xyk_swap(offer_reserve, ask_reserve, offer_asset.amount)?;
            to_json_binary(&SimulationResponse {
                return_amount,
                spread_amount,
                commission_amount: Uint128::zero(),
            })
        }
    }
}

/// Returns `(offer_reserve, ask_reserve, ask_token_type)` for a swap whose
/// offer side matches `offer_info`. Reads live balances at the pool address.
fn load_reserves(
    deps: Deps,
    env: &Env,
    asset_infos: &[TokenType; 2],
    offer_info: &TokenType,
) -> StdResult<(Uint128, Uint128, TokenType)> {
    let assets = query_pools(asset_infos, &deps.querier, env.contract.address.clone())?;
    let (offer, ask) = if assets[0].info.equal(offer_info) {
        (&assets[0], &assets[1])
    } else if assets[1].info.equal(offer_info) {
        (&assets[1], &assets[0])
    } else {
        return Err(StdError::generic_err(
            "offer asset does not match either side of this pool",
        ));
    };
    Ok((offer.amount, ask.amount, ask.info.clone()))
}

fn xyk_swap(
    offer_reserve: Uint128,
    ask_reserve: Uint128,
    offer_amount: Uint128,
) -> StdResult<(Uint128, Uint128)> {
    if offer_reserve.is_zero() || ask_reserve.is_zero() {
        return Err(StdError::generic_err("pool has no liquidity"));
    }
    let new_offer = offer_reserve
        .checked_add(offer_amount)
        .map_err(|_| StdError::generic_err("offer overflow"))?;
    // return_amount = ask_reserve * offer_amount / (offer_reserve + offer_amount)
    let return_amount = ask_reserve.multiply_ratio(offer_amount, new_offer);
    // ideal_return = ask_reserve * offer_amount / offer_reserve (zero-slip)
    let ideal_return = ask_reserve.multiply_ratio(offer_amount, offer_reserve);
    let spread = ideal_return.checked_sub(return_amount).unwrap_or_default();
    Ok((return_amount, spread))
}

fn build_transfer_msg(
    asset: &TokenType,
    recipient: &Addr,
    amount: Uint128,
) -> StdResult<CosmosMsg<Empty>> {
    if amount.is_zero() {
        return Err(StdError::generic_err("zero return amount"));
    }
    match asset {
        TokenType::Native { denom } => Ok(CosmosMsg::Bank(BankMsg::Send {
            to_address: recipient.to_string(),
            amount: vec![Coin {
                denom: denom.clone(),
                amount,
            }],
        })),
        TokenType::CreatorToken { contract_addr } => Ok(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: contract_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                recipient: recipient.to_string(),
                amount,
            })?,
            funds: vec![],
        })),
    }
}
