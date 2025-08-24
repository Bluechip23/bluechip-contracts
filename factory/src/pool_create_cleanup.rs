use cosmwasm_std::{to_json_binary, Addr, CosmosMsg, DepsMut, Empty, Env, Reply, Response, Storage, SubMsg, SubMsgResponse, SubMsgResult, WasmMsg};
use cw20::Cw20ExecuteMsg;
use cw721_base::Action;

use crate::error::ContractError;
use crate::execute::{BURN_ADDRESS, CLEANUP_NFT_REPLY_ID, CLEANUP_TOKEN_REPLY_ID};
use crate::state::{CreationState, CreationStatus, CREATION_STATES, TEMPCREATOR, TEMPNFTADDR, TEMPPAIRINFO, TEMPPOOLID, TEMPTOKENADDR};

//clean and remove all temp information used during pool creation
pub fn cleanup_temp_state(storage: &mut dyn Storage) -> Result<(), ContractError> {
    TEMPPOOLID.remove(storage);
    TEMPPAIRINFO.remove(storage);
    TEMPCREATOR.remove(storage);
    TEMPTOKENADDR.remove(storage);
    TEMPNFTADDR.remove(storage);
    Ok(())
}

//if partial transaction happens 
pub fn create_cleanup_messages(creation_state: &CreationState) -> Result<Vec<SubMsg>, ContractError> {
    // Changed return type to Vec<SubMsg>
    let mut messages = vec![];

    // if token was created, disable it by removing minter
    if let Some(token_addr) = &creation_state.token_address {
        let disable_token_msg = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter {
                new_minter: None, // remove minter entirely
            })?,
            funds: vec![],
        };

        // create SubMsg that will trigger reply handler
        let sub_msg: SubMsg = SubMsg::reply_on_error(disable_token_msg, CLEANUP_TOKEN_REPLY_ID);
        messages.push(sub_msg);
    }

    // if NFT was created, disable it by removing minter
    if let Some(nft_addr) = &creation_state.nft_address {
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

        // Create SubMsg that will trigger reply handler
        let sub_msg: SubMsg = SubMsg::reply_on_error(disable_nft_msg, CLEANUP_NFT_REPLY_ID);
        messages.push(sub_msg);
    }

    Ok(messages)
}

//handles replies for cleanup operations
pub fn handle_cleanup_reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    // Handle cleanup completion/failure
    match msg.result {
        SubMsgResult::Ok(_) => {
            // Cleanup succeeded - remove creation state
            if let Ok(pool_id) = TEMPPOOLID.load(deps.storage) {
                CREATION_STATES.remove(deps.storage, pool_id);
                cleanup_temp_state(deps.storage)?;
            }

            Ok(Response::new().add_attribute("action", "cleanup_completed"))
        }
        SubMsgResult::Err(err) => {
            
            if let Ok(pool_id) = TEMPPOOLID.load(deps.storage) {
                if let Ok(mut state) = CREATION_STATES.load(deps.storage, pool_id) {
                    state.status = CreationStatus::Failed;
                    state.retry_count += 1;
                    CREATION_STATES.save(deps.storage, pool_id, &state)?;
                }
            }

            Ok(Response::new()
                .add_attribute("action", "cleanup_failed")
                .add_attribute("error", err))
        }
    }
}

//pull contract addresss - can be used for multiple types.
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

//give pool minter responsibilities for both creator token and liquidty NFTs
pub fn create_ownership_transfer_messages(
    token_addr: &Addr,
    nft_addr: &Addr,
    pool_addr: &Addr,
) -> Result<Vec<CosmosMsg>, ContractError> {
    Ok(vec![
        WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter {
                //make pool minter of the tokens, 
                new_minter: Some(pool_addr.to_string()),
            })?,
            funds: vec![],
        }
        .into(),
        WasmMsg::Execute {
            contract_addr: nft_addr.to_string(),
            msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
                Action::TransferOwnership {
                    //pool now own nft contract to mint nfts to liquidity providers
                    new_owner: pool_addr.to_string(),
                    expiry: None,
                },
            ))?,
            funds: vec![],
        }
        .into(),
    ])
}
