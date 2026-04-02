use crate::error::ContractError;
use crate::generic_helpers::{check_rate_limit, enforce_transaction_deadline};
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees, calc_liquidity_for_deposit,
    calculate_fee_size_multiplier, calculate_fees_owed, check_ratio_deviation, check_slippage,
    sync_position_on_transfer, verify_position_ownership,
};
use crate::asset::get_bluechip_denom;
use crate::state::{
    PoolInfo, PoolSpecs, MINIMUM_LIQUIDITY, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE,
    RATE_LIMIT_GUARD,
};
use crate::state::{Position, TokenMetadata, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, OWNER_POSITIONS};
use crate::swap_helper::update_price_accumulator;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, Timestamp, Uint128, WasmMsg,
};
use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};

struct DepositPrep {
    pool_info: PoolInfo,
    native_denom: String,
    liquidity: Uint128,
    actual_amount0: Uint128,
    actual_amount1: Uint128,
    refund_amount: Uint128,
}

fn prepare_deposit(
    deps: Deps,
    info: &MessageInfo,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<DepositPrep, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let native_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;
    let paid_bluechip = info
        .funds
        .iter()
        .find(|c| c.denom == native_denom)
        .map(|c| c.amount)
        .unwrap_or_default();

    let (liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps, amount0, amount1)?;

    if paid_bluechip < actual_amount0 {
        return Err(ContractError::InvalidNativeAmount {
            expected: actual_amount0,
            actual: paid_bluechip,
        });
    }

    check_slippage(actual_amount0, min_amount0, "bluechip")?;
    check_slippage(actual_amount1, min_amount1, "cw20")?;

    let refund_amount = if paid_bluechip > actual_amount0 {
        paid_bluechip - actual_amount0
    } else {
        Uint128::zero()
    };

    Ok(DepositPrep {
        pool_info,
        native_denom,
        liquidity,
        actual_amount0,
        actual_amount1,
        refund_amount,
    })
}

fn build_deposit_transfer_msgs(
    pool_info: &PoolInfo,
    sender: &Addr,
    contract_addr: &Addr,
    native_denom: &str,
    actual_amount1: Uint128,
    refund_amount: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut messages = vec![];

    if !actual_amount1.is_zero() {
        let transfer_cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                owner: sender.to_string(),
                recipient: contract_addr.to_string(),
                amount: actual_amount1,
            })?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(transfer_cw20_msg));
    }

    if !refund_amount.is_zero() {
        let refund_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![Coin {
                denom: native_denom.to_string(),
                amount: refund_amount,
            }],
        };
        messages.push(CosmosMsg::Bank(refund_msg));
    }

    Ok(messages)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_deposit_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let prep = prepare_deposit(deps.as_ref(), &info, amount0, amount1, min_amount0, min_amount1)?;

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    let mut messages = vec![];
    if !pool_state.nft_ownership_accepted {
        let accept_msg = WasmMsg::Execute {
            contract_addr: prep.pool_info.position_nft_address.to_string(),
            msg: to_json_binary(&Cw721ExecuteMsg::<()>::UpdateOwnership(
                Action::AcceptOwnership,
            ))?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(accept_msg));
        pool_state.nft_ownership_accepted = true;
    }

    let transfer_msgs = build_deposit_transfer_msgs(
        &prep.pool_info,
        &info.sender,
        &env.contract.address,
        &prep.native_denom,
        prep.actual_amount1,
        prep.refund_amount,
    )?;
    messages.extend(transfer_msgs);

    let mut pos_id = NEXT_POSITION_ID.load(deps.storage)?;
    pos_id = pos_id.checked_add(1).ok_or_else(|| ContractError::Std(StdError::generic_err("Position ID overflow")))?;
    NEXT_POSITION_ID.save(deps.storage, &pos_id)?;
    let position_id = pos_id.to_string();

    let metadata = TokenMetadata {
        name: Some(format!("LP Position #{}", position_id)),
        description: Some("Pool Liquidity Position".to_string()),
    };
    let mint_liquidity_nft = WasmMsg::Execute {
        contract_addr: prep.pool_info.position_nft_address.to_string(),
        msg: to_json_binary(&Cw721ExecuteMsg::<TokenMetadata>::Mint {
            token_id: position_id.clone(),
            owner: user.to_string(),
            token_uri: None,
            extension: metadata,
        })?,
        funds: vec![],
    };
    messages.push(CosmosMsg::Wasm(mint_liquidity_nft));
    let fee_size_multiplier = calculate_fee_size_multiplier(prep.liquidity);
    let position = Position {
        liquidity: prep.liquidity,
        owner: user.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier,
        unclaimed_fees_0: Uint128::zero(),
        unclaimed_fees_1: Uint128::zero(),
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;
    OWNER_POSITIONS.save(deps.storage, (&user, &position_id), &true)?;

    pool_state.reserve0 = pool_state.reserve0.checked_add(prep.actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(prep.actual_amount1)?;
    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(prep.liquidity)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    let unpaused = pool_state.reserve0 >= MINIMUM_LIQUIDITY && pool_state.reserve1 >= MINIMUM_LIQUIDITY;
    if unpaused {
        POOL_PAUSED.save(deps.storage, &false)?;
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "deposit_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("depositor", user)
        .add_attribute("liquidity", prep.liquidity.to_string())
        .add_attribute("actual_amount0", prep.actual_amount0.to_string())
        .add_attribute("actual_amount1", prep.actual_amount1.to_string())
        .add_attribute("refunded_amount0", prep.refund_amount.to_string())
        .add_attribute("offered_amount0", amount0.to_string())
        .add_attribute("offered_amount1", amount1.to_string())
        .add_attribute(
            "pool_unpaused",
            if unpaused { "true" } else { "false" },
        ))
}

pub fn execute_collect_fees(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;
    let (fees_owed_0, fees_owed_1) = calc_capped_fees(&liquidity_position, &pool_fee_state)?;

    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.unclaimed_fees_0 = Uint128::zero();
    liquidity_position.unclaimed_fees_1 = Uint128::zero();

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_fee_state.fee_reserve_0 = pool_fee_state.fee_reserve_0.checked_sub(fees_owed_0)?;
    pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_sub(fees_owed_1)?;

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    let fee_msgs = build_fee_transfer_msgs(&pool_info, &info.sender, fees_owed_0, fees_owed_1)?;

    Ok(Response::new()
        .add_messages(fee_msgs)
        .add_attribute("action", "collect_fees")
        .add_attribute("position_id", position_id)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1))
}

#[allow(clippy::too_many_arguments)]
pub fn add_to_position(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    position_id: String,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let prep = prepare_deposit(deps.as_ref(), &info, amount0, amount1, min_amount0, min_amount1)?;

    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    verify_position_ownership(
        deps.as_ref(),
        &prep.pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;
    // Collect pending fees before adding new liquidity to reset accounting.
    let (fees_owed_0, fees_owed_1) = calc_capped_fees(&liquidity_position, &pool_fee_state)?;

    let mut messages = build_deposit_transfer_msgs(
        &prep.pool_info,
        &info.sender,
        &env.contract.address,
        &prep.native_denom,
        prep.actual_amount1,
        prep.refund_amount,
    )?;

    liquidity_position.liquidity = liquidity_position.liquidity.checked_add(prep.liquidity)?;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_size_multiplier =
        calculate_fee_size_multiplier(liquidity_position.liquidity);
    liquidity_position.unclaimed_fees_0 = Uint128::zero();
    liquidity_position.unclaimed_fees_1 = Uint128::zero();

    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(prep.liquidity)?;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_fee_state.fee_reserve_0 = pool_fee_state.fee_reserve_0.checked_sub(fees_owed_0)?;
    pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_sub(fees_owed_1)?;

    pool_state.reserve0 = pool_state.reserve0.checked_add(prep.actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(prep.actual_amount1)?;

    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    let mut response = Response::new()
        .add_attribute("action", "add_to_position")
        .add_attribute("position_id", position_id)
        .add_attribute("additional_liquidity", prep.liquidity.to_string())
        .add_attribute("total_liquidity", liquidity_position.liquidity.to_string())
        .add_attribute("amount0_requested", amount0)
        .add_attribute("amount1_requested", amount1)
        .add_attribute("actual_amount0_added", prep.actual_amount0.to_string())
        .add_attribute("actual_amount1_added", prep.actual_amount1.to_string())
        .add_attribute("refunded_amount0", prep.refund_amount.to_string())
        .add_attribute("fees_collected_0", fees_owed_0)
        .add_attribute("fees_collected_1", fees_owed_1);
    let fee_msgs = build_fee_transfer_msgs(&prep.pool_info, &user, fees_owed_0, fees_owed_1)?;
    messages.extend(fee_msgs);
    response = response.add_messages(messages);

    Ok(response)
}

pub fn remove_all_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;

    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    if pool_state.total_liquidity.is_zero() {
        return Err(ContractError::Std(StdError::generic_err("Pool total liquidity is zero")));
    }
    let user_share_0 =
        current_reserve0.multiply_ratio(liquidity_position.liquidity, pool_state.total_liquidity);
    let user_share_1 =
        current_reserve1.multiply_ratio(liquidity_position.liquidity, pool_state.total_liquidity);
    check_slippage(user_share_0, min_amount0, "bluechip")?;
    check_slippage(user_share_1, min_amount1, "cw20")?;
    check_ratio_deviation(user_share_0, user_share_1, min_amount0, min_amount1, max_ratio_deviation_bps)?;
    let (fees_owed_0, fees_owed_1) = calc_capped_fees(&liquidity_position, &pool_fee_state)?;

    let total_amount_0 = user_share_0.checked_add(fees_owed_0)?;
    let total_amount_1 = user_share_1.checked_add(fees_owed_1)?;

    let liquidity_to_subtract = liquidity_position.liquidity;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_subtract)?;

    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(user_share_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(user_share_1)?;
    pool_fee_state.fee_reserve_0 = pool_fee_state.fee_reserve_0.checked_sub(fees_owed_0)?;
    pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_sub(fees_owed_1)?;

    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.remove(deps.storage, &position_id);
    OWNER_POSITIONS.remove(deps.storage, (&info.sender, &position_id));
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

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
    let transfer_msgs = build_fee_transfer_msgs(&pool_info, &info.sender, total_amount_0, total_amount_1)?;
    response = response.add_messages(transfer_msgs);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub fn remove_partial_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    sync_position_on_transfer(
        deps.storage,
        &mut liquidity_position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;

    if liquidity_to_remove.is_zero() {
        return Err(ContractError::InvalidAmount {});
    }
    if liquidity_to_remove == liquidity_position.liquidity {
        return execute_remove_all_liquidity(
            deps.branch(),
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        );
    }
    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    // Only calculate fees on the portion being removed.
    let fees_owed_0 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    )?;

    let fees_owed_1 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    )?;

    // Preserve fees on the remaining portion so resetting the snapshot
    // below doesn't discard them.
    let remaining_liquidity = liquidity_position.liquidity.checked_sub(liquidity_to_remove)?;
    let preserved_fees_0 = calculate_fees_owed(
        remaining_liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_size_multiplier,
    )?;
    let preserved_fees_1 = calculate_fees_owed(
        remaining_liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_size_multiplier,
    )?;

    if pool_state.total_liquidity.is_zero() {
        return Err(ContractError::Std(StdError::generic_err("Pool total liquidity is zero")));
    }
    let withdrawal_amount_0 =
        current_reserve0.multiply_ratio(liquidity_to_remove, pool_state.total_liquidity);

    let withdrawal_amount_1 =
        current_reserve1.multiply_ratio(liquidity_to_remove, pool_state.total_liquidity);

    let fees_owed_0 = fees_owed_0.min(pool_fee_state.fee_reserve_0);
    let fees_owed_1 = fees_owed_1.min(pool_fee_state.fee_reserve_1);

    check_slippage(withdrawal_amount_0, min_amount0, "bluechip")?;
    check_slippage(withdrawal_amount_1, min_amount1, "cw20")?;
    check_ratio_deviation(withdrawal_amount_0, withdrawal_amount_1, min_amount0, min_amount1, max_ratio_deviation_bps)?;
    let total_amount_0 = withdrawal_amount_0.checked_add(fees_owed_0)?;
    let total_amount_1 = withdrawal_amount_1.checked_add(fees_owed_1)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(withdrawal_amount_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(withdrawal_amount_1)?;
    pool_fee_state.fee_reserve_0 = pool_fee_state.fee_reserve_0.checked_sub(fees_owed_0)?;
    pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_sub(fees_owed_1)?;

    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_remove)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;

    liquidity_position.unclaimed_fees_0 = liquidity_position.unclaimed_fees_0
        .checked_add(preserved_fees_0)?;
    liquidity_position.unclaimed_fees_1 = liquidity_position.unclaimed_fees_1
        .checked_add(preserved_fees_1)?;

    liquidity_position.liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;

    liquidity_position.fee_size_multiplier =
        calculate_fee_size_multiplier(liquidity_position.liquidity);

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
        .add_attribute("preserved_fees_0", preserved_fees_0)
        .add_attribute("preserved_fees_1", preserved_fees_1)
        .add_attribute("total_0", total_amount_0)
        .add_attribute("total_1", total_amount_1);
    let transfer_msgs = build_fee_transfer_msgs(&pool_info, &info.sender, total_amount_0, total_amount_1)?;
    response = response.add_messages(transfer_msgs);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_add_to_position(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    add_to_position(
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
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_all_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    remove_all_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_partial_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    remove_partial_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        liquidity_to_remove,
        transaction_deadline,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_remove_partial_liquidity_by_percent(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    percentage: u64,
    transaction_deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<Response, ContractError> {
    if percentage == 0 {
        return Err(ContractError::InvalidPercent {});
    }

    if percentage >= 100 {
        return execute_remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        );
    }

    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    let liquidity_to_remove = liquidity_position
        .liquidity
        .checked_mul(Uint128::from(percentage))?
        .checked_div(Uint128::from(100u128))
        .map_err(|_| ContractError::DivideByZero)?;
    execute_remove_partial_liquidity(
        deps,
        env,
        info,
        position_id,
        liquidity_to_remove,
        transaction_deadline,
        min_amount0,
        min_amount1,
        max_ratio_deviation_bps,
    )
}
