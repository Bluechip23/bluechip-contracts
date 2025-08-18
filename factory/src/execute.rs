use std::env;

use crate::asset::AssetInfo;
use crate::error::ContractError;
use crate::msg::{
    CreatePoolReplyMsg, ExecuteMsg, MigrateMsg, TokenInfo,
    TokenInstantiateMsg,
};
use crate::pair::{CreatePool, FeeInfo, ThresholdPayout};
use crate::state::{
    FactoryInstantiate, CommitInfo, CONFIG, NEXT_POOL_ID, POOLS_BY_ID, COMMIT, TEMPCREATOR, TEMPNFTADDR,
    TEMPPOOLINFO, TEMPPOOLID, TEMPTOKENADDR,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Deps, DepsMut, Empty, Env, MessageInfo, Reply, Response,
    StdError, StdResult, SubMsg, SubMsgResult, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, MinterResponse};
use cw721_base::msg::InstantiateMsg as NFTPositionInstantiate;
use cw721_base::Action;

const CONTRACT_NAME: &str = "bluechip_factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTANTIATE_TOKEN_REPLY_ID: u64 = 1;
const INSTANTIATE_NFT_REPLY_ID: u64 = 3;
const INSTANTIATE_POOL_REPLY_ID: u64 = 2;

#[cfg_attr(not(feature = "library"), entry_point)]
//Pools use the factory as almost a launch pad. I guess the best way to think of it is as a literal factory. 
//it creates a template and holds logic for each new pool and gives the newly created pool new abilities like minting rights and other things. 
//It takes in parameters set by a json file once, so it becomes easy to set standards across all pools. Basically making it very repeatable.
//the factory is also the central entity pools can potentially recieve upgrades from. factory = bill gates pools = pcs running on windows.


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
        //edit factory parameters - only bluechip can (updates etc) - does not touch existing pools unless we do a chain wide change
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
        //creates new pool
        ExecuteMsg::Create {
            create_pool_msg,
            token_info,
        } => execute_create(deps, env, info, create_pool_msg, token_info),
    }
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
    create_pool: CreatePool,
    token_info: TokenInfo,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info.clone())?;
    let config = CONFIG.load(deps.storage)?;
    let sender = &info.sender;
    let pool_id = NEXT_POOL_ID.load(deps.storage)?;
    //incriments next pool
    NEXT_POOL_ID.save(deps.storage, &(pool_id + 1))?;
    //saves incrimented pool
    TEMPPOOLID.save(deps.storage, &pool_id)?;
    //temporary pool data, will get updated in reply
    TEMPPOOLINFO.save(deps.storage, &create_pool)?;
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
                cap: Some(Uint128::new(1_200_000u128)),
            }),
            marketing: None,
        })?,
        //no initial balance. waits until threshold is crossed to mint creator tokens. 
        funds: vec![],
        //the factory is the admin to the pool so it can call upgrades to the pools as the chain advances. 
        admin: Some(info.sender.to_string()),
        label: token_info.name,
    };

    let sub_msg = vec![SubMsg::reply_on_success(msg, INSTANTIATE_TOKEN_REPLY_ID)];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_submessages(sub_msg))
}

#[entry_point]
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    match msg.id {
        INSTANTIATE_TOKEN_REPLY_ID => {
            let config = CONFIG.load(deps.storage)?;

            // Extract token address from reply
            let token_address = match msg.result {
                SubMsgResult::Ok(result) => {
                    let contract_addr = result
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
                        .ok_or_else(|| {
                            StdError::generic_err("Contract address not found in events")
                        })?;

                    deps.api.addr_validate(&contract_addr)?
                }
                SubMsgResult::Err(err) => {
                    return Err(StdError::generic_err(format!(
                        "Token instantiation failed: {}",
                        err
                    ))
                    .into())
                }
            };

            // Save token address
            TEMPTOKENADDR.save(deps.storage, &token_address)?;

            //NFT instantiate message
            let nft_instantiate_msg = to_json_binary(&NFTPositionInstantiate {
                name: "AMM LP Positions".to_string(),
                symbol: "AMM-LP".to_string(),
                minter: env.contract.address.to_string(),
            })?;

            let nft_msg = WasmMsg::Instantiate {
                code_id: config.position_nft_id,
                msg: nft_instantiate_msg,
                funds: vec![],
                admin: None,
                label: format!("AMM-LP-NFT-{}", token_address),
            };

            let sub_msg = SubMsg::reply_on_success(nft_msg, INSTANTIATE_NFT_REPLY_ID);

            Ok(Response::new()
                .add_attribute("action", "instantiate_token_reply")
                .add_attribute("token_address", token_address)
                .add_submessage(sub_msg))
        }

        INSTANTIATE_NFT_REPLY_ID => {
            let config = CONFIG.load(deps.storage)?;
            let temp_pool_info = TEMPPOOLINFO.load(deps.storage)?;
            let temp_creator = TEMPCREATOR.load(deps.storage)?;
            let token_address = TEMPTOKENADDR.load(deps.storage)?;

            // Extract NFT address from reply
            let nft_address = match msg.result {
                SubMsgResult::Ok(result) => {
                    let contract_addr = result
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
                        .ok_or_else(|| {
                            StdError::generic_err("NFT contract address not found in events")
                        })?;

                    deps.api.addr_validate(&contract_addr)?
                }
                SubMsgResult::Err(err) => {
                    return Err(
                        StdError::generic_err(format!("NFT instantiation failed: {}", err)).into(),
                    )
                }
            };

            // Save NFT address
            TEMPNFTADDR.save(deps.storage, &nft_address)?;

            let threshold_payout = ThresholdPayout {
                creator_amount: Uint128::new(325_000_000_000),
                bluechip_amount: Uint128::new(25_000_000_000),
                pool_amount: Uint128::new(350_000_000_000),
                commit_amount: Uint128::new(500_000_000_000),
            };
            let threshold_payout = to_json_binary(&threshold_payout)?;

            let mut updated_asset_infos = temp_pool_info.asset_infos;
            for asset_info in updated_asset_infos.iter_mut() {
                if let AssetInfo::Token { contract_addr } = asset_info {
                    if contract_addr.as_str() == "WILL_BE_CREATED_BY_FACTORY" {
                        *contract_addr = token_address.clone();
                    }
                }
            }
            let pool_id = TEMPPOOLID.load(deps.storage)?;
            // Instantiate the pool
            let pool_msg = WasmMsg::Instantiate {
                code_id: config.pair_id,
                msg: to_json_binary(&CreatePoolReplyMsg {
                    pool_id,
                    asset_infos: updated_asset_infos,
                    factory_addr: env.contract.address,
                    token_code_id: config.token_id,
                    threshold_payout: Some(threshold_payout),
                    fee_info: FeeInfo {
                        bluechip_address: config.bluechip_address.clone(),
                        creator_address: temp_creator.clone(),
                        bluechip_fee: config.bluechipe_fee,
                        creator_fee: config.creator_fee,
                    },
                    commit_limit_usd: config.commit_limit_usd,
                    oracle_addr: config.oracle_addr.clone(),
                    oracle_symbol: config.oracle_symbol.clone(),
                    token_address: token_address,
                    position_nft_address: nft_address.clone(),
                })?,
                funds: vec![],
                admin: None,
                label: "Pair".to_string(),
            };

            let sub_msg = SubMsg::reply_on_success(pool_msg, INSTANTIATE_POOL_REPLY_ID);

            Ok(Response::new()
                .add_attribute("action", "instantiate_nft_reply")
                .add_attribute("nft_address", nft_address)
                .add_submessage(sub_msg))
        }

        INSTANTIATE_POOL_REPLY_ID => {
            // Your existing pool reply code - just add NFT minter update
            let temp_creator = TEMPCREATOR.load(deps.storage)?;
            let temp_token_address = TEMPTOKENADDR.load(deps.storage)?;
            let temp_nft_address = TEMPNFTADDR.load(deps.storage)?;
            let pool_id = TEMPPOOLID.load(deps.storage)?;

            let pool_address = match msg.result {
                SubMsgResult::Ok(result) => {
                    let contract_addr = result
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
                        .ok_or_else(|| {
                            StdError::generic_err("Contract address not found in events")
                        })?;

                    deps.api.addr_validate(&contract_addr)?
                }
                SubMsgResult::Err(err) => {
                    return Err(StdError::generic_err(format!(
                        "Pool instantiation failed: {}",
                        err
                    ))
                    .into())
                }
            };

            // Save commit info
            let commit_info = CommitInfo {
                pool_id, // Include the pool_id
                creator: temp_creator.clone(),
                token_addr: temp_token_address.clone(),
                pool_addr: pool_address.clone(),
            };

            // Save by creator address (existing)
            COMMIT.save(deps.storage, &temp_creator.to_string(), &commit_info)?;

            // save pool id for queries
            POOLS_BY_ID.save(deps.storage, pool_id, &commit_info)?;
            // make pool cw20 and cw721 (nft) minter for future responsibilities after crossing
            let update_token_minter = WasmMsg::Execute {
                contract_addr: temp_token_address.to_string(),
                msg: to_json_binary(&Cw20ExecuteMsg::UpdateMinter {
                    new_minter: Some(pool_address.to_string()),
                })?,
                funds: vec![],
            };
            let update_nft_ownership = WasmMsg::Execute {
                contract_addr: temp_nft_address.to_string(),
                msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
                    Action::TransferOwnership {
                        new_owner: pool_address.to_string(),
                        expiry: None,
                    },
                ))?,
                funds: vec![],
            };
            // Clean up temp storage
            TEMPPOOLID.remove(deps.storage);
            TEMPPOOLINFO.remove(deps.storage);
            TEMPCREATOR.remove(deps.storage);
            TEMPTOKENADDR.remove(deps.storage);
            TEMPNFTADDR.remove(deps.storage);

            Ok(Response::new()
                .add_message(update_token_minter)
                .add_message(update_nft_ownership)
                .add_attribute("action", "instantiate_pool_reply")
                .add_attribute("pool_address", pool_address)
                .add_attribute("pool_id", pool_id.to_string()))
        }

        _ => Err(StdError::generic_err(format!("Unknown reply ID: {}", msg.id)).into()),
    }
}

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
