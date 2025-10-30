use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, DepsMut, Empty, Env, Reply, Response, StdResult, Storage,
    SubMsg, SubMsgResponse, SubMsgResult, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw721_base::Action;

use crate::error::ContractError;
use crate::execute::{BURN_ADDRESS, CLEANUP_NFT_REPLY_ID, CLEANUP_TOKEN_REPLY_ID};
use crate::state::{CreationStatus, PoolCreationState, POOL_CREATION_STATES, TEMP_POOL_CREATION};

pub fn cleanup_temp_state(storage: &mut dyn Storage) -> StdResult<()> {
    TEMP_POOL_CREATION.remove(storage);
    Ok(())
}

pub fn create_cleanup_messages(
    creation_state: &PoolCreationState,
) -> Result<Vec<SubMsg>, ContractError> {
    let mut messages = vec![];
    if let Some(token_addr) = &creation_state.creator_token_address {
        let disable_token_msg = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter { new_minter: None })?,
            funds: vec![],
        };
        let sub_msg: SubMsg = SubMsg::reply_on_error(disable_token_msg, CLEANUP_TOKEN_REPLY_ID);
        messages.push(sub_msg);
    }
    if let Some(nft_addr) = &creation_state.mint_new_position_nft_address {
        let disable_nft_msg = WasmMsg::Execute {
            contract_addr: nft_addr.to_string(),
            msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
                Action::TransferOwnership {
                    new_owner: BURN_ADDRESS.to_string(),
                    expiry: None,
                },
            ))?,
            funds: vec![],
        };
        let sub_msg: SubMsg = SubMsg::reply_on_error(disable_nft_msg, CLEANUP_NFT_REPLY_ID);
        messages.push(sub_msg);
    }

    Ok(messages)
}
pub fn handle_cleanup_reply(
    deps: DepsMut,
    _env: Env,
    msg: Reply,
) -> Result<Response, ContractError> {
    match msg.result {
        SubMsgResult::Ok(_) => {
            if let Ok(temp_state) = TEMP_POOL_CREATION.load(deps.storage) {
                let pool_id = temp_state.pool_id;
                POOL_CREATION_STATES.remove(deps.storage, pool_id);
                cleanup_temp_state(deps.storage)?;
            }
            Ok(Response::new().add_attribute("action", "cleanup_completed"))
        }
        SubMsgResult::Err(err) => {
            if let Ok(temp_state) = TEMP_POOL_CREATION.load(deps.storage) {
                let pool_id = temp_state.pool_id;
                if let Ok(mut state) = POOL_CREATION_STATES.load(deps.storage, pool_id) {
                    state.status = CreationStatus::Failed;
                    state.retry_count += 1;
                    POOL_CREATION_STATES.save(deps.storage, pool_id, &state)?;
                }
            }

            Ok(Response::new()
                .add_attribute("action", "cleanup_failed")
                .add_attribute("error", err))
        }
    }
}

pub fn extract_contract_address(result: &SubMsgResponse) -> Result<Addr, ContractError> {
    result
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
        .ok_or_else(|| ContractError::ContractAddressNotFound {})
        .and_then(|addr_str| Ok(Addr::unchecked(addr_str)))
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
            msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
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
