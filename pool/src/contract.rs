#![allow(non_snake_case)]
use crate::asset::{PoolPairType, TokenInfo, TokenType};
use crate::error::ContractError;
use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, get_bank_transfer_to_msg,
    trigger_threshold_payout, update_pool_fee_growth, validate_factory_address,
    validate_pool_threshold_payments,
};
use crate::liquidity::{
    execute_add_to_position, execute_collect_fees, execute_deposit_liquidity,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent,
};
use crate::msg::{Cw20HookMsg, ExecuteMsg, PoolInstantiateMsg};
use crate::query::query_check_commit;
use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitLimitInfo, ExpectedFactory, OracleInfo, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs,
    ThresholdPayoutAmounts, COMMITFEEINFO, COMMITSTATUS, COMMIT_LEDGER, COMMIT_LIMIT_INFO,
    EXPECTED_FACTORY, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, ORACLE_INFO, POOL_FEE_STATE,
    POOL_INFO, POOL_SPECS, POOL_STATE, RATE_LIMIT_GUARD, THRESHOLD_PAYOUT_AMOUNTS,
    THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::state::{
    Commiting, PoolState, Position, COMMIT_INFO, LIQUIDITY_POSITIONS, NEXT_POSITION_ID,
};
use crate::swap_helper::{
    assert_max_spread, compute_swap, get_bluechip_value, get_usd_value, update_price_accumulator,
};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, Binary, CosmosMsg, Decimal, DepsMut, Env,
    Fraction, MessageInfo, Reply, Response, StdError, StdResult, SubMsgResult, Timestamp, Uint128,
    WasmMsg,
};
use cw2::set_contract_version;
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use protobuf::Message;
use std::vec;
// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";
// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;
// Contract name that is used for migration.
const CONTRACT_NAME: &str = "betfi-pair";
// Contract version that is used for migration.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    //ensure the correct factory contract address was used in creating the pool
    let cfg = ExpectedFactory {
        expected_factory_address: msg.used_factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;
    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;
    validate_factory_address(&real_factory.expected_factory_address, &msg.used_factory_addr)?;
    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }
    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;

    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
    }

    if (msg.commit_fee_info.commit_fee_bluechip + msg.commit_fee_info.commit_fee_creator)
        > Decimal::one()
    {
        return Err(ContractError::InvalidFee {});
    }
    let threshold_payouts = if let Some(params_binary) = msg.threshold_payout {
        let params: ThresholdPayoutAmounts = from_json(&params_binary)?;
        //make sure params match - no funny business with token minting.
        //checks total value and predetermined amounts for creator, BlueChip, original subscribers (commit amount), and the pool itself
        validate_pool_threshold_payments(&params)?;
        params
    } else {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Your params could not be validated during pool instantiation."),
        });
    };
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pool_info: PoolDetails {
            contract_addr: env.contract.address.clone(),
            asset_infos: msg.pool_token_info.clone(),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: msg.used_factory_addr.clone(),
        token_address: msg.token_address.clone(),
        position_nft_address: msg.position_nft_address.clone(),
    };

    let liquidity_position = Position {
        liquidity: Uint128::zero(),
        owner: Addr::unchecked(""),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier: Decimal::one(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayoutAmounts {
        creator_reward_amount: threshold_payouts.creator_reward_amount,
        bluechip_reward_amount: threshold_payouts.bluechip_reward_amount,
        pool_seed_amount: threshold_payouts.pool_seed_amount,
        commit_return_amount: threshold_payouts.commit_return_amount,
    };

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: msg.commit_threshold_limit_usd,
        commit_amount_for_threshold: msg.commit_amount_for_threshold,
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.used_factory_addr.clone(),
    };

    let pool_state = PoolState {
        pool_contract_address: env.contract.address.clone(),
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(), // bluechip token
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        // Initially false, set to true after NFT ownership is verified
        nft_ownership_accepted: false,
    };

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };

    USD_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    COMMITFEEINFO.save(deps.storage, &msg.commit_fee_info)?;
    COMMITSTATUS.save(deps.storage, &Uint128::zero())?;
    NATIVE_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    IS_THRESHOLD_HIT.save(deps.storage, &false)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT_AMOUNTS.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_LIMIT_INFO.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    // Create the LP token contract
    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool", env.contract.address.to_string()))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::UpdateConfig { .. } => Err(ContractError::NonSupported {}),
        //special swap funcntion that behaves differently before and after a threshold - contributed to commit ledger prior to crossing the threshold - acts a swap post threshold
        ExecuteMsg::Commit {
            asset,
            amount,
            transaction_deadline,
            belief_price,
            max_spread,
        } => commit(
            deps,
            env,
            info,
            asset,
            amount,
            transaction_deadline,
            belief_price,
            max_spread,
        ),
        //a standard swap - this can only be called IF the asset is the bluechip. if not, performing a swap will require executing the CW20 contract
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        } => {
            // only allow swap once commit_limit_usd has been reached
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            offer_asset.confirm_sent_bluechip_token_balance(&info)?;
            let sender_addr = info.sender.clone();
            let to_addr: Option<Addr> = to
                .map(|to_str| deps.api.addr_validate(&to_str))
                .transpose()?;
            // call the shared AMM logic
            simple_swap(
                deps,
                env,
                info,
                sender_addr,
                offer_asset,
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        ExecuteMsg::Receive(cw20_msg) => execute_swap_cw20(deps, env, info, cw20_msg),
        //deposit liquidity into a pool - accumulates liquiidty units for fees - mints an NFT for position
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            execute_deposit_liquidity(
                deps,
                env,
                info,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        //add to a currently held position by the user
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            // check threshold requirement
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            execute_add_to_position(
                deps,
                env,
                info,
                position_id,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        //collect all fees for a position
        ExecuteMsg::CollectFees { position_id } => {
            execute_collect_fees(deps, env, info, position_id)
        }
        //removes liquidity based on a specific amount (I have 100 liquidity I want to remove 18.) - will collect fees in proportion of removal to rebalance accounting
        ExecuteMsg::RemovePartialLiquidity {
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
        } => execute_remove_partial_liquidity(
            deps,
            env,
            info,
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
        ),
        //removes all liquidity for a position - (i have 100 liquidity and I remove 100) - collects all fees.
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount1,
            min_amount0,
        } => execute_remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
        ),
        //removes liquidity based on a specific percent (I have 100 liquidity I want to remove 18% = remove 18.) - will collect fees in proportion of removal to rebalance accounting
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
        } => execute_remove_partial_liquidity_by_percent(
            deps,
            env,
            info,
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
        ),
    }
}

pub fn execute_swap_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    let contract_addr = info.sender.clone();
    match from_json(&cw20_msg.msg) {
        Ok(Cw20HookMsg::Swap {
            belief_price,
            max_spread,
            to,
            transaction_deadline,
        }) => {
            // Only asset contract can execute this message
            let mut authorized: bool = false;
            let pool_info: PoolInfo = POOL_INFO.load(deps.storage)?;

            for pool in pool_info.pool_info.asset_infos {
                if let TokenType::CreatorToken { contract_addr, .. } = &pool {
                    if contract_addr == &info.sender {
                        authorized = true;
                    }
                }
            }
            if !authorized {
                return Err(ContractError::Unauthorized {});
            }
            let to_addr = if let Some(to_addr) = to {
                Some(deps.api.addr_validate(to_addr.as_str())?)
            } else {
                None
            };
            simple_swap(
                deps,
                env,
                info,
                Addr::unchecked(cw20_msg.sender),
                TokenInfo {
                    info: TokenType::CreatorToken { contract_addr },
                    amount: cw20_msg.amount,
                },
                belief_price,
                max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        Ok(Cw20HookMsg::DepositLiquidity {
            amount0,
            min_amount0,
            min_amount1,
            transaction_deadline,
        }) => execute_deposit_liquidity(
            deps,
            env,
            info,
            Addr::unchecked(cw20_msg.sender), //mainly focusing on this
            amount0,
            cw20_msg.amount,
            min_amount0,
            min_amount1,
            transaction_deadline,
        ),
        Ok(Cw20HookMsg::AddToPosition {
            position_id,
            amount0,
            min_amount0,
            min_amount1,
            transaction_deadline,
        }) => execute_add_to_position(
            deps,
            env,
            info,
            position_id,
            Addr::unchecked(cw20_msg.sender),
            amount0,
            cw20_msg.amount,
            min_amount0,
            min_amount1,
            transaction_deadline,
        ),
        Err(err) => Err(ContractError::Std(err)),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    if msg.id == 42 {
        let res = match msg.result {
            SubMsgResult::Ok(res) => res,
            SubMsgResult::Err(err) => {
                return Err(StdError::generic_err(format!("submsg error: {err}")).into())
            }
        };
        let data: Binary = res
            .data
            .ok_or_else(|| StdError::not_found("instantiate data"))?;
        let parsed: MsgInstantiateContractResponse = Message::parse_from_bytes(data.as_slice())
            .map_err(|_| {
                StdError::parse_err(
                    "MsgInstantiateContractResponse",
                    "invalid instantiate reply data",
                )
            })?;
        let lp: Addr = deps.api.addr_validate(parsed.get_contract_address())?;

        POOL_INFO.update(deps.storage, |pool_info| -> Result<_, ContractError> {
            Ok(pool_info)
        })?;

        return Ok(Response::new().add_attribute("lp_token", lp));
    }

    Err(StdError::generic_err("unknown reply id").into())
}

pub fn simple_swap(
    mut deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
    transaction_deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = sender.clone();

    // Rate limiting check
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = execute_simple_swap(
        &mut deps,
        env,
        _info,
        sender,
        offer_asset,
        belief_price,
        max_spread,
        to,
    );
    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

#[allow(clippy::too_many_arguments)]
//logic to carry out the simple swap transacation
pub fn execute_simple_swap(
    deps: &mut DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: TokenInfo,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let (offer_pool_contract_addressx, offer_pool, ask_pool) =
        if offer_asset.info.equal(&pool_info.pool_info.asset_infos[0]) {
            (0, pool_state.reserve0, pool_state.reserve1)
        } else if offer_asset.info.equal(&pool_info.pool_info.asset_infos[1]) {
            (1, pool_state.reserve1, pool_state.reserve0)
        } else {
            return Err(ContractError::AssetMismatch {});
        };
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, offer_asset.amount, pool_specs.lp_fee)?;
    assert_max_spread(
        belief_price,
        max_spread,
        offer_asset.amount,
        return_amt + commission_amt,
        spread_amt,
    )?;
    let offer_pool_post = offer_pool.checked_add(offer_asset.amount)?;
    let ask_pool_post = ask_pool.checked_sub(return_amt)?;

    if offer_pool_contract_addressx == 0 {
        pool_state.reserve0 = offer_pool_post;
        pool_state.reserve1 = ask_pool_post;
    } else {
        pool_state.reserve0 = ask_pool_post;
        pool_state.reserve1 = offer_pool_post;
    }
    // Update fee growth
    update_pool_fee_growth(
        &mut pool_fee_state,
        &pool_state,
        offer_pool_contract_addressx,
        commission_amt,
    )?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    // Update pool prices
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

    // Save updated state
    POOL_STATE.save(deps.storage, &pool_state)?;
    let ask_asset_info = if offer_pool_contract_addressx == 0 {
        pool_info.pool_info.asset_infos[1].clone()
    } else {
        pool_info.pool_info.asset_infos[0].clone()
    };

    let msgs = if !return_amt.is_zero() {
        vec![TokenInfo {
            info: ask_asset_info.clone(),
            amount: return_amt,
        }
        .into_msg(&deps.querier, to.unwrap_or(sender.clone()))?]
    } else {
        vec![]
    };
    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "swap")
        .add_attribute("sender", sender)
        .add_attribute("offer_asset", offer_asset.info.to_string())
        .add_attribute("ask_asset", ask_asset_info.to_string())
        .add_attribute("offer_amount", offer_asset.amount.to_string())
        .add_attribute("return_amount", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount", commission_amt.to_string())
        .add_attribute(
            "belief_price",
            belief_price
                .map(|p| p.to_string())
                .unwrap_or("none".to_string()),
        )
        .add_attribute(
            "max_spread",
            max_spread
                .map(|s| s.to_string())
                .unwrap_or("none".to_string()),
        ))
}

#[allow(clippy::too_many_arguments)]
pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    amount: Uint128,
    transaction_deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    enforce_transaction_deadline(env.block.time, transaction_deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = execute_commit_logic(
        &mut deps,
        env,
        info,
        asset,
        amount,
        belief_price,
        max_spread,
    );
    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

pub fn execute_commit_logic(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    asset: TokenInfo,
    amount: Uint128,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let threshold_payout = THRESHOLD_PAYOUT_AMOUNTS.load(deps.storage)?;
    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let sender = info.sender.clone();
    // Validate asset type
    if !asset.info.equal(&pool_info.pool_info.asset_infos[0])
        && !asset.info.equal(&pool_info.pool_info.asset_infos[1])
    {
        return Err(ContractError::AssetMismatch {});
    }
    // Validate amount matches
    if amount != asset.amount {
        return Err(ContractError::MismatchAmount {});
    }
    if asset.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }
    let usd_value = get_usd_value(deps.as_ref(), asset.amount)?;
    match &asset.info {
        TokenType::Bluechip { denom } if denom == "stake" => {
            // Verify funds were actually sent
            let sent = info
                .funds
                .iter()
                .find(|c| c.denom == denom.as_str())
                .map(|c| c.amount)
                .unwrap_or_default();
            if sent < amount {
                return Err(ContractError::MismatchAmount {});
            }
            let mut messages: Vec<CosmosMsg> = Vec::new();
            // fees calculated right away
            let commit_fee_bluechip_amt = amount * fee_info.commit_fee_bluechip.numerator()
                / fee_info.commit_fee_bluechip.denominator();
            let commit_fee_creator_amt = amount * fee_info.commit_fee_creator.numerator()
                / fee_info.commit_fee_creator.denominator();
            // Create fee transfer messages
            let bluechip_transfer = get_bank_transfer_to_msg(
                &fee_info.bluechip_wallet_address,
                &denom,
                commit_fee_bluechip_amt,
            )
            .map_err(|e| {
                ContractError::Std(StdError::generic_err(format!(
                    "Bluechip transfer failed: {}",
                    e
                )))
            })?;
            let creator_transfer =
                get_bank_transfer_to_msg(&fee_info.creator_wallet_address, &denom, commit_fee_creator_amt)
                    .map_err(|e| {
                        ContractError::Std(StdError::generic_err(format!(
                            "Creator transfer failed: {}",
                            e
                        )))
                    })?;
            messages.push(bluechip_transfer);
            messages.push(creator_transfer);
            // load state of threshold of the pool
            let threshold_already_hit = IS_THRESHOLD_HIT.load(deps.storage)?;
            //pre threshold logic begins....
            if !threshold_already_hit {
                let current_usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
                let new_total = current_usd_raised + usd_value;
                // Check if this commit will cross or exceed the threshold
                if new_total >= commit_config.commit_amount_for_threshold_usd {
                    // Try to acquire the threshold processing lock to trigger crossing
                    let processing = THRESHOLD_PROCESSING
                        .may_load(deps.storage)?
                        .unwrap_or(false);
                    let can_process = if processing {
                        false // a different transaction is processing
                    } else {
                        THRESHOLD_PROCESSING.save(deps.storage, &true)?;
                        true // commiters transaction get to be the threshold crosser WHICH wins them nothing.
                    };
                    //logic if a different transaction indeed triggered the threhsold crossing.
                    if !can_process {
                        //double check to handle race conditions
                        if IS_THRESHOLD_HIT.load(deps.storage)? {
                            // Threshold was hit, process as post-threshold
                            return process_post_threshold_commit(
                                deps,
                                env,
                                sender,
                                asset,
                                usd_value,
                                messages,
                                belief_price,
                                max_spread,
                            );
                        }

                        // Still pre-threshold, process normally
                        return process_pre_threshold_commit(
                            deps, env, sender, &asset, usd_value, messages,
                        );
                    }
                    // Calculate exact amounts for threshold crossing
                    let usd_to_threshold = commit_config
                        .commit_amount_for_threshold_usd
                        .checked_sub(current_usd_raised)
                        .unwrap_or(Uint128::zero());
                    //if transaction crosess threshold and still has leftover funds (commit amount = $24,900 next commit = $500)
                    if usd_value > usd_to_threshold && usd_to_threshold > Uint128::zero() {
                        // Calculate the bluechip amount that corresponds to reaching exactly $25k
                        let bluechip_to_threshold =
                            get_bluechip_value(deps.as_ref(), usd_to_threshold)?;
                        let bluechip_excess = asset.amount.checked_sub(bluechip_to_threshold)?;
                        let usd_excess = usd_value.checked_sub(usd_to_threshold)?;
                        // Update commit ledger with only the threshold portion
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default() + usd_to_threshold)
                        })?;
                        // Set USD raised to exactly the threshold
                        USD_RAISED_FROM_COMMIT
                            .save(deps.storage, &commit_config.commit_amount_for_threshold_usd)?;
                        //mark threshold as hit
                        IS_THRESHOLD_HIT.save(deps.storage, &true)?;
                        // Trigger threshold payouts
                        messages.extend(trigger_threshold_payout(
                            deps.storage,
                            &pool_info,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            &env,
                        )?);
                        COMMIT_INFO.update(
                            deps.storage,
                            &sender,
                            |maybe_commiting| -> Result<_, ContractError> {
                                match maybe_commiting {
                                    Some(mut commit) => {
                                        commit.total_paid_bluechip += bluechip_to_threshold;
                                        commit.total_paid_usd += usd_to_threshold;
                                        commit.last_payment_bluechip = bluechip_to_threshold;
                                        commit.last_payment_usd = usd_to_threshold;
                                        commit.last_commited = env.block.time;
                                        Ok(commit)
                                    }
                                    None => Ok(Commiting {
                                        pool_contract_address: pool_state.pool_contract_address,
                                        commiter: sender.clone(),
                                        total_paid_bluechip: bluechip_to_threshold,
                                        total_paid_usd: usd_to_threshold,
                                        last_commited: env.block.time,
                                        last_payment_bluechip: bluechip_to_threshold,
                                        last_payment_usd: usd_to_threshold,
                                    }),
                                }
                            },
                        )?;
                        // Process the excess as a swap
                        let mut return_amt = Uint128::zero();
                        let mut spread_amt = Uint128::zero();
                        let mut commission_amt = Uint128::zero();
                        if bluechip_excess > Uint128::zero() {
                            // Load updated pool state (modified by threshold payout)
                            let mut pool_state = POOL_STATE.load(deps.storage)?;
                            let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
                            // Perform swap with excess amount
                            let offer_pool = pool_state.reserve0;
                            let ask_pool = pool_state.reserve1;
                            //make sure both assets have been set in the pool properly
                            if !ask_pool.is_zero() && !offer_pool.is_zero() {
                                let (ret_amt, sp_amt, comm_amt) = compute_swap(
                                    offer_pool,
                                    ask_pool,
                                    bluechip_excess,
                                    pool_specs.lp_fee,
                                )?;
                                return_amt = ret_amt;
                                spread_amt = sp_amt;
                                commission_amt = comm_amt;
                            }
                            // Check slippage if specified
                            if let Some(max_spread) = max_spread {
                                assert_max_spread(
                                    belief_price,
                                    Some(max_spread),
                                    bluechip_excess,
                                    return_amt,
                                    spread_amt,
                                )?;
                            }
                            // Update reserves
                            pool_state.reserve0 = offer_pool.checked_add(bluechip_excess)?;
                            pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;

                            // Update fee growth
                            update_pool_fee_growth(
                                &mut pool_fee_state,
                                &pool_state,
                                0,
                                commission_amt,
                            )?;
                            // Save states
                            POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
                            update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
                            POOL_STATE.save(deps.storage, &pool_state)?;
                            // Send CW20 tokens from swap
                            if !return_amt.is_zero() {
                                messages.push(
                                    WasmMsg::Execute {
                                        contract_addr: pool_info.token_address.to_string(),
                                        msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                                            recipient: sender.to_string(),
                                            amount: return_amt,
                                        })?,
                                        funds: vec![],
                                    }
                                    .into(),
                                );
                            }
                            //save the commit transaction
                            COMMIT_INFO.update(
                                deps.storage,
                                &sender,
                                |maybe_commiting| -> Result<_, ContractError> {
                                    if let Some(mut commiting) = maybe_commiting {
                                        commiting.total_paid_bluechip += bluechip_excess;
                                        commiting.total_paid_usd += usd_excess;
                                        Ok(commiting)
                                    } else {
                                        unreachable!("Commit was created above")
                                    }
                                },
                            )?;
                        }
                        // Clear the processing lock
                        THRESHOLD_PROCESSING.save(deps.storage, &false)?;
                        // Return response for split commit
                        return Ok(Response::new()
                            .add_messages(messages)
                            .add_attribute("action", "commit")
                            .add_attribute("phase", "threshold_crossing")
                            .add_attribute("committer", sender)
                            .add_attribute("total_amount_bluechip", asset.amount.to_string())
                            .add_attribute(
                                "threshold_amount_bluechip",
                                bluechip_to_threshold.to_string(),
                            )
                            .add_attribute("swap_amount_bluechip", bluechip_excess.to_string())
                            .add_attribute("threshold_amount_usd", usd_to_threshold.to_string())
                            .add_attribute("swap_amount_usd", usd_excess.to_string())
                            .add_attribute("bluechip_excess_spread", spread_amt.to_string())
                            .add_attribute("bluechip_excess_returned", return_amt.to_string())
                            .add_attribute(
                                "bluechip_excess_commission",
                                commission_amt.to_string(),
                            ));
                    } else {
                        //threshold hit on the nose.
                        // Update commit ledger
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default() + usd_value)
                        })?;
                        // Update total USD raised
                        let final_usd = if new_total > commit_config.commit_amount_for_threshold_usd
                        {
                            commit_config.commit_amount_for_threshold_usd
                        } else {
                            new_total
                        };
                        USD_RAISED_FROM_COMMIT.save(deps.storage, &final_usd)?;
                        // Mark threshold as hit
                        IS_THRESHOLD_HIT.save(deps.storage, &true)?;
                        // Trigger threshold payouts
                        messages.extend(trigger_threshold_payout(
                            deps.storage,
                            &pool_info,
                            &mut pool_state,
                            &mut pool_fee_state,
                            &commit_config,
                            &threshold_payout,
                            &fee_info,
                            &env,
                        )?);
                        COMMIT_INFO.update(
                            deps.storage,
                            &sender,
                            |maybe_commiting| -> Result<_, ContractError> {
                                match maybe_commiting {
                                    Some(mut commiting) => {
                                        commiting.total_paid_bluechip += asset.amount;
                                        commiting.total_paid_usd += usd_value;
                                        commiting.last_payment_bluechip = asset.amount;
                                        commiting.last_payment_usd = usd_value;
                                        commiting.last_commited = env.block.time;
                                        Ok(commiting)
                                    }
                                    None => Ok(Commiting {
                                        pool_contract_address: pool_state.pool_contract_address,
                                        commiter: sender.clone(),
                                        total_paid_bluechip: asset.amount,
                                        total_paid_usd: usd_value,
                                        last_commited: env.block.time,
                                        last_payment_bluechip: asset.amount,
                                        last_payment_usd: usd_value,
                                    }),
                                }
                            },
                        )?;
                        // Clear the processing lock
                        THRESHOLD_PROCESSING.save(deps.storage, &false)?;
                        return Ok(Response::new()
                            .add_messages(messages)
                            .add_attribute("action", "commit")
                            .add_attribute("phase", "threshold_hit_exact")
                            .add_attribute("committer", sender)
                            .add_attribute("commit_amount_bluechip", asset.amount.to_string())
                            .add_attribute("commit_amount_usd", usd_value.to_string()));
                    }
                } else {
                    // normal commit pre threshold (doesn't reach threshold)
                    return process_pre_threshold_commit(
                        deps, env, sender, &asset, usd_value, messages,
                    );
                }
            } else {
                // post threshold commit swap logic using bluechip token
                return process_post_threshold_commit(
                    deps,
                    env,
                    sender,
                    asset,
                    usd_value,
                    messages,
                    belief_price,
                    max_spread,
                );
            }
        }
        _ => Err(ContractError::AssetMismatch {}),
    }
}

//commit transaction priot to threhold being crossed. commit to ledger and store values for return mint
fn process_pre_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &TokenInfo,
    usd_value: Uint128,
    messages: Vec<CosmosMsg>,
) -> Result<Response, ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    //do not calculate fees in function, they are calculated prior.
    // Update commit ledger
    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default() + usd_value)
    })?;
    // Update total USD raised
    let usd_total =
        USD_RAISED_FROM_COMMIT.update::<_, ContractError>(deps.storage, |r| Ok(r + usd_value))?;
    COMMITSTATUS.save(deps.storage, &usd_total)?;

    COMMIT_INFO.update(
        deps.storage,
        &sender,
        |maybe_commiting| -> Result<_, ContractError> {
            match maybe_commiting {
                Some(mut commiting) => {
                    commiting.total_paid_bluechip += asset.amount;
                    commiting.total_paid_usd += usd_value;
                    commiting.last_payment_bluechip = asset.amount;
                    commiting.last_payment_usd = usd_value;
                    commiting.last_commited = env.block.time;
                    Ok(commiting)
                }
                None => Ok(Commiting {
                    pool_contract_address: pool_state.pool_contract_address,
                    commiter: sender.clone(),
                    total_paid_bluechip: asset.amount,
                    total_paid_usd: usd_value,
                    last_commited: env.block.time,
                    last_payment_bluechip: asset.amount,
                    last_payment_usd: usd_value,
                }),
            }
        },
    )?;
    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "funding")
        .add_attribute("committer", sender)
        .add_attribute("block_committed", env.block.time.to_string())
        .add_attribute("commit_amount_bluechip", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string()))
}

//commit transaction post threshold - makes a swap with pool - still has fees taken out for creator and bluechip done in execute_commit_logic
fn process_post_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: TokenInfo,
    usd_value: Uint128,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    //do not calculate fees in function, they are calculated prior.
    // Load current pool balances
    let offer_pool = pool_state.reserve0;
    let ask_pool = pool_state.reserve1;
    // Calculate swap output
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, asset.amount, pool_specs.lp_fee)?;
    // Check slippage
    assert_max_spread(
        belief_price,
        max_spread,
        asset.amount,
        return_amt,
        spread_amt,
    )?;
    // Update reserves
    pool_state.reserve0 = offer_pool.checked_add(asset.amount)?;
    pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;
    // Update fee growth
    update_pool_fee_growth(&mut pool_fee_state, &pool_state, 0, commission_amt)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    // Update price accumulator
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    // Send CW20 tokens to user
    if !return_amt.is_zero() {
        messages.push(
            WasmMsg::Execute {
                contract_addr: pool_info.token_address.to_string(),
                msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                    recipient: sender.to_string(),
                    amount: return_amt,
                })?,
                funds: vec![],
            }
            .into(),
        );
    }
    COMMIT_INFO.update(
        deps.storage,
        &sender,
        |maybe_commiting| -> Result<_, ContractError> {
            match maybe_commiting {
                Some(mut commiting) => {
                    commiting.total_paid_bluechip += asset.amount;
                    commiting.total_paid_usd += usd_value;
                    commiting.last_payment_bluechip = asset.amount;
                    commiting.last_payment_usd = usd_value;
                    commiting.last_commited = env.block.time;
                    Ok(commiting)
                }
                None => Ok(Commiting {
                    pool_contract_address: pool_state.pool_contract_address,
                    commiter: sender.clone(),
                    total_paid_bluechip: asset.amount,
                    total_paid_usd: usd_value,
                    last_commited: env.block.time,
                    last_payment_bluechip: asset.amount,
                    last_payment_usd: usd_value,
                }),
            }
        },
    )?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "active")
        .add_attribute("committer", sender)
        .add_attribute("commit_amount_bluechip", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string())
        .add_attribute("block_committed", env.block.time.to_string())
        .add_attribute("tokens_received", return_amt.to_string()))
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
