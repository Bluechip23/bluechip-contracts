//! First-time LP deposit handler + the shared deposit plumbing used by
//! `add_to_position` too.
//!
//! `prepare_deposit` runs the checks common to any liquidity-in
//! operation: ratio matching, slippage bounds, per-asset fund
//! collection (Native -> BankMsg refund on overpayment,
//! CW20 -> Cw20ExecuteMsg::TransferFrom), and returns a `DepositPrep`
//! bundle for the caller. `execute_deposit_liquidity` then uses that
//! bundle to mint a fresh position NFT + credit the LP.
//!
//! Both `DepositPrep` and `prepare_deposit` are `pub(crate)` so
//! `super::add::add_to_position` can reuse them without re-implementing
//! the collection logic.

use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, Timestamp, Uint128, WasmMsg,
};
use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};

use crate::asset::TokenType;
use crate::error::ContractError;
use crate::generic::enforce_transaction_deadline;
use crate::liquidity_helpers::{
    calc_liquidity_for_deposit, calculate_fee_size_multiplier, check_slippage,
};
use crate::state::{
    PoolInfo, Position, TokenMetadata, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, NEXT_POSITION_ID,
    OWNER_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_STATE,
};
use crate::swap::update_price_accumulator;

/// Everything prepare_deposit discovers up front: how much liquidity the
/// deposit will produce, how much of each side was actually used (vs the
/// offered `amount0`/`amount1` — ratio-matching may clamp one side), and
/// the exact list of CosmosMsgs needed to move tokens into position
/// (TransferFrom for CW20 sides, BankMsg refunds for over-paid native
/// sides).
///
/// `collect_msgs` is pair-shape agnostic: for each of the two asset
/// positions we dispatch on `TokenType` and emit the appropriate
/// collection/refund message. Native/CW20, Native/Native, and CW20/CW20
/// pools all produce a correct list.
pub(crate) struct DepositPrep {
    pub pool_info: PoolInfo,
    pub liquidity: Uint128,
    pub actual_amount0: Uint128,
    pub actual_amount1: Uint128,
    pub collect_msgs: Vec<CosmosMsg>,
    /// Over-payment on the asset-0 side (always 0 unless asset 0 is
    /// `Native` AND the caller attached more of that denom than
    /// `actual_amount0` needed). Preserved for the `refunded_amount0`
    /// response attribute existing external tooling (logs, tests) parses.
    pub refund_amount0: Uint128,
    /// Same for asset-1. New in H14 Commit 4b — prior to the refactor
    /// asset 1 was always CW20, so refund semantics didn't apply.
    pub refund_amount1: Uint128,
}

/// For a single asset position, emit the CosmosMsgs needed to pull
/// `amount` into the pool contract and return the over-payment refund:
///   - `Native`: verify `info.funds` covers at least `amount` of the
///     denom; emit a BankMsg refund for the overpayment (if any) back
///     to the sender; returns the refunded amount.
///   - `CreatorToken`: emit a `Cw20ExecuteMsg::TransferFrom` so the pool
///     pulls exactly `amount` from the sender (requires prior allowance);
///     always returns 0 (no refund concept for CW20 TransferFrom).
fn collect_deposit_side(
    asset_info: &TokenType,
    amount: Uint128,
    info: &MessageInfo,
    pool_contract: &Addr,
    out_msgs: &mut Vec<CosmosMsg>,
) -> Result<Uint128, ContractError> {
    match asset_info {
        TokenType::Native { denom } => {
            let paid = info
                .funds
                .iter()
                .find(|c| c.denom == *denom)
                .map(|c| c.amount)
                .unwrap_or_default();
            if paid < amount {
                return Err(ContractError::InvalidNativeAmount {
                    expected: amount,
                    actual: paid,
                });
            }
            let refund = paid.checked_sub(amount).unwrap_or(Uint128::zero());
            if !refund.is_zero() {
                out_msgs.push(CosmosMsg::Bank(BankMsg::Send {
                    to_address: info.sender.to_string(),
                    amount: vec![Coin {
                        denom: denom.clone(),
                        amount: refund,
                    }],
                }));
            }
            Ok(refund)
        }
        TokenType::CreatorToken { contract_addr } => {
            if !amount.is_zero() {
                out_msgs.push(CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: contract_addr.to_string(),
                    msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                        owner: info.sender.to_string(),
                        recipient: pool_contract.to_string(),
                        amount,
                    })?,
                    funds: vec![],
                }));
            }
            Ok(Uint128::zero())
        }
    }
}

pub(crate) fn prepare_deposit(
    deps: Deps,
    info: &MessageInfo,
    amount0: Uint128,
    amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<DepositPrep, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;

    let (liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps, amount0, amount1)?;

    check_slippage(actual_amount0, min_amount0, "asset0")?;
    check_slippage(actual_amount1, min_amount1, "asset1")?;

    let mut collect_msgs: Vec<CosmosMsg> = Vec::new();
    let refund_amount0 = collect_deposit_side(
        &pool_info.pool_info.asset_infos[0],
        actual_amount0,
        info,
        &pool_info.pool_info.contract_addr,
        &mut collect_msgs,
    )?;
    let refund_amount1 = collect_deposit_side(
        &pool_info.pool_info.asset_infos[1],
        actual_amount1,
        info,
        &pool_info.pool_info.contract_addr,
        &mut collect_msgs,
    )?;

    Ok(DepositPrep {
        pool_info,
        liquidity,
        actual_amount0,
        actual_amount1,
        collect_msgs,
        refund_amount0,
        refund_amount1,
    })
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

    let prep = prepare_deposit(
        deps.as_ref(),
        &info,
        amount0,
        amount1,
        min_amount0,
        min_amount1,
    )?;

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // First-depositor detection. `total_liquidity == 0` AND both reserves
    // were zero immediately before this call → genuinely empty pool, this
    // is the inflation-attack-relevant first deposit. We lock
    // `MINIMUM_LIQUIDITY` LP units of the depositor's Position so they
    // can never withdraw the principal, while letting fees still accrue
    // against the full position (see `Position.locked_liquidity` doc).
    //
    // Creator pools after threshold crossing have non-zero seed reserves
    // and non-zero `total_liquidity`, so this branch is not taken there
    // — first post-threshold LPs deposit normally with no lock.
    let is_first_deposit = pool_state.total_liquidity.is_zero()
        && pool_state.reserve0.is_zero()
        && pool_state.reserve1.is_zero();
    let locked_liquidity = if is_first_deposit {
        MINIMUM_LIQUIDITY
    } else {
        Uint128::zero()
    };

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

    // prepare_deposit already dispatched per-asset and built the collection
    // messages (TransferFrom for CW20 sides, BankMsg refunds for over-paid
    // native sides). Splice them into the response list.
    messages.extend(prep.collect_msgs.clone());

    let mut pos_id = NEXT_POSITION_ID.load(deps.storage)?;
    pos_id = pos_id
        .checked_add(1)
        .ok_or_else(|| ContractError::Std(StdError::generic_err("Position ID overflow")))?;
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
        locked_liquidity,
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;
    OWNER_POSITIONS.save(deps.storage, (&user, &position_id), &true)?;

    pool_state.reserve0 = pool_state.reserve0.checked_add(prep.actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(prep.actual_amount1)?;
    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(prep.liquidity)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    let unpaused =
        pool_state.reserve0 >= MINIMUM_LIQUIDITY && pool_state.reserve1 >= MINIMUM_LIQUIDITY;
    if unpaused {
        POOL_PAUSED.save(deps.storage, &false)?;
    }

    // Update analytics
    let mut analytics = POOL_ANALYTICS.may_load(deps.storage)?.unwrap_or_default();
    analytics.total_lp_deposit_count += 1;
    POOL_ANALYTICS.save(deps.storage, &analytics)?;

    // Share of pool in basis points
    let share_of_pool_bps = if !pool_state.total_liquidity.is_zero() {
        prep.liquidity
            .checked_mul(Uint128::from(10000u128))
            .unwrap_or(Uint128::MAX)
            .checked_div(pool_state.total_liquidity)
            .unwrap_or(Uint128::zero())
            .to_string()
    } else {
        "10000".to_string() // 100% if first depositor
    };

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![
            ("action", "deposit_liquidity".to_string()),
            ("position_id", position_id),
            ("depositor", user.to_string()),
            ("liquidity", prep.liquidity.to_string()),
            ("actual_amount0", prep.actual_amount0.to_string()),
            ("actual_amount1", prep.actual_amount1.to_string()),
            ("refunded_amount0", prep.refund_amount0.to_string()),
            ("refunded_amount1", prep.refund_amount1.to_string()),
            ("offered_amount0", amount0.to_string()),
            ("offered_amount1", amount1.to_string()),
            ("reserve0_after", pool_state.reserve0.to_string()),
            ("reserve1_after", pool_state.reserve1.to_string()),
            ("total_liquidity_after", pool_state.total_liquidity.to_string()),
            ("share_of_pool_bps", share_of_pool_bps),
            ("pool_contract", pool_state.pool_contract_address.to_string()),
            ("block_height", env.block.height.to_string()),
            ("block_time", env.block.time.seconds().to_string()),
            ("total_lp_deposit_count", analytics.total_lp_deposit_count.to_string()),
            ("pool_unpaused", if unpaused { "true".to_string() } else { "false".to_string() }),
        ]))
}
