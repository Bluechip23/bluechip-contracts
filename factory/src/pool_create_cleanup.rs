use cosmwasm_std::{to_json_binary, Addr, CosmosMsg, DepsMut, StdResult, SubMsgResponse, WasmMsg};
use cw20::Cw20ExecuteMsg;
use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};

use crate::error::ContractError;

// NOTE: The pool-creation reply chain uses `SubMsg::reply_on_success` at every
// step (see execute::execute_create_creator_pool, pool_creation_reply::set_tokens,
// and pool_creation_reply::mint_create_pool). Under reply_on_success, a failing
// submessage bypasses the reply handler entirely and propagates the error up
// through the entire CosmWasm tx, which rolls back ALL state writes atomically
// — including any prior successful reply handlers' writes, and the CW20/CW721
// instantiations themselves.
//
// As a result there is nothing to clean up on failure, and the previous
// `cleanup_temp_state` / `create_cleanup_messages` / `handle_cleanup_reply`
// machinery was structurally unreachable (dead code). It has been removed.
// If a future change converts any step to `reply_always` / `reply_on_error`,
// explicit cleanup must be reintroduced at that step.

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
) -> StdResult<Vec<CosmosMsg>> {
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
