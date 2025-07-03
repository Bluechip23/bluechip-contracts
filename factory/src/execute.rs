use std::env;

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, MigrateMsg, TokenInfo, TokenInstantiateMsg};
use crate::pair::{FeeInfo, InstantiateMsg as PairInstantiateMsg};
use crate::state::{
    Config, CONFIG, SUBSCRIBE, TEMPCREATOR, TEMPPAIRINFO, TEMPTOKENADDR,
};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Reply, Response,
    StdError, StdResult, SubMsg, SubMsgResult, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, MinterResponse};

const CONTRACT_NAME: &str = "bluechip_factory";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTANTIATE_TOKEN_REPLY_ID: u64 = 1;
const INSTANTIATE_POOL_REPLY_ID: u64 = 2;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    /* Validate addresses */
    CONFIG.save(deps.storage, &msg.config)?;

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
        ExecuteMsg::UpdateConfig { config } => execute_update_config(deps, info, config),
        ExecuteMsg::Create {
            pair_msg,
            token_info,
        } => execute_create(deps, env, info, pair_msg, token_info),
    }
}

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    config: Config,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info)?;

    if config.total_token_amount
        != config.bluechip_amount
            + config.pool_amount
            + config.creator_amount
            + config.commit_amount
    {
        return Err(ContractError::WrongConfiguration {});
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

fn execute_create(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pair_msg: PairInstantiateMsg,
    token_info: TokenInfo,
) -> Result<Response, ContractError> {
    assert_is_admin(deps.as_ref(), info.clone())?;
    let config = CONFIG.load(deps.storage)?;
    let sender = info.sender;

    TEMPPAIRINFO.save(deps.storage, &pair_msg)?;
    TEMPCREATOR.save(deps.storage, &sender)?;
    let msg = WasmMsg::Instantiate {
        code_id: config.token_id,
        msg: to_json_binary(&TokenInstantiateMsg {
            name: token_info.name.clone(),
            symbol: token_info.symbol.clone(),
            decimals: 6,
            initial_balances: vec![],
            mint: Some(MinterResponse {
                minter: env.contract.address.to_string(),
                cap: Some(config.total_token_amount),
            }),
            marketing: None,
        })?,
        funds: vec![],
        admin: None,
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
            let temp_pool_info = TEMPPAIRINFO.load(deps.storage)?;
            let temp_creator = TEMPCREATOR.load(deps.storage)?;

            // ✅ Extract contract address from reply
            let raw_bytes = match msg.result {
                SubMsgResult::Ok(result) => result.data.ok_or_else(|| {
                    StdError::generic_err("Reply data missing in token instantiation")
                })?,
                SubMsgResult::Err(err) => {
                    return Err(StdError::generic_err(format!(
                        "Token instantiation failed: {}",
                        err
                    ))
                    .into())
                }
            };

            let contract_addr_str = std::str::from_utf8(&raw_bytes)
                .map_err(|_| StdError::parse_err("UTF-8", "Failed to decode reply data"))?;
            let token_address = deps.api.addr_validate(contract_addr_str)?;

            // Save token address
            TEMPTOKENADDR.save(deps.storage, &token_address)?;

            // Instantiate the pair contract
            let msg = WasmMsg::Instantiate {
                code_id: config.pair_id,
                msg: to_json_binary(&PairInstantiateMsg {
                    asset_infos: temp_pool_info.asset_infos,
                    factory_addr: env.contract.address.to_string(),
                    token_code_id: config.token_id,
                    init_params: None,
                    fee_info: FeeInfo {
                        bluechip_address: config.bluechip_address.clone(),
                        creator_address: temp_creator.clone(),
                        bluechip_fee: config.bluechipe_fee,
                        creator_fee: config.creator_fee,
                    },
                    commit_amount: config.commit_amount,
                    commit_limit_usd: config.commit_limit_usd,
                    commit_limit: config.commit_limit,
                    creator_amount: config.creator_amount,
                    bluechip_amount: config.bluechip_amount,
                    pool_amount: config.pool_amount,
                    oracle_addr: config.oracle_addr.clone(),
                    oracle_symbol: config.oracle_symbol.clone(),
                    token_address: token_address.clone(),
                })?,
                funds: vec![],
                admin: None,
                label: "Pair".to_string(),
            };

            let sub_msg = SubMsg::reply_on_success(msg, INSTANTIATE_POOL_REPLY_ID);

            Ok(Response::new()
                .add_attribute("action", "instantiate_token_reply")
                .add_attribute("token_address", token_address)
                .add_submessage(sub_msg))
        }

        INSTANTIATE_POOL_REPLY_ID => {
            let config = CONFIG.load(deps.storage)?;
            let temp_creator = TEMPCREATOR.load(deps.storage)?;
            let temp_token_address = TEMPTOKENADDR.load(deps.storage)?;

            let raw_bytes = match msg.result {
                SubMsgResult::Ok(result) => result.data.ok_or_else(|| {
                    StdError::generic_err("Reply data missing in pool instantiation")
                })?,
                SubMsgResult::Err(err) => {
                    return Err(StdError::generic_err(format!(
                        "Pool instantiation failed: {}",
                        err
                    ))
                    .into())
                }
            };

            let contract_addr_str = std::str::from_utf8(&raw_bytes)
                .map_err(|_| StdError::parse_err("UTF-8", "Failed to decode reply data"))?;
            let pool_address = deps.api.addr_validate(contract_addr_str)?;

            // Save subscribe info
            SUBSCRIBE.save(
                deps.storage,
                &temp_creator.to_string(),
                &crate::state::SubscribeInfo {
                    creator: temp_creator.clone(),
                    token_addr: temp_token_address.clone(),
                    pool_addr: pool_address.clone(),
                },
            )?;

            // Mint tokens
            let mut messages: Vec<CosmosMsg> = vec![];
            messages.push(mint_tokens(
                &temp_token_address,
                &temp_creator,
                config.creator_amount,
            )?);
            messages.push(mint_tokens(
                &temp_token_address,
                &config.bluechip_address,
                config.bluechip_amount,
            )?);
            messages.push(mint_tokens(
                &temp_token_address,
                &pool_address,
                config.commit_amount + config.pool_amount,
            )?);

            Ok(Response::new()
                .add_attribute("action", "instantiate_pool_reply")
                .add_attribute("pool_address", pool_address)
                .add_messages(messages))
        }

        _ => Err(StdError::generic_err(format!("Unknown reply ID: {}", msg.id)).into()),
    }
}

pub fn get_cw20_transfer_msg(
    token_addr: &Addr,
    recipient: &Addr,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_cw20_msg = Cw20ExecuteMsg::Transfer {
        recipient: recipient.into(),
        amount,
    };

    let exec_cw20_transfer_msg = WasmMsg::Execute {
        contract_addr: token_addr.into(),
        msg: to_json_binary(&transfer_cw20_msg)?,
        funds: vec![],
    };

    let cw20_transfer_msg: CosmosMsg = exec_cw20_transfer_msg.into();
    Ok(cw20_transfer_msg)
}

pub fn get_cw20_transfer_from_msg(
    token_addr: &Addr,
    owner: &Addr,
    recipient: &Addr,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_cw20_msg = Cw20ExecuteMsg::TransferFrom {
        owner: owner.into(),
        recipient: recipient.into(),
        amount,
    };

    let exec_cw20_transfer_msg = WasmMsg::Execute {
        contract_addr: token_addr.into(),
        msg: to_json_binary(&transfer_cw20_msg)?,
        funds: vec![],
    };

    let cw20_transfer_msg: CosmosMsg = exec_cw20_transfer_msg.into();
    Ok(cw20_transfer_msg)
}

pub fn get_cw20_burn_from_msg(
    token_addr: &Addr,
    owner: &Addr,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let burn_cw20_msg = Cw20ExecuteMsg::BurnFrom {
        owner: owner.into(),
        amount,
    };
    let exec_cw20_burn_msg = WasmMsg::Execute {
        contract_addr: token_addr.into(),
        msg: to_json_binary(&burn_cw20_msg)?,
        funds: vec![],
    };

    let cw20_burn_msg: CosmosMsg = exec_cw20_burn_msg.into();
    Ok(cw20_burn_msg)
}

pub fn mint_tokens(token_addr: &Addr, recipient: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    let burn_cw20_msg = Cw20ExecuteMsg::Mint {
        recipient: recipient.to_string(),
        amount,
    };
    let exec_cw20_burn_msg = WasmMsg::Execute {
        contract_addr: token_addr.into(),
        msg: to_json_binary(&burn_cw20_msg)?,
        funds: vec![],
    };

    let cw20_burn_msg: CosmosMsg = exec_cw20_burn_msg.into();
    Ok(cw20_burn_msg)
}

pub fn get_bank_transfer_to_msg(
    recipient: &Addr,
    denom: &str,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_bank_msg = cosmwasm_std::BankMsg::Send {
        to_address: recipient.into(),
        amount: vec![Coin {
            denom: denom.to_string(),
            amount,
        }],
    };

    let transfer_bank_cosmos_msg: CosmosMsg = transfer_bank_msg.into();
    Ok(transfer_bank_cosmos_msg)
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
