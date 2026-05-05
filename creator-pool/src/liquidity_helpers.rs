//! Commit-phase-only claim handlers. The shared math + validators
//! previously in this file now live in `pool_core::liquidity_helpers`
//! and are re-exported below.
pub use pool_core::liquidity_helpers::*;

use crate::asset::get_native_denom;
use crate::error::ContractError;
use crate::state::{
    CreatorFeePot, COMMITFEEINFO, CREATOR_EXCESS_POSITION, CREATOR_FEE_POT, POOL_INFO,
};
use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, MessageInfo, Response, Timestamp, Uint128, WasmMsg,
};

/// Empties the CREATOR_FEE_POT to the creator wallet configured at pool
/// instantiation. Only the creator wallet can call this. Clip-slice fees
/// accumulate in the pot via `execute_collect_fees`, `add_to_position`,
/// `remove_all_liquidity`, and `remove_partial_liquidity`.
pub fn execute_claim_creator_fees(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    crate::generic_helpers::enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    crate::generic_helpers::with_reentrancy_guard(deps, |deps| {
        execute_claim_creator_fees_inner(deps, env, info)
    })
}

fn execute_claim_creator_fees_inner(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    if info.sender != fee_info.creator_wallet_address {
        return Err(ContractError::Unauthorized {});
    }

    let pot = CREATOR_FEE_POT.may_load(deps.storage)?.unwrap_or_default();
    if pot.amount_0.is_zero() && pot.amount_1.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut messages: Vec<CosmosMsg> = vec![];

    if !pot.amount_0.is_zero() {
        let native_denom = get_native_denom(&pool_info.pool_info.asset_infos)?;
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: fee_info.creator_wallet_address.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: native_denom,
                amount: pot.amount_0,
            }],
        }));
    }
    if !pot.amount_1.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: fee_info.creator_wallet_address.to_string(),
                amount: pot.amount_1,
            })?,
            funds: vec![],
        }));
    }

    // Reset the pot AFTER building the messages so a serialization error
    // in building the CW20 transfer would leave the pot intact.
    CREATOR_FEE_POT.save(
        deps.storage,
        &CreatorFeePot {
            amount_0: Uint128::zero(),
            amount_1: Uint128::zero(),
        },
    )?;

    Ok(Response::new().add_messages(messages).add_attributes(vec![
        ("action", "claim_creator_fees".to_string()),
        ("creator", fee_info.creator_wallet_address.to_string()),
        ("amount_0", pot.amount_0.to_string()),
        ("amount_1", pot.amount_1.to_string()),
        ("pool_contract", env.contract.address.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
    ]))
}

pub fn execute_claim_creator_excess(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    // Same deadline semantics as commit / swap / liquidity handlers: when
    // provided, reject txs that landed past the caller's deadline. Keeps
    // claims from being ambushed by a mempool replay long after the
    // creator expected their tx to be final.
    crate::generic_helpers::enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    crate::generic_helpers::with_reentrancy_guard(deps, |deps| {
        execute_claim_creator_excess_inner(deps, env, info)
    })
}

fn execute_claim_creator_excess_inner(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let excess_position = CREATOR_EXCESS_POSITION.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;

    if info.sender != excess_position.creator {
        return Err(ContractError::Unauthorized {});
    }

    if env.block.time < excess_position.unlock_time {
        return Err(ContractError::PositionLocked {
            unlock_time: excess_position.unlock_time,
        });
    }

    CREATOR_EXCESS_POSITION.remove(deps.storage);

    // Send tokens directly to the creator instead of creating an LP position.
    // The creator can deposit as liquidity themselves if they choose to.
    let mut messages: Vec<CosmosMsg> = vec![];

    if !excess_position.bluechip_amount.is_zero() {
        let native_denom = get_native_denom(&pool_info.pool_info.asset_infos)?;
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: excess_position.creator.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: native_denom,
                amount: excess_position.bluechip_amount,
            }],
        }));
    }

    if !excess_position.token_amount.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: excess_position.creator.to_string(),
                amount: excess_position.token_amount,
            })?,
            funds: vec![],
        }));
    }

    Ok(Response::new().add_messages(messages).add_attributes(vec![
        ("action", "claim_creator_excess".to_string()),
        ("creator", excess_position.creator.to_string()),
        ("bluechip_amount", excess_position.bluechip_amount.to_string()),
        ("token_amount", excess_position.token_amount.to_string()),
        ("pool_contract", env.contract.address.to_string()),
        ("block_height", env.block.height.to_string()),
        ("block_time", env.block.time.seconds().to_string()),
    ]))
}
