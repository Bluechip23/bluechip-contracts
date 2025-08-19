use std::env;

use crate::asset::AssetInfo;
use crate::error::ContractError;
use crate::msg::{CreatePoolReplyMsg, ExecuteMsg, MigrateMsg, TokenInfo, TokenInstantiateMsg};
use crate::pair::{CreatePool, FeeInfo, ThresholdPayout};
use crate::state::{
    CommitInfo, CreationState, CreationStatus, FactoryInstantiate, CONFIG, CREATION_STATES,
    NEXT_POOL_ID, POOLS_BY_ID, COMMIT, TEMPCREATOR, TEMPNFTADDR, TEMPPAIRINFO, TEMPPOOLID,
    TEMPTOKENADDR,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Deps, DepsMut, Empty, Env, MessageInfo, Reply, Response,
    StdError, StdResult, Storage, SubMsg, SubMsgResponse, SubMsgResult, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, MinterResponse};
use cw721_base::msg::InstantiateMsg as Cw721InstantiateMsg;
use cw721_base::Action;

const CONTRACT_NAME: &str = "bluechip_factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BURN_ADDRESS: &str = "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnrql8a";
pub const SET_TOKENS: u64 = 1;
pub const MINT_CREATE_POOL: u64 = 2;
pub const FINALIZE_POOL: u64 = 3;
pub const CLEANUP_TOKEN_REPLY_ID: u64 = 100;
pub const CLEANUP_NFT_REPLY_ID: u64 = 101;

#[cfg_attr(not(feature = "library"), entry_point)]
//instantiates the factory for pools to use.
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: FactoryInstantiate,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    //saves the factory parameters set in the json file
    CONFIG.save(deps.storage, &msg)?;
    //sets the first pool created by this factory to 1
    NEXT_POOL_ID.save(deps.storage, &1u64)?;
    //viola
    Ok(Response::new().add_attribute("action", "init_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    let version = cw2::get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }
    if version.version != CONTRACT_VERSION {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }

    Ok(Response::default().add_attribute("action", "migrate_contract"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        //edit factory parameters - only bluechip can - does not touch existing pools unless we do a chain wide change
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
        //creates new pool
        ExecuteMsg::Create {
            pool_msg,
            token_info,
        } => execute_create(deps, env, info, pool_msg, token_info),
    }
}

//make sure the factory sent the message to instantiate the pool
fn assert_is_admin(deps: Deps, info: MessageInfo) -> StdResult<bool> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.admin {
        return Err(StdError::generic_err(format!(
            "Only the admin can execute this function. Admin: {}, Sender: {}",
            config.admin, info.sender
        )));
    }

    Ok(true)
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    config: FactoryInstantiate,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info)?;

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

//create pool
fn execute_create(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pair_msg: CreatePool,
    token_info: TokenInfo,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info.clone())?;
    let config = CONFIG.load(deps.storage)?;
    let sender = info.sender.clone();
    let pool_id = NEXT_POOL_ID.load(deps.storage)?;
    NEXT_POOL_ID.save(deps.storage, &(pool_id + 1))?;
    TEMPPOOLID.save(deps.storage, &pool_id)?;
    TEMPPAIRINFO.save(deps.storage, &pair_msg)?;
    TEMPCREATOR.save(deps.storage, &sender)?;
    let msg = WasmMsg::Instantiate {
        code_id: config.token_id,
        //creating the creator tokens - they are not minted yet. Simply created. Factory hands the minting responsibilities to pool.
        msg: to_json_binary(&TokenInstantiateMsg {
            name: token_info.name.clone(),
            symbol: token_info.symbol.clone(),
            decimals: 6,
            initial_balances: vec![],
            mint: Some(MinterResponse {
                minter: env.contract.address.to_string(),
                //amount minted after threshold.
                cap: Some(Uint128::new(1_500_000_000_000)),
            }),
        })?,
        //no initial balance. waits until threshold is crossed to mint creator tokens.
        funds: vec![],
        //the factory is the admin to the pool so it can call upgrades to the pools as the chain advances.
        admin: Some(info.sender.to_string()),
        label: token_info.name,
    };

    let creation_state = CreationState {
        pool_id,
        creator: info.sender,
        token_address: None,
        nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
    //triggers reply function when things go well.
    let sub_msg = vec![SubMsg::reply_on_success(msg, SET_TOKENS)];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

#[entry_point]
//called by execute create.
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    match msg.id {
        SET_TOKENS => set_tokens(deps, env, msg),
        MINT_CREATE_POOL => mint_create_pool(deps, env, msg),
        FINALIZE_POOL => finalize_pool(deps, env, msg),
        CLEANUP_TOKEN_REPLY_ID => handle_cleanup_reply(deps, env, msg),
        CLEANUP_NFT_REPLY_ID => handle_cleanup_reply(deps, env, msg),
        _ => Err(ContractError::UnknownReplyId { id: msg.id }),
    }
}

fn set_tokens(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    let pool_id = TEMPPOOLID.load(deps.storage)?;
    let mut creation_state = CREATION_STATES.load(deps.storage, pool_id)?;
    let factory_addr = env.contract.address.clone();
    match msg.result {
        SubMsgResult::Ok(result) => {
            // extract token address from reply - helper below.
            let token_address = extract_contract_address(&result)?;

            //update creation state to catch transaction failures.
            creation_state.token_address = Some(token_address.clone());
            creation_state.status = CreationStatus::TokenCreated;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            // Save to temp storage for next step
            TEMPTOKENADDR.save(deps.storage, &token_address)?;

            // Create NFT instantiate message
            let config = CONFIG.load(deps.storage)?;
            let nft_instantiate_msg = to_json_binary(&Cw721InstantiateMsg {
                name: "AMM LP Positions".to_string(),
                symbol: "AMM-LP".to_string(),
                minter: env.contract.address.to_string(),
            })?;

            let nft_msg = WasmMsg::Instantiate {
                code_id: config.position_nft_id,
                msg: nft_instantiate_msg,
                //no initial NFTs since no positions have been created yet
                funds: vec![],
                //set factory as admin
                admin: Some(factory_addr.to_string()),
                label: format!("AMM-LP-NFT-{}", token_address),
            };

            let sub_msg = SubMsg::reply_on_success(nft_msg, MINT_CREATE_POOL);

            Ok(Response::new()
                .add_attribute("action", "token_created_successfully")
                .add_attribute("token_address", token_address)
                .add_attribute("pool_id", pool_id.to_string())
                .add_submessage(sub_msg))
        }
        SubMsgResult::Err(err) => {
            // failure, mark as clean up and burn. Check helpers below.
            creation_state.status = CreationStatus::Failed;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            cleanup_temp_state(deps.storage)?;

            Err(ContractError::TokenCreationFailed {
                pool_id,
                reason: err,
            })
        }
    }
}

fn mint_create_pool(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    let pool_id = TEMPPOOLID.load(deps.storage)?;
    let mut creation_state = CREATION_STATES.load(deps.storage, pool_id)?;
    let factory_addr = env.contract.address.clone();
    match msg.result {
        SubMsgResult::Ok(result) => {
            let nft_address = extract_contract_address(&result)?;

            creation_state.nft_address = Some(nft_address.clone());
            creation_state.status = CreationStatus::NftCreated;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            TEMPNFTADDR.save(deps.storage, &nft_address)?;

            let config = CONFIG.load(deps.storage)?;
            let temp_pool_info = TEMPPAIRINFO.load(deps.storage)?;
            let temp_creator = TEMPCREATOR.load(deps.storage)?;
            let token_address = TEMPTOKENADDR.load(deps.storage)?;

            //set threshold payouts for threshold crossing
            let threshold_payout = ThresholdPayout {
                creator_amount: Uint128::new(325_000_000_000),
                bluechip_amount: Uint128::new(25_000_000_000),
                pool_amount: Uint128::new(350_000_000_000),
                commit_amount: Uint128::new(500_000_000_000),
            };
            let threshold_binary = to_json_binary(&threshold_payout)?;

            let mut updated_asset_infos = temp_pool_info.asset_infos;
            for asset_info in updated_asset_infos.iter_mut() {
                if let AssetInfo::Token { contract_addr } = asset_info {
                    if contract_addr.as_str() == "WILL_BE_CREATED_BY_FACTORY" {
                        *contract_addr = token_address.clone();
                    }
                }
            }

            // Instantiate the pool
            let pool_msg = WasmMsg::Instantiate {
                code_id: config.pair_id,
                msg: to_json_binary(&CreatePoolReplyMsg {
                    pool_id,
                    asset_infos: updated_asset_infos,
                    factory_addr: env.contract.address,
                    token_code_id: config.token_id,
                    threshold_payout: Some(threshold_binary),
                    fee_info: FeeInfo {
                        bluechip_address: config.bluechip_address.clone(),
                        creator_address: temp_creator.clone(),
                        bluechip_fee: config.bluechip_fee,
                        creator_fee: config.creator_fee,
                    },
                    commit_amount_for_threshold:config.commit_amount_for_threshold, 
                    commit_limit_usd: config.commit_limit_usd,
                    oracle_addr: config.oracle_addr.clone(),
                    oracle_symbol: config.oracle_symbol.clone(),
                    token_address: token_address,
                    position_nft_address: nft_address.clone(),
                })?,
                funds: vec![],
                admin: Some(factory_addr.to_string()),
                label: "Pair".to_string(),
            };
            let sub_msg: SubMsg = SubMsg::reply_on_success(pool_msg, FINALIZE_POOL);

            Ok(Response::new()
                .add_attribute("action", "nft_created_successfully")
                .add_attribute("nft_address", nft_address)
                .add_attribute("pool_id", pool_id.to_string())
                .add_submessage(sub_msg))
        }
        SubMsgResult::Err(err) => {
            creation_state.status = CreationStatus::CleaningUp;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            let cleanup_msgs = create_cleanup_messages(&creation_state)?;

            Ok(Response::new()
                .add_submessages(cleanup_msgs)
                .add_attribute("action", "nft_creation_failed_cleanup")
                .add_attribute("pool_id", pool_id.to_string())
                .add_attribute("error", err))
        }
    }
}

fn finalize_pool(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    let pool_id = TEMPPOOLID.load(deps.storage)?;
    let mut creation_state = CREATION_STATES.load(deps.storage, pool_id)?;
    match msg.result {
        SubMsgResult::Ok(result) => {
            let pool_address = extract_contract_address(&result)?;

            creation_state.pool_address = Some(pool_address.clone());
            creation_state.status = CreationStatus::PoolCreated;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            let temp_creator = TEMPCREATOR.load(deps.storage)?;
            let temp_token_address = TEMPTOKENADDR.load(deps.storage)?;
            let temp_nft_address = TEMPNFTADDR.load(deps.storage)?;
            // create commit parameters
            let commit_info = CommitInfo {
                pool_id,
                creator: temp_creator.clone(),
                token_addr: temp_token_address.clone(),
                pool_addr: pool_address.clone(),
            };
            // save commit state to record future commit executions
            COMMIT.save(deps.storage, &temp_creator.to_string(), &commit_info)?;
            POOLS_BY_ID.save(deps.storage, pool_id, &commit_info)?;

            // make pool cw20 and cw721 (nft) minter for threshold payout - compartmentalizes pools so they can run without factory.
            //1 factory for all pools instead of 1 factory 1 pool
            let ownership_msgs = create_ownership_transfer_messages(
                &temp_token_address,
                &temp_nft_address,
                &pool_address,
            )?;

            creation_state.status = CreationStatus::Completed;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            cleanup_temp_state(deps.storage)?;

            Ok(Response::new()
                .add_messages(ownership_msgs)
                .add_attribute("action", "pool_created_successfully")
                .add_attribute("pool_address", pool_address)
                .add_attribute("pool_id", pool_id.to_string()))
        }
        SubMsgResult::Err(err) => {
            // Pool creation failed - cleanup everything
            creation_state.status = CreationStatus::CleaningUp;
            CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;

            let cleanup_msgs = create_cleanup_messages(&creation_state)?;

            Ok(Response::new()
                .add_submessages(cleanup_msgs)
                .add_attribute("action", "pool_creation_failed_cleanup")
                .add_attribute("pool_id", pool_id.to_string())
                .add_attribute("error", err))
        }
    }
}

//helpers for reply function

//clean and remove all temp information
fn cleanup_temp_state(storage: &mut dyn Storage) -> Result<(), ContractError> {
    TEMPPOOLID.remove(storage);
    TEMPPAIRINFO.remove(storage);
    TEMPCREATOR.remove(storage);
    TEMPTOKENADDR.remove(storage);
    TEMPNFTADDR.remove(storage);
    Ok(())
}

//if partial transaction happens
fn create_cleanup_messages(creation_state: &CreationState) -> Result<Vec<SubMsg>, ContractError> {
    // Changed return type to Vec<SubMsg>
    let mut messages = vec![];

    // if token was created, disable it
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

    // if NFT was created, disable it
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

fn handle_cleanup_reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
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
            // Cleanup failed - mark for manual intervention
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
fn extract_contract_address(result: &SubMsgResponse) -> Result<Addr, ContractError> {
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

//give pool minter responsibilities
fn create_ownership_transfer_messages(
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
