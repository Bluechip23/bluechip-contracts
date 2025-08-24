use crate::{
    asset::AssetInfo,
    error::ContractError,
    execute::{FINALIZE_POOL, MINT_CREATE_POOL},
    msg::CreatePoolReplyMsg,
    pair::{FeeInfo, ThresholdPayout},
    pool_create_cleanup::{
        cleanup_temp_state, create_cleanup_messages, create_ownership_transfer_messages,
        extract_contract_address, handle_cleanup_reply,
    },
    state::{
        CommitInfo, CreationStatus, COMMIT, CONFIG, CREATION_STATES, POOLS_BY_ID, TEMPCREATOR,
        TEMPNFTADDR, TEMPPAIRINFO, TEMPPOOLID, TEMPTOKENADDR,
    },
};
use cosmwasm_std::{
    entry_point, to_json_binary, DepsMut, Env, Reply, Response, SubMsg, SubMsgResult, Uint128,
    WasmMsg,
};
use cw721_base::msg::InstantiateMsg as Cw721InstantiateMsg;

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

pub fn set_tokens(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
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
            //everything successful, move on to next step in creation
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

pub fn mint_create_pool(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    let pool_id = TEMPPOOLID.load(deps.storage)?;
    let mut creation_state = CREATION_STATES.load(deps.storage, pool_id)?;
    let factory_addr = env.contract.address.clone();
    match msg.result {
        SubMsgResult::Ok(result) => {
            let nft_address = extract_contract_address(&result)?;
            //set creation state for tracking and potential errors
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

            // Instantiate the pool with final values
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
                    commit_amount_for_threshold: config.commit_amount_for_threshold,
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
            //goes well, move on to next step
            let sub_msg: SubMsg = SubMsg::reply_on_success(pool_msg, FINALIZE_POOL);

            Ok(Response::new()
                .add_attribute("action", "nft_created_successfully")
                .add_attribute("nft_address", nft_address)
                .add_attribute("pool_id", pool_id.to_string())
                .add_submessage(sub_msg))
        }
        //doesnt go well, find which state the pool is in and behave accordingly.
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

//take created pool, give it minting rights, sync to necessary contracts to mint.
pub fn finalize_pool(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
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
            // create commit parameters for commit transactions
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
            //mark completed and clean all temp states since all values have all been updated and created.
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
