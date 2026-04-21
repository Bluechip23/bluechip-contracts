use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, DepsMut, Env, Reply, Response, StdResult, Storage, SubMsg,
    SubMsgResponse, SubMsgResult, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};

use crate::error::ContractError;
use crate::execute::{encode_reply_id, CLEANUP_NFT_STEP, CLEANUP_TOKEN_STEP};
use crate::state::{CreationStatus, PoolCreationState, POOL_CREATION_CONTEXT};

/// Drop the per-pool creation context once the creation flow has
/// terminated (success or after cleanup messages have fired).
pub fn cleanup_temp_state(storage: &mut dyn Storage, pool_id: u64) -> StdResult<()> {
    POOL_CREATION_CONTEXT.remove(storage, pool_id);
    Ok(())
}

pub fn create_cleanup_messages(
    creation_state: &PoolCreationState,
    pool_id: u64,
) -> Result<Vec<SubMsg>, ContractError> {
    let mut messages = vec![];
    if let Some(token_addr) = &creation_state.creator_token_address {
        let disable_token_msg = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter { new_minter: None })?,
            funds: vec![],
        };
        let sub_msg: SubMsg = SubMsg::reply_always(
            disable_token_msg,
            encode_reply_id(pool_id, CLEANUP_TOKEN_STEP),
        );
        messages.push(sub_msg);
    }
    if let Some(nft_addr) = &creation_state.mint_new_position_nft_address {
        // Renounce instead of transferring to a sentinel address: the
        // previous "cosmos1qqq...nrql8a" sentinel had a `cosmos` bech32
        // prefix and was rejected by addr_validate on this chain (prefix
        // `bluechip`), causing every NFT cleanup to fail silently.
        // RenounceOwnership doesn't need any address at all.
        let disable_nft_msg = WasmMsg::Execute {
            contract_addr: nft_addr.to_string(),
            msg: to_json_binary(&Cw721ExecuteMsg::<()>::UpdateOwnership(
                Action::RenounceOwnership,
            ))?,
            funds: vec![],
        };
        let sub_msg: SubMsg =
            SubMsg::reply_always(disable_nft_msg, encode_reply_id(pool_id, CLEANUP_NFT_STEP));
        messages.push(sub_msg);
    }

    Ok(messages)
}
pub fn handle_cleanup_reply(
    deps: DepsMut,
    _env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    match msg.result {
        SubMsgResult::Ok(_) => {
            cleanup_temp_state(deps.storage, pool_id)?;
            Ok(Response::new().add_attribute("action", "cleanup_completed"))
        }
        SubMsgResult::Err(err) => {
            if let Ok(mut ctx) = POOL_CREATION_CONTEXT.load(deps.storage, pool_id) {
                ctx.state.status = CreationStatus::Failed;
                ctx.state.retry_count += 1;
                POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;
            }

            Ok(Response::new()
                .add_attribute("action", "cleanup_failed")
                .add_attribute("error", err))
        }
    }
}

pub fn extract_contract_address(
    deps: &DepsMut,
    result: &SubMsgResponse,
) -> Result<Addr, ContractError> {
    let addr_str = result
        .events
        .iter()
        .find(|event| event.ty == "instantiate")
        .and_then(|event| {
            event
                .attributes
                .iter()
                .find(|attr| attr.key == "_contract_address")
                .map(|attr| attr.value.clone())
        })
        .ok_or(ContractError::ContractAddressNotFound {})?;

    deps.api.addr_validate(&addr_str).map_err(|e| {
        ContractError::Std(cosmwasm_std::StdError::generic_err(format!(
            "Invalid contract address from instantiate event: {}",
            e
        )))
    })
}

pub fn give_pool_ownership_cw20_and_nft(
    token_addr: &Addr,
    nft_addr: &Addr,
    pool_addr: &Addr,
) -> Result<Vec<CosmosMsg>, ContractError> {
    Ok(vec![
        WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter {
                new_minter: Some(pool_addr.to_string()),
            })?,
            funds: vec![],
        }
        .into(),
        WasmMsg::Execute {
            contract_addr: nft_addr.to_string(),
            msg: to_json_binary(&Cw721ExecuteMsg::<()>::UpdateOwnership(
                Action::TransferOwnership {
                    new_owner: pool_addr.to_string(),
                    expiry: None,
                },
            ))?,
            funds: vec![],
        }
        .into(),
    ])
}
