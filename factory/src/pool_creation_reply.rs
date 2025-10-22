use crate::{
    asset::TokenType,
    error::ContractError,
    execute::{FINALIZE_POOL, MINT_CREATE_POOL},
    msg::CreatePoolReplyMsg,
    pool_struct::{CommitFeeInfo, PoolDetails, ThresholdPayoutAmounts},
    pool_create_cleanup::{
        cleanup_temp_state, create_cleanup_messages, extract_contract_address, give_pool_ownership_cw20_and_nft
    },
    state::{
        CommitInfo, CreationStatus, POOL_CREATION_STATES, FACTORYINSTANTIATEINFO, POOLS_BY_ID, SETCOMMIT, TEMP_POOL_CREATION
    },
};
use cosmwasm_std::{
    to_json_binary, DepsMut, Env, Reply, Response, SubMsg, SubMsgResult, Uint128,
    WasmMsg, StdError,
};
use cw721_base::msg::InstantiateMsg as Cw721InstantiateMsg;

// pool_creation_reply.rs

pub fn set_tokens(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    // Load the consolidated context
    let mut pool_context = TEMP_POOL_CREATION.load(deps.storage)?;
    let pool_id = pool_context.pool_id;
    let mut creation_state = POOL_CREATION_STATES.load(deps.storage, pool_id)?;
    
    match msg.result {
        SubMsgResult::Ok(result) => {
            let token_address = extract_contract_address(&result)?;
            
            // Update both context and creation state
            pool_context.creator_token_addr = Some(token_address.clone());
            TEMP_POOL_CREATION.save(deps.storage, &pool_context)?;
            
            creation_state.creator_token_address = Some(token_address.clone());
            creation_state.status = CreationStatus::TokenCreated;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
            let nft_instantiate_msg = to_json_binary(&Cw721InstantiateMsg {
                name: "AMM LP Positions".to_string(),
                symbol: "AMM-LP".to_string(),
                minter: env.contract.address.to_string(),
            })?;
            
            let nft_msg = WasmMsg::Instantiate {
                code_id: config.cw721_nft_contract_id,
                msg: nft_instantiate_msg,
                funds: vec![],
                admin: Some(env.contract.address.to_string()),
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
            creation_state.status = CreationStatus::Failed;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            cleanup_temp_state(deps.storage)?;
            
            Err(ContractError::TokenCreationFailed {
                pool_id,
                reason: err,
            })
        }
    }
}

pub fn mint_create_pool(deps: DepsMut, env: Env, msg: Reply) -> Result<Response, ContractError> {
    let mut pool_context = TEMP_POOL_CREATION.load(deps.storage)?;
    let pool_id = pool_context.pool_id;
    let mut creation_state = POOL_CREATION_STATES.load(deps.storage, pool_id)?;
    
    match msg.result {
        SubMsgResult::Ok(result) => {
            let nft_address = extract_contract_address(&result)?;
            
            // Update context
            pool_context.nft_addr = Some(nft_address.clone());
            TEMP_POOL_CREATION.save(deps.storage, &pool_context)?;
            
            creation_state.mint_new_position_nft_address = Some(nft_address.clone());
            creation_state.status = CreationStatus::NftCreated;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
            let token_address = pool_context.creator_token_addr.clone()
                .ok_or_else(|| ContractError::Std(StdError::generic_err("missing token address")))?;
            
            // Configure threshold payouts
            let threshold_payout = ThresholdPayoutAmounts {
                creator_reward_amount: Uint128::new(325_000_000_000),
                bluechip_reward_amount: Uint128::new(25_000_000_000),
                pool_seed_amount: Uint128::new(350_000_000_000),
                commit_return_amount: Uint128::new(500_000_000_000),
            };
            
            // Validate the payout amounts
            threshold_payout.validate(Uint128::new(1_200_000_000_000))?;
            
            let threshold_binary = to_json_binary(&threshold_payout)?;
            
            // Update asset infos with actual token address
            let mut updated_asset_infos = pool_context.temp_pool_info.pool_token_info.clone();
            for asset_info in updated_asset_infos.iter_mut() {
                if let TokenType::CreatorToken { contract_addr } = asset_info {
                    if contract_addr.as_str() == "WILL_BE_CREATED_BY_FACTORY" {
                        *contract_addr = token_address.clone();
                    }
                }
            }       
            let pool_msg = WasmMsg::Instantiate {
                code_id: config.create_pool_wasm_contract_id,
                msg: to_json_binary(&CreatePoolReplyMsg {
                    pool_id,
                    pool_token_info: updated_asset_infos,
                    used_factory_addr: env.contract.address.clone(),
                    cw20_token_contract_id: config.cw20_token_contract_id,
                    threshold_payout: Some(threshold_binary),
                    commit_fee_info: CommitFeeInfo {
                        bluechip_wallet_address: config.bluechip_wallet_address.clone(),
                        creator_wallet_address: pool_context.temp_creator_wallet.clone(),
                        commit_fee_bluechip: config.commit_fee_bluechip,
                        commit_fee_creator: config.commit_fee_creator,
                    },
                    commit_amount_for_threshold: config.commit_amount_for_threshold_bluechip,
                    commit_threshold_limit_usd: config.commit_threshold_limit_usd,
                    token_address,
                    position_nft_address: nft_address.clone(),
                })?,
                funds: vec![],
                admin: Some(env.contract.address.to_string()),
                label: format!("Pool-{}", pool_id),
            };
            
            let sub_msg = SubMsg::reply_on_success(pool_msg, FINALIZE_POOL);
            
            Ok(Response::new()
                .add_attribute("action", "nft_created_successfully")
                .add_attribute("nft_address", nft_address)
                .add_attribute("pool_id", pool_id.to_string())
                .add_submessage(sub_msg))
        }
        SubMsgResult::Err(err) => {
            creation_state.status = CreationStatus::CleaningUp;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            let cleanup_msgs = create_cleanup_messages(&creation_state)?;
            
            Ok(Response::new()
                .add_submessages(cleanup_msgs)
                .add_attribute("action", "nft_creation_failed_cleanup")
                .add_attribute("pool_id", pool_id.to_string())
                .add_attribute("error", err))
        }
    }
}

pub fn finalize_pool(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    let pool_context = TEMP_POOL_CREATION.load(deps.storage)?;
    let pool_id = pool_context.pool_id;
    let mut creation_state = POOL_CREATION_STATES.load(deps.storage, pool_id)?;
    
    match msg.result {
        SubMsgResult::Ok(result) => {
            let pool_address = extract_contract_address(&result)?;
            
            creation_state.pool_address = Some(pool_address.clone());
            creation_state.status = CreationStatus::PoolCreated;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            let token_address = pool_context.creator_token_addr
                .ok_or_else(|| ContractError::Std(StdError::generic_err("missing token address")))?;
            let nft_address = pool_context.nft_addr
                .ok_or_else(|| ContractError::Std(StdError::generic_err("missing nft address")))?;
            
            // Create commit parameters
            let commit_info = CommitInfo {
                pool_id,
                creator: pool_context.temp_creator_wallet.clone(),
                creator_token_addr: token_address.clone(),
                creator_pool_addr: pool_address.clone(),
            };
            
            let pool_details = PoolDetails {
                pool_id,
                pool_token_info: pool_context.temp_pool_info.pool_token_info,
                creator_pool_addr: pool_address.clone(),
            };
            
            SETCOMMIT.save(deps.storage, &pool_context.temp_creator_wallet.to_string(), &commit_info)?;
            POOLS_BY_ID.save(deps.storage, pool_id, &pool_details)?;
            
            // Transfer ownership to pool
            let ownership_msgs = give_pool_ownership_cw20_and_nft(
                &token_address,
                &nft_address,
                &pool_address,
            )?;
            
            creation_state.status = CreationStatus::Completed;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            // Clean up temporary state
            cleanup_temp_state(deps.storage)?;
            
            Ok(Response::new()
                .add_messages(ownership_msgs)
                .add_attribute("action", "pool_created_successfully")
                .add_attribute("pool_address", pool_address)
                .add_attribute("pool_id", pool_id.to_string()))
        }
        SubMsgResult::Err(err) => {
            creation_state.status = CreationStatus::CleaningUp;
            POOL_CREATION_STATES.save(deps.storage, pool_id, &creation_state)?;
            
            let cleanup_msgs = create_cleanup_messages(&creation_state)?;
            
            Ok(Response::new()
                .add_submessages(cleanup_msgs)
                .add_attribute("action", "pool_creation_failed_cleanup")
                .add_attribute("pool_id", pool_id.to_string())
                .add_attribute("error", err))
        }
    }
}
