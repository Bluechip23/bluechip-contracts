#![allow(non_snake_case)]
use crate::error::ContractError;
use crate::generic_helpers::{check_rate_limit, enforce_transaction_deadline};
use crate::liquidity_helpers::{
    calc_liquidity_for_deposit, calculate_fee_size_multiplier, calculate_fees_owed,
    verify_position_ownership,
};
use crate::state::{
    PoolSpecs, POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE, RATE_LIMIT_GUARD,
};
use crate::state::{Position, TokenMetadata, LIQUIDITY_POSITIONS, NEXT_POSITION_ID};
use crate::swap_helper::update_price_accumulator;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, DepsMut, Empty, Env, MessageInfo, Response,
    Timestamp, Uint128, WasmMsg,
};
use cw721_base::ExecuteMsg as CW721BaseExecuteMsg;

use std::vec;

//deposit liquidity in pool
pub fn execute_deposit_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    amount0: Uint128, // bluechip amount
    amount1: Uint128, // CW20 amount
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    const NATIVE_DENOM: &str = "stake";
    let paid_bluechip = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();
    // calculate actual amounts needed to maintain pool ratio
    let (liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;
    // Ensure the user sent enough bluechip tokens
    if paid_bluechip < actual_amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }
    //slippage check
    if let Some(min0) = min_amount0 {
        if actual_amount0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: actual_amount0,
                token: "bluechip".to_string(),
            });
        }
    }
    if let Some(min1) = min_amount1 {
        if actual_amount1 < min1 {
            return Err(ContractError::SlippageExceeded {
                expected: min1,
                actual: actual_amount1,
                token: "cw20".to_string(),
            });
        }
    }

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    let mut messages = vec![];
    // accept NFT ownership if this is the first deposit
    if !pool_state.nft_ownership_accepted {
        let accept_msg = WasmMsg::Execute {
            contract_addr: pool_info.position_nft_address.to_string(),
            msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
                cw721_base::Action::AcceptOwnership {},
            ))?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(accept_msg));
        pool_state.nft_ownership_accepted = true;
    }

    // Transfer only the actual CW20 amount needed
    if !actual_amount1.is_zero() {
        let transfer_cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                owner: info.sender.to_string(),
                recipient: env.contract.address.to_string(),
                amount: actual_amount1, // Use actual amount, not requested
            })?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(transfer_cw20_msg));
    }

    // Refund excess bluechip tokens
    let refund_amount = paid_bluechip.checked_sub(actual_amount0)?;
    if !refund_amount.is_zero() {
        let refund_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: NATIVE_DENOM.to_string(),
                amount: refund_amount,
            }],
        };
        messages.push(CosmosMsg::Bank(refund_msg));
    }
    //incriment nft id
    let mut pos_id = NEXT_POSITION_ID.load(deps.storage)?;
    pos_id += 1;
    NEXT_POSITION_ID.save(deps.storage, &pos_id)?;
    let position_id = pos_id.to_string();

    let metadata = TokenMetadata {
        name: Some(format!("LP Position #{}", position_id)),
        description: Some(format!("Pool Liquidity Position")),
    };
    //mint nft position
    let mint_liquidity_nft = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(),
        msg: to_json_binary(
            &CW721BaseExecuteMsg::<TokenMetadata, cosmwasm_std::Empty>::Mint {
                token_id: position_id.clone(),
                owner: user.to_string(),
                token_uri: None,
                extension: metadata,
            },
        )?,
        funds: vec![],
    };
    messages.push(CosmosMsg::Wasm(mint_liquidity_nft));
    let fee_size_multiplier = calculate_fee_size_multiplier(liquidity);
    let position = Position {
        liquidity,
        owner: user.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier,
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;

    pool_state.reserve0 = pool_state.reserve0.checked_add(actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(actual_amount1)?;
    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(liquidity)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "deposit_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("depositor", user)
        .add_attribute("liquidity", liquidity.to_string())
        .add_attribute("actual_amount0", actual_amount0.to_string())
        .add_attribute("actual_amount1", actual_amount1.to_string())
        .add_attribute("refunded_amount0", refund_amount.to_string())
        .add_attribute("offered_amount0", amount0.to_string())
        .add_attribute("offered_amount1", amount1.to_string()))
}

pub fn execute_collect_fees(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    );

    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    let mut response = Response::new()
        .add_attribute("action", "collect_fees")
        .add_attribute("position_id", position_id)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1);

    if !fees_owed_0.is_zero() {
        let bluechip_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: fees_owed_0,
            }],
        };
        response = response.add_message(bluechip_msg);
    }

    if !fees_owed_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: info.sender.to_string(),
                amount: fees_owed_1,
            })?,
            funds: vec![],
        };
        response = response.add_message(cw20_msg);
    }

    Ok(response)
}

//add liquidity to an already exisitnng position and collects fees for accounting
pub fn add_to_position(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    position_id: String,
    //bluechip token
    amount0: Uint128,
    //creator token
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    const NATIVE_DENOM: &str = "stake";
    let paid_bluechip = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    //make sure position belongs to wallet sending new funds
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    let (additional_liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;

    if paid_bluechip < actual_amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let mut messages: Vec<CosmosMsg> = vec![];
    //send accumulated fees to reset accounting - collect before adding new liquidity
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    );
    //check slippage
    if let Some(min0) = min_amount0 {
        if actual_amount0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: actual_amount0,
                token: "bluechip".to_string(),
            });
        }
    }

    if let Some(min1) = min_amount1 {
        if actual_amount1 < min1 {
            return Err(ContractError::SlippageExceeded {
                expected: min1,
                actual: actual_amount1,
                token: "cw20".to_string(),
            });
        }
    }
    //send the appropraite amount of both assets to the pool for the liquidity position
    if !actual_amount1.is_zero() {
        let transfer_cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                owner: info.sender.to_string(),
                recipient: env.contract.address.to_string(),
                amount: actual_amount1,
            })?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(transfer_cw20_msg));
    }

    let refund_amount = paid_bluechip.checked_sub(actual_amount0)?;
    if !refund_amount.is_zero() {
        let refund_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: NATIVE_DENOM.to_string(),
                amount: refund_amount,
            }],
        };
        messages.push(CosmosMsg::Bank(refund_msg));
    }
    //update position with new liquidity
    liquidity_position.liquidity += additional_liquidity;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_size_multiplier = calculate_fee_size_multiplier(liquidity_position.liquidity);

    pool_state.total_liquidity += additional_liquidity;

    // add actual deposit amounts
    pool_state.reserve0 = pool_state.reserve0.checked_add(actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(actual_amount1)?;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    let mut response = Response::new()
        .add_messages(messages)
        .add_attribute("action", "add_to_position")
        .add_attribute("position_id", position_id)
        .add_attribute("additional_liquidity", additional_liquidity.to_string())
        .add_attribute("total_liquidity", liquidity_position.liquidity.to_string())
        .add_attribute("amount0_requested", amount0)
        .add_attribute("amount1_requested", amount1)
        .add_attribute("actual_amount0_added", actual_amount0.to_string())
        .add_attribute("actual_amount1_added", actual_amount1.to_string())
        .add_attribute("refunded_amount0", refund_amount.to_string())
        .add_attribute("fees_collected_0", fees_owed_0)
        .add_attribute("fees_collected_1", fees_owed_1);
    //actually send fees
    if !fees_owed_0.is_zero() {
        let bluechip_msg = BankMsg::Send {
            to_address: user.to_string(),
            amount: vec![Coin {
                denom: NATIVE_DENOM.to_string(),
                amount: fees_owed_0,
            }],
        };
        response = response.add_message(bluechip_msg);
    }

    if !fees_owed_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: user.to_string(),
                amount: fees_owed_1,
            })?,
            funds: vec![],
        };
        response = response.add_message(cw20_msg);
    }

    Ok(response)
}

// Functionality for user to remove all their liquidity - collect all fees associated with position and deactivate the position
pub fn remove_all_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {

    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    let user_share_0 =
        (liquidity_position.liquidity * current_reserve0) / pool_state.total_liquidity;
    let user_share_1 =
        (liquidity_position.liquidity * current_reserve1) / pool_state.total_liquidity;
    //protect against slippage and error out transaction
    if let Some(min0) = min_amount0 {
        if user_share_0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: user_share_0,
                token: "bluechip".to_string(),
            });
        }
    }

    if let Some(min1) = min_amount1 {
        if user_share_1 < min1 {
            return Err(ContractError::SlippageExceeded {
                expected: min1,
                actual: user_share_1,
                token: "cw20".to_string(),
            });
        }
    }
    //calculate the fees owed to the position and prepare for collection
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    );

    let total_amount_0 = user_share_0 + fees_owed_0;
    let total_amount_1 = user_share_1 + fees_owed_1;

    let liquidity_to_subtract = liquidity_position.liquidity;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_subtract)?;

    /* let burn_msg = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(), // External NFT contract
        msg: to_json_binary(&cw721::Cw721ExecuteMsg::Burn {
            token_id: position_id.clone(),
        })?,
        funds: vec![],
    };*/

    //update pool fees, collect fees, and reserve prices
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(user_share_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(user_share_1)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.remove(deps.storage, &position_id);

    let mut response = Response::new()
        .add_attribute("action", "remove_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute(
            "liquidity_removed",
            liquidity_position.liquidity.to_string(),
        )
        .add_attribute("principal_0", user_share_0)
        .add_attribute("principal_1", user_share_1)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1)
        .add_attribute("total_0", total_amount_0)
        .add_attribute("total_1", total_amount_1);
    //redeem the tokens correlated with the users positions
    if !total_amount_0.is_zero() {
        let bluechip_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: total_amount_0,
            }],
        };
        response = response.add_message(bluechip_msg);
    }

    if !total_amount_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: info.sender.to_string(),
                amount: total_amount_1,
            })?,
            funds: vec![],
        };
        response = response.add_message(cw20_msg);
    }

    Ok(response)
}

pub fn remove_partial_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    // Specific amount of liquidity to remove
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    if liquidity_to_remove.is_zero() {
        return Err(ContractError::InvalidAmount {});
    }
    //if user removes all their liquidity.
    if liquidity_to_remove == liquidity_position.liquidity {
        return execute_remove_all_liquidity(
            deps.branch(),
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
        );
    }
    //cant take out what you dont have.
    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    let fees_owed_0 = calculate_fees_owed(
        //only considers the amount of liquidity being removed when collecting fees
        liquidity_to_remove,
        //everything else remains the same
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        //only considers the amount of liquidity being removed when collecting fees
        liquidity_to_remove,
        //everything else remains the same
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    );
    //finds total amount based on the amount of liquidity the user would like to remove.
    let withdrawal_amount_0 = liquidity_to_remove
        .checked_mul(current_reserve0)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    let withdrawal_amount_1 = liquidity_to_remove
        .checked_mul(current_reserve1)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    //add amounts to transfer back to user
    let total_amount_0 = withdrawal_amount_0 + fees_owed_0;
    let total_amount_1 = withdrawal_amount_1 + fees_owed_1;
    //update state
    pool_state.reserve0 = pool_state.reserve0.checked_sub(withdrawal_amount_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(withdrawal_amount_1)?;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_remove)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    liquidity_position.liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;

    let mut response = Response::new()
        .add_attribute("action", "remove_partial_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("liquidity_removed", liquidity_to_remove.to_string())
        .add_attribute(
            "remaining_liquidity",
            liquidity_position.liquidity.to_string(),
        )
        .add_attribute("principal_0", withdrawal_amount_0)
        .add_attribute("principal_1", withdrawal_amount_1)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1)
        .add_attribute("total_0", total_amount_0)
        .add_attribute("total_1", total_amount_1);
    //send assets back to user.
    if !total_amount_0.is_zero() {
        let bluechip_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: total_amount_0,
            }],
        };
        response = response.add_message(bluechip_msg);
    }

    if !total_amount_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: info.sender.to_string(),
                amount: total_amount_1,
            })?,
            funds: vec![],
        };
        response = response.add_message(cw20_msg);
    }

    Ok(response)
}

pub fn execute_add_to_position(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128, // bluechip token
    amount1: Uint128, // cw20 token
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;

    // prohibit spam liquidity
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = add_to_position(
        &mut deps,
        env,
        info.clone(),
        sender.clone(),
        position_id,
        amount0,
        amount1,
        min_amount0,
        min_amount1,
        transaction_deadline,
    );
    result
}

pub fn execute_remove_all_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();
    //spam protection
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = remove_all_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        min_amount0,
        min_amount1,
    );
    result
}

pub fn execute_remove_partial_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = remove_partial_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        liquidity_to_remove,
        transaction_deadline,
        min_amount0,
        min_amount1,
    );

    result
}

//same as remove partial liquidity but with a percent instead of a whole number
pub fn execute_remove_partial_liquidity_by_percent(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    //using percentage
    percentage: u64,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    // cant remove zero 
    if percentage == 0 {
        return Err(ContractError::InvalidPercent {});
    }

    if percentage >= 100 {
        // redirect to full removal
        return execute_remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
        );
    }

    // load position to calculate absolute amount
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    // convert percentage to whole number to use in execute_remote_partial_liquidity
    let liquidity_to_remove = liquidity_position
        .liquidity
        .checked_mul(Uint128::from(percentage))?
        .checked_div(Uint128::from(100u128))
        .map_err(|_| ContractError::DivideByZero)?;
    // Call the main partial removal function
    execute_remove_partial_liquidity(
        deps,
        env,
        info,
        position_id,
        //converted from decimal
        liquidity_to_remove,
        transaction_deadline,
        min_amount0,
        min_amount1,
    )
}
