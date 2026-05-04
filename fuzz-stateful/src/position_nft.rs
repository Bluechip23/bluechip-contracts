//! Minimal CW721-shaped position NFT used by the fuzz harness.
//!
//! Implements just the surface area the pool-core liquidity handlers
//! call into:
//!   - InstantiateMsg { name, symbol, minter }
//!   - ExecuteMsg::Mint { token_id, owner, ... }
//!   - ExecuteMsg::UpdateOwnership(TransferOwnership { new_owner, .. })
//!   - ExecuteMsg::UpdateOwnership(AcceptOwnership)
//!   - QueryMsg::OwnerOf { token_id, .. } -> OwnerOfResponse
//!
//! The minter is the only address allowed to mint. The owner is the
//! address allowed to call UpdateOwnership(TransferOwnership). The
//! pending-owner is the address allowed to call AcceptOwnership.

use cosmwasm_std::{
    entry_point, to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult,
};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::cw721_msgs::{
    Action, Cw721ExecuteMsg, Cw721InstantiateMsg, Cw721QueryMsg, OwnerOfResponse,
};

const OWNER: Item<Addr> = Item::new("owner");
const PENDING_OWNER: Item<Addr> = Item::new("pending_owner");
const MINTER: Item<Addr> = Item::new("minter");
const TOKEN_OWNERS: Map<&str, Addr> = Map::new("tokens");

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: Cw721InstantiateMsg,
) -> StdResult<Response> {
    let minter = deps.api.addr_validate(&msg.minter)?;
    OWNER.save(deps.storage, &info.sender)?;
    MINTER.save(deps.storage, &minter)?;
    Ok(Response::new()
        .add_attribute("action", "instantiate_nft")
        .add_attribute("name", msg.name)
        .add_attribute("symbol", msg.symbol))
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: Cw721ExecuteMsg<cosmwasm_std::Empty>,
) -> StdResult<Response> {
    match msg {
        Cw721ExecuteMsg::Mint { token_id, owner, .. } => {
            let minter = MINTER.load(deps.storage)?;
            if info.sender != minter {
                return Err(StdError::generic_err("only minter"));
            }
            let owner_addr = deps.api.addr_validate(&owner)?;
            if TOKEN_OWNERS.may_load(deps.storage, &token_id)?.is_some() {
                return Err(StdError::generic_err("token already minted"));
            }
            TOKEN_OWNERS.save(deps.storage, &token_id, &owner_addr)?;
            Ok(Response::new()
                .add_attribute("action", "mint")
                .add_attribute("token_id", token_id)
                .add_attribute("owner", owner))
        }
        Cw721ExecuteMsg::UpdateOwnership(action) => match action {
            Action::TransferOwnership { new_owner, .. } => {
                let cur = OWNER.load(deps.storage)?;
                if info.sender != cur {
                    return Err(StdError::generic_err("only owner"));
                }
                let new = deps.api.addr_validate(&new_owner)?;
                PENDING_OWNER.save(deps.storage, &new)?;
                Ok(Response::new()
                    .add_attribute("action", "transfer_ownership")
                    .add_attribute("new_owner", new_owner))
            }
            Action::AcceptOwnership => {
                let pending = PENDING_OWNER.may_load(deps.storage)?.ok_or_else(|| {
                    StdError::generic_err("no pending ownership")
                })?;
                if info.sender != pending {
                    return Err(StdError::generic_err("only pending owner"));
                }
                OWNER.save(deps.storage, &pending)?;
                MINTER.save(deps.storage, &pending)?;
                PENDING_OWNER.remove(deps.storage);
                Ok(Response::new().add_attribute("action", "accept_ownership"))
            }
            Action::RenounceOwnership => {
                let cur = OWNER.load(deps.storage)?;
                if info.sender != cur {
                    return Err(StdError::generic_err("only owner"));
                }
                Ok(Response::new().add_attribute("action", "renounce_ownership"))
            }
        },
    }
}

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: Cw721QueryMsg) -> StdResult<Binary> {
    match msg {
        Cw721QueryMsg::OwnerOf { token_id, .. } => {
            let owner = TOKEN_OWNERS.load(deps.storage, &token_id)?;
            to_json_binary(&OwnerOfResponse {
                owner: owner.to_string(),
                approvals: vec![],
            })
        }
    }
}
