#![allow(non_snake_case)]
use crate::asset::{call_pool_info, Asset, AssetInfo, PairType};
use crate::error::ContractError;
use crate::msg::{
    CommitStatus, CommiterInfo, ConfigResponse, CumulativePricesResponse, Cw20HookMsg, ExecuteMsg,
    FeeInfo, FeeInfoResponse, LastCommitedResponse, MigrateMsg, PoolCommitResponse,
    PoolFeeStateResponse, PoolInfoResponse, PoolInstantiateMsg, PoolResponse, PoolStateResponse,
    PositionResponse, PositionsResponse, QueryMsg, ReverseSimulationResponse, SimulationResponse,
};
use crate::oracle::{OracleData, PriceResponse, PythQueryMsg};
use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitInfo,
    ExpectedFactory,
    OracleInfo,
    PairInfo,
    PoolFeeState,
    PoolInfo,
    PoolSpecs,
    ThresholdPayout,
    COMMITSTATUS,
    COMMIT_CONFIG,
    COMMIT_LEDGER,
    EXPECTED_FACTORY,
    FEEINFO,
    MAX_ORACLE_AGE,
    NATIVE_RAISED,
    ORACLE_INFO,
    POOL_FEE_STATE,
    POOL_INFO,
    POOL_SPECS,
    POOL_STATE,
    RATE_LIMIT_GUARD,
    THRESHOLD_HIT,
    THRESHOLD_PAYOUT,
    THRESHOLD_PROCESSING,
    USD_RAISED,
    USER_LAST_COMMIT, //ACCUMULATED_BLUECHIP_FEES, ACCUMULATED_CREATOR_FEES,
};
use crate::state::{
    Commiting, PoolState, Position, TokenMetadata, COMMIT_INFO, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID,
};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, BankMsg, Binary, Coin, CosmosMsg, Decimal,
    Decimal256, Deps, DepsMut, Empty, Env, Fraction, MessageInfo, Order, QuerierWrapper, Reply,
    Response, StdError, StdResult, Storage, SubMsgResult, Timestamp, Uint128, Uint256, WasmMsg,
};
use cw2::{get_contract_version, set_contract_version};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use cw721_base::ExecuteMsg as CW721BaseExecuteMsg;
use cw_storage_plus::Bound;
use protobuf::Message;
use std::convert::TryInto;
use std::str::FromStr;
use std::vec;
/// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
/// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";

// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;

/// Contract name that is used for migration.
const CONTRACT_NAME: &str = "betfi-pair";
/// Contract version that is used for migration.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    let cfg = ExpectedFactory {
        expected_factory_address: msg.factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;

    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;

    // Validate factory address stored matches message factory_addr (optional)
    validate_factory_address(&real_factory.expected_factory_address, &msg.factory_addr)?;

    // **Here is the critical check: does the caller equal the stored factory?**
    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }
    msg.asset_infos[0].check(deps.api)?;
    msg.asset_infos[1].check(deps.api)?;

    if msg.asset_infos[0] == msg.asset_infos[1] {
        return Err(ContractError::DoublingAssets {});
    }

    if (msg.fee_info.bluechip_fee + msg.fee_info.creator_fee) > Decimal::one() {
        return Err(ContractError::InvalidFee {});
    }
    let threshold_payouts = if let Some(params_binary) = msg.threshold_payout {
        let params: ThresholdPayout = from_json(&params_binary)?;
        // CRITICAL: Validate the params match expected values
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
        pair_info: PairInfo {
            contract_addr: env.contract.address.clone(),
            liquidity_token: Addr::unchecked(""), // Set later
            asset_infos: msg.asset_infos.clone(),
            pair_type: PairType::Xyk {},
        },
        factory_addr: msg.factory_addr.clone(),
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
        fee_multiplier: Decimal::one(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayout {
        creator_amount: threshold_payouts.creator_amount,
        bluechip_amount: threshold_payouts.bluechip_amount,
        pool_amount: threshold_payouts.pool_amount,
        commit_amount: threshold_payouts.commit_amount,
    };

    let commit_config = CommitInfo {
        commit_limit_usd: msg.commit_limit_usd,
        commit_amount_for_threshold: msg.commit_amount_for_threshold, //variable set here
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.oracle_addr.clone(),
        oracle_symbol: msg.oracle_symbol.clone(),
    };

    let pool_state = PoolState {
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(), // native token
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        nft_ownership_accepted: false, // Initially false, set to true after NFT ownership is verified
    };

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };

    USD_RAISED.save(deps.storage, &Uint128::zero())?;
    FEEINFO.save(deps.storage, &msg.fee_info)?;
    COMMITSTATUS.save(deps.storage, &Uint128::zero())?;
    NATIVE_RAISED.save(deps.storage, &Uint128::zero())?;
    THRESHOLD_HIT.save(deps.storage, &false)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_CONFIG.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    // Create the LP token contract
    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool", env.contract.address.to_string()))
}
fn validate_pool_threshold_payments(params: &ThresholdPayout) -> Result<(), ContractError> {
    // Define the ONLY acceptable values
    const EXPECTED_CREATOR: u128 = 325_000_000_000;
    const EXPECTED_BLUECHIP: u128 = 25_000_000_000;
    const EXPECTED_POOL: u128 = 350_000_000_000;
    const EXPECTED_COMMIT: u128 = 500_000_000_000;
    const EXPECTED_TOTAL: u128 = 1_200_000_000_000;

    // Verify each amount exactly
    if params.creator_amount != Uint128::new(EXPECTED_CREATOR) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Creator amount must be {}", EXPECTED_CREATOR),
        });
    }

    if params.bluechip_amount != Uint128::new(EXPECTED_BLUECHIP) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("BlueChip amount must be {}", EXPECTED_BLUECHIP),
        });
    }

    if params.pool_amount != Uint128::new(EXPECTED_POOL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", EXPECTED_POOL),
        });
    }

    if params.commit_amount != Uint128::new(EXPECTED_COMMIT) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Commit amount must be {}", EXPECTED_COMMIT),
        });
    }

    // Verify total
    let total =
        params.creator_amount + params.bluechip_amount + params.pool_amount + params.commit_amount;

    if total != Uint128::new(EXPECTED_TOTAL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Total must equal {} (got {})", EXPECTED_TOTAL, total),
        });
    }

    Ok(())
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

        ExecuteMsg::Commit {
            asset,
            amount,
            deadline,
            belief_price,
            max_spread,
        } => commit(
            deps,
            env,
            info,
            asset,
            amount,
            deadline,
            belief_price,
            max_spread,
        ),
        // ── standard swap via native coin ──────────────────
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            to,
            deadline,
        } => {
            // only allow swap once commit_limit_usd has been reached
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            // ensure they sent the native coin
            offer_asset.assert_sent_native_token_balance(&info)?;
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
                deadline,
            )
        }
        ExecuteMsg::Receive(cw20_msg) => execute_swap_cw20(deps, env, info, cw20_msg),

        // ── NEW: NFT-based liquidity management ──────────────────
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            deadline,
        } => {
            // Check threshold requirement (same as swap)
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
                deadline,
            )
        }

        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            deadline,
        } => {
            // Check threshold requirement
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
                deadline,
            )
        }

        ExecuteMsg::CollectFees { position_id } => {
            execute_collect_fees(deps, env, info, position_id)
        }

        ExecuteMsg::RemovePartialLiquidity {
            position_id,
            liquidity_to_remove,
            deadline,
            min_amount0,
            min_amount1,
        } => execute_remove_partial_liquidity(
            deps,
            env,
            info,
            position_id,
            liquidity_to_remove,
            deadline,
            min_amount0,
            min_amount1,
        ),

        ExecuteMsg::RemoveLiquidity {
            position_id,
            deadline,
            min_amount1,
            min_amount0,
        } => execute_remove_liquidity(
            deps,
            env,
            info,
            position_id,
            deadline,
            min_amount0,
            min_amount1,
        ),
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            deadline,
            min_amount0,
            min_amount1,
        } => execute_remove_partial_liquidity_by_percent(
            deps,
            env,
            info,
            position_id,
            percentage,
            deadline,
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
            deadline,
        }) => {
            // Only asset contract can execute this message
            let mut authorized: bool = false;
            let pool_info: PoolInfo = POOL_INFO.load(deps.storage)?;

            for pool in pool_info.pair_info.asset_infos {
                if let AssetInfo::Token { contract_addr, .. } = &pool {
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
                Asset {
                    info: AssetInfo::Token { contract_addr },
                    amount: cw20_msg.amount,
                },
                belief_price,
                max_spread,
                to_addr,
                deadline,
            )
        }
        Ok(Cw20HookMsg::DepositLiquidity {
            amount0,
            min_amount0,
            min_amount1,
            deadline,
        }) => execute_deposit_liquidity(
            deps,
            env,
            info,
            Addr::unchecked(cw20_msg.sender), //mainly focusing on this
            amount0,
            cw20_msg.amount,
            min_amount0,
            min_amount1,
            deadline,
        ),
        Ok(Cw20HookMsg::AddToPosition {
            position_id,
            amount0,
            min_amount0,
            min_amount1,
            deadline,
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
            deadline,
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

        // CHANGED: Update POOL_INFO instead of CONFIG
        POOL_INFO.update(deps.storage, |mut pool_info| -> Result<_, ContractError> {
            pool_info.pair_info.liquidity_token = lp.clone();
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
    offer_asset: Asset,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
    deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
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
pub fn execute_simple_swap(
    deps: &mut DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    offer_asset: Asset,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;

    let (offer_pool_idx, offer_pool, ask_pool) =
        if offer_asset.info.equal(&pool_info.pair_info.asset_infos[0]) {
            (0, pool_state.reserve0, pool_state.reserve1)
        } else if offer_asset.info.equal(&pool_info.pair_info.asset_infos[1]) {
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

    if offer_pool_idx == 0 {
        pool_state.reserve0 = offer_pool_post;
        pool_state.reserve1 = ask_pool_post;
    } else {
        pool_state.reserve0 = ask_pool_post;
        pool_state.reserve1 = offer_pool_post;
    }

    // Update fee growth
    update_fee_growth(
        &mut pool_fee_state,
        &pool_state,
        offer_pool_idx,
        commission_amt,
    )?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    // Update pool prices
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

    // Save updated state
    POOL_STATE.save(deps.storage, &pool_state)?;

    let ask_asset_info = if offer_pool_idx == 0 {
        pool_info.pair_info.asset_infos[1].clone()
    } else {
        pool_info.pair_info.asset_infos[0].clone()
    };

    let msgs = if !return_amt.is_zero() {
        vec![Asset {
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

// Update fee growth based on which token was offered
fn update_fee_growth(
    pool_fee_state: &mut PoolFeeState,
    pool_state: &PoolState,
    offer_pool_idx: usize,
    commission_amt: Uint128,
) -> Result<(), ContractError> {
    if pool_state.total_liquidity.is_zero() || commission_amt.is_zero() {
        return Ok(());
    }

    let fee_growth = Decimal::from_ratio(commission_amt, pool_state.total_liquidity);

    if offer_pool_idx == 0 {
        // Token0 offered, fees collected in token0
        pool_fee_state.fee_growth_global_0 += fee_growth;
        pool_fee_state.total_fees_collected_0 += commission_amt;
    } else {
        // Token1 offered, fees collected in token1
        pool_fee_state.fee_growth_global_1 += fee_growth;
        pool_fee_state.total_fees_collected_1 += commission_amt;
    }

    Ok(())
}

// Update price accumulator with time-weighted average
fn update_price_accumulator(
    pool_state: &mut PoolState,
    current_time: u64,
) -> Result<(), ContractError> {
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);

    if time_elapsed > 0 && !pool_state.reserve0.is_zero() && !pool_state.reserve1.is_zero() {
        // Calculate price * time_elapsed directly
        let price0_increment = pool_state
            .reserve1
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve0)
            .map_err(|_| ContractError::DivideByZero)?;

        let price1_increment = pool_state
            .reserve0
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve1)
            .map_err(|_| ContractError::DivideByZero)?;

        pool_state.price0_cumulative_last = pool_state
            .price0_cumulative_last
            .checked_add(price0_increment)?;
        pool_state.price1_cumulative_last = pool_state
            .price1_cumulative_last
            .checked_add(price1_increment)?;
        pool_state.block_time_last = current_time;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]

pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: Asset,
    amount: Uint128,
    deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
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
    asset: Asset,
    amount: Uint128,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let commit_config = COMMIT_CONFIG.load(deps.storage)?;
    let oracle_info = ORACLE_INFO.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let threshold_payout = THRESHOLD_PAYOUT.load(deps.storage)?;
    let fee_info = FEEINFO.load(deps.storage)?;
    let sender = info.sender.clone();

    // Validate asset type
    if !asset.info.equal(&pool_info.pair_info.asset_infos[0])
        && !asset.info.equal(&pool_info.pair_info.asset_infos[1])
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

    let oracle_data = get_and_validate_oracle_price(
        &deps.querier,
        &oracle_info.oracle_addr,
        &oracle_info.oracle_symbol,
        env.block.time.seconds(),
    )?;
    //check post threshold commits only against twaps
    if THRESHOLD_HIT.load(deps.storage)? {
        validate_oracle_price_against_twap(
            &deps.as_ref(),
            oracle_data.price,
            env.block.time.seconds(),
        )?;
    }
    let usd_value = native_to_usd(oracle_data.price, asset.amount, oracle_data.expo)?;

    match &asset.info {
        AssetInfo::NativeToken { denom } if denom == "stake" => {
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

            // Initialize messages vector for transfers
            let mut messages: Vec<CosmosMsg> = Vec::new();

            // Calculate fees
            let bluechip_fee_amt =
                amount * fee_info.bluechip_fee.numerator() / fee_info.bluechip_fee.denominator();
            let creator_fee_amt =
                amount * fee_info.creator_fee.numerator() / fee_info.creator_fee.denominator();

            // Verify contract has enough balance for fees
            let contract_balance = deps
                .querier
                .query_balance(env.contract.address.clone(), denom.clone())?;
            let total_fees = bluechip_fee_amt + creator_fee_amt;

            if contract_balance.amount < total_fees {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Contract has insufficient balance to pay fees. Contract has {}, needs {}",
                    contract_balance.amount, total_fees
                ))));
            }

            // Create fee transfer messages
            let bluechip_transfer =
                get_bank_transfer_to_msg(&fee_info.bluechip_address, &denom, bluechip_fee_amt)
                    .map_err(|e| {
                        ContractError::Std(StdError::generic_err(format!(
                            "Bluechip transfer failed: {}",
                            e
                        )))
                    })?;

            let creator_transfer =
                get_bank_transfer_to_msg(&fee_info.creator_address, &denom, creator_fee_amt)
                    .map_err(|e| {
                        ContractError::Std(StdError::generic_err(format!(
                            "Creator transfer failed: {}",
                            e
                        )))
                    })?;

            messages.push(bluechip_transfer);
            messages.push(creator_transfer);

            // Check if threshold has been hit
            let threshold_already_hit = THRESHOLD_HIT.load(deps.storage)?;

            if !threshold_already_hit {
                // PRE-THRESHOLD LOGIC
                let current_usd_raised = USD_RAISED.load(deps.storage)?;
                let new_total = current_usd_raised + usd_value;

                // Check if this commit will cross or exceed the threshold
                if new_total >= commit_config.commit_limit_usd {
                    // Try to acquire the threshold processing lock to trigger crossing
                    let processing = THRESHOLD_PROCESSING
                        .may_load(deps.storage)?
                        .unwrap_or(false);
                    let can_process = if processing {
                        false // Someone else is processing
                    } else {
                        THRESHOLD_PROCESSING.save(deps.storage, &true)?;
                        true // We get to process
                    };

                    if !can_process {
                        // Another transaction is handling the threshold crossing
                        // Process this as a normal pre-threshold commit

                        // Double-check if threshold was hit while we were waiting
                        if THRESHOLD_HIT.load(deps.storage)? {
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

                    // Double-check threshold hasn't been hit (defensive programming)
                    if THRESHOLD_HIT.load(deps.storage)? {
                        // Someone else hit it, clear our lock and process as post-threshold
                        THRESHOLD_PROCESSING.save(deps.storage, &false)?;
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

                    // Calculate exact amounts for threshold crossing
                    let usd_to_threshold = commit_config
                        .commit_limit_usd
                        .checked_sub(current_usd_raised)
                        .unwrap_or(Uint128::zero());
                    //if transaction crosess threshold and still has leftover funds (commit amount = $24,900 next commit = $500)
                    if usd_value > usd_to_threshold && usd_to_threshold > Uint128::zero() {
                        // Calculate the native amount that corresponds to reaching exactly $25k
                        let native_to_threshold =
                            usd_to_native(usd_to_threshold, oracle_data.price)?;
                        let native_excess = asset.amount.checked_sub(native_to_threshold)?;
                        let usd_excess = usd_value.checked_sub(usd_to_threshold)?;

                        // Update commit ledger with only the threshold portion
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default() + usd_to_threshold)
                        })?;

                        // Set USD raised to exactly the threshold
                        USD_RAISED.save(deps.storage, &commit_config.commit_limit_usd)?;
                        COMMITSTATUS.save(deps.storage, &commit_config.commit_limit_usd)?;

                        //mark threshold as hit
                        THRESHOLD_HIT.save(deps.storage, &true)?;

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
                                        commit.total_paid_native += native_to_threshold;
                                        commit.total_paid_usd += usd_to_threshold;
                                        commit.last_payment_native = native_to_threshold;
                                        commit.last_payment_usd = usd_to_threshold;
                                        commit.last_commited = env.block.time;
                                        Ok(commit)
                                    }
                                    None => Ok(Commiting {
                                        pool_id: pool_info.pool_id,
                                        commiter: sender.clone(),
                                        total_paid_native: native_to_threshold,
                                        total_paid_usd: usd_to_threshold,
                                        last_commited: env.block.time,
                                        last_payment_native: native_to_threshold,
                                        last_payment_usd: usd_to_threshold,
                                    }),
                                }
                            },
                        )?;

                        // Process the excess as a swap
                        let mut return_amt = Uint128::zero();
                        let mut spread_amt = Uint128::zero();
                        let mut commission_amt = Uint128::zero();

                        if native_excess > Uint128::zero() {
                            // Load updated pool state (modified by threshold payout)
                            let mut pool_state = POOL_STATE.load(deps.storage)?;
                            let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

                            // Perform swap with excess amount
                            let offer_pool = pool_state.reserve0;
                            let ask_pool = pool_state.reserve1;

                            if !ask_pool.is_zero() && !offer_pool.is_zero() {
                                let (ret_amt, sp_amt, comm_amt) = compute_swap(
                                    offer_pool,
                                    ask_pool,
                                    native_excess,
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
                                    native_excess,
                                    return_amt,
                                    spread_amt,
                                )?;
                            }

                            // Update reserves
                            pool_state.reserve0 = offer_pool.checked_add(native_excess)?;
                            pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;

                            // Update fee growth
                            update_fee_growth(&mut pool_fee_state, &pool_state, 0, commission_amt)?;

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

                            COMMIT_INFO.update(
                                deps.storage,
                                &sender,
                                |maybe_commiting| -> Result<_, ContractError> {
                                    if let Some(mut commiting) = maybe_commiting {
                                        commiting.total_paid_native += native_excess;
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
                            .add_attribute("total_amount_native", asset.amount.to_string())
                            .add_attribute(
                                "threshold_amount_native",
                                native_to_threshold.to_string(),
                            )
                            .add_attribute("swap_amount_native", native_excess.to_string())
                            .add_attribute("threshold_amount_usd", usd_to_threshold.to_string())
                            .add_attribute("swap_amount_usd", usd_excess.to_string())
                            .add_attribute("native_excess_spread", spread_amt.to_string())
                            .add_attribute("native_excess_returned", return_amt.to_string())
                            .add_attribute(
                                "native_excess_commission",
                                commission_amt.to_string(),
                            ));
                    } else {
                        //threshold hit on the nose.
                        // Update commit ledger
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default() + usd_value)
                        })?;

                        // Update total USD raised
                        let final_usd = if new_total > commit_config.commit_limit_usd {
                            commit_config.commit_limit_usd
                        } else {
                            new_total
                        };

                        USD_RAISED.save(deps.storage, &final_usd)?;
                        COMMITSTATUS.save(deps.storage, &final_usd)?;

                        // Mark threshold as hit
                        THRESHOLD_HIT.save(deps.storage, &true)?;

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
                                        commiting.total_paid_native += asset.amount;
                                        commiting.total_paid_usd += usd_value;
                                        commiting.last_payment_native = asset.amount;
                                        commiting.last_payment_usd = usd_value;
                                        commiting.last_commited = env.block.time;
                                        Ok(commiting)
                                    }
                                    None => Ok(Commiting {
                                        pool_id: pool_info.pool_id,
                                        commiter: sender.clone(),
                                        total_paid_native: asset.amount,
                                        total_paid_usd: usd_value,
                                        last_commited: env.block.time,
                                        last_payment_native: asset.amount,
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
                            .add_attribute("commit_amount_native", asset.amount.to_string())
                            .add_attribute("commit_amount_usd", usd_value.to_string()));
                    }
                } else {
                    // normal commit pre threshold (doesn't reach threshold)
                    return process_pre_threshold_commit(
                        deps, env, sender, &asset, usd_value, messages,
                    );
                }
            } else {
                // post threshold commit swap
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
    asset: &Asset,
    usd_value: Uint128,
    messages: Vec<CosmosMsg>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;

    // Update commit ledger
    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default() + usd_value)
    })?;

    // Update total USD raised
    let usd_total = USD_RAISED.update::<_, ContractError>(deps.storage, |r| Ok(r + usd_value))?;
    COMMITSTATUS.save(deps.storage, &usd_total)?;

    COMMIT_INFO.update(
        deps.storage,
        &sender,
        |maybe_commiting| -> Result<_, ContractError> {
            match maybe_commiting {
                Some(mut commiting) => {
                    commiting.total_paid_native += asset.amount;
                    commiting.total_paid_usd += usd_value;
                    commiting.last_payment_native = asset.amount;
                    commiting.last_payment_usd = usd_value;
                    commiting.last_commited = env.block.time;
                    Ok(commiting)
                }
                None => Ok(Commiting {
                    pool_id: pool_info.pool_id,
                    commiter: sender.clone(),
                    total_paid_native: asset.amount,
                    total_paid_usd: usd_value,
                    last_commited: env.block.time,
                    last_payment_native: asset.amount,
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
        .add_attribute("commit_amount_native", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string()))
}

//commit transaction post threshold - makes a swap with pool - still has fees taken out for creator and bluechip
fn process_post_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: Asset,
    usd_value: Uint128,
    mut messages: Vec<CosmosMsg>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let fee_info = FEEINFO.load(deps.storage)?;

    // Calculate net amount after fees
    let bluechip_fee_amt =
        asset.amount * fee_info.bluechip_fee.numerator() / fee_info.bluechip_fee.denominator();
    let creator_fee_amt =
        asset.amount * fee_info.creator_fee.numerator() / fee_info.creator_fee.denominator();
    let net_amount = asset
        .amount
        .checked_sub(bluechip_fee_amt + creator_fee_amt)?;

    // Load current pool state
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // Load current pool balances
    let offer_pool = pool_state.reserve0;
    let ask_pool = pool_state.reserve1;

    // Calculate swap output
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, net_amount, pool_specs.lp_fee)?;

    // Check slippage
    assert_max_spread(belief_price, max_spread, net_amount, return_amt, spread_amt)?;

    // Update reserves
    pool_state.reserve0 = offer_pool.checked_add(net_amount)?;
    pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;

    // Update fee growth
    update_fee_growth(&mut pool_fee_state, &pool_state, 0, commission_amt)?;
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
                    commiting.total_paid_native += asset.amount;
                    commiting.total_paid_usd += usd_value;
                    commiting.last_payment_native = asset.amount;
                    commiting.last_payment_usd = usd_value;
                    commiting.last_commited = env.block.time;
                    Ok(commiting)
                }
                None => Ok(Commiting {
                    pool_id: pool_info.pool_id,
                    commiter: sender.clone(),
                    total_paid_native: asset.amount,
                    total_paid_usd: usd_value,
                    last_commited: env.block.time,
                    last_payment_native: asset.amount,
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
        .add_attribute("commit_amount_native", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string())
        .add_attribute("net_offered", net_amount.to_string())
        .add_attribute("block_committed", env.block.time.to_string())
        .add_attribute("tokens_received", return_amt.to_string()))
}

pub fn execute_add_to_position(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    sender: Addr,
    amount0: Uint128, // native token
    amount1: Uint128, // cw20 token
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;

    // Rate limiting check
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    // Your existing function logic here...
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
        deadline,
    );

    // Always clear the guard before returning (even on error)
    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

pub fn execute_remove_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    // Rate limiting check
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = remove_liquidity(
        &mut deps,
        env,
        info.clone(),
        position_id,
        deadline,
        min_amount0,
        min_amount1,
    );

    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

pub fn execute_remove_partial_liquidity(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    // Rate limiting check
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
        deadline,
        min_amount0,
        min_amount1,
    );

    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

fn check_rate_limit(
    deps: &mut DepsMut,
    env: &Env,
    pool_specs: &PoolSpecs,
    sender: &Addr,
) -> Result<(), ContractError> {
    if let Some(last_commit_time) = USER_LAST_COMMIT.may_load(deps.storage, sender)? {
        let time_since_last = env.block.time.seconds() - last_commit_time;

        if time_since_last < pool_specs.min_commit_interval {
            let wait_time = pool_specs.min_commit_interval - time_since_last;
            return Err(ContractError::TooFrequentCommits { wait_time });
        }
    }

    USER_LAST_COMMIT.save(deps.storage, sender, &env.block.time.seconds())?;

    Ok(())
}

fn native_to_usd(
    cached_price: Uint128,
    native_amount: Uint128,
    expo: i32, // micro-native
) -> StdResult<Uint128> {
    if expo != -8 {
        return Err(StdError::generic_err(format!(
            "Unexpected price exponent: {}. Expected: -8",
            expo
        )));
    }
    // 2. convert: (µnative × price) / 10^(8-6) = µUSD
    let usd_micro_u256 = (Uint256::from(native_amount) * Uint256::from(cached_price))
        / Uint256::from(100_000_000u128); // 10^(8-6) = 100

    let usd_micro = Uint128::try_from(usd_micro_u256)?;
    Ok(usd_micro)
}

pub fn usd_to_native(
    usd_amount: Uint128,
    cached_price: Uint128, // micro-USD (6 decimals)
) -> StdResult<Uint128> {
    if cached_price.is_zero() {
        return Err(StdError::generic_err("Invalid zero price"));
    }
    let native_micro_u256 =
        (Uint256::from(usd_amount) * Uint256::from(100u128)) / Uint256::from(cached_price);
    Uint128::try_from(native_micro_u256).map_err(|_| StdError::generic_err("Overflow"))
}

pub fn get_and_validate_oracle_price(
    querier: &QuerierWrapper,
    oracle_addr: &Addr,
    symbol: &str,
    current_time: u64,
) -> StdResult<OracleData> {
    let resp: PriceResponse = querier
        .query_wasm_smart(
            oracle_addr.clone(),
            &PythQueryMsg::GetPrice {
                price_id: symbol.into(),
            },
        )
        .map_err(|e| StdError::generic_err(format!("Oracle query failed: {}", e)))?;

    // Staleness check - STANDARD PRACTICE
    let zero: Uint128 = Uint128::zero();
    if resp.price <= zero {
        return Err(StdError::generic_err(
            "Invalid zero or negative price from oracle",
        ));
    }

    if current_time.saturating_sub(resp.publish_time) > MAX_ORACLE_AGE {
        return Err(StdError::generic_err("Oracle price too stale"));
    }
    Ok(OracleData {
        price: resp.price,
        expo: resp.expo,
    })
}

pub fn validate_oracle_price_against_twap(
    deps: &Deps,
    oracle_price: Uint128,
    current_time: u64,
) -> Result<(), ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;

    // Calculate TWAP over last hour (or since last update)
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);

    if time_elapsed > 3600 && !pool_state.reserve0.is_zero() {
        // Calculate average price from accumulator
        let twap_price = pool_state
            .price0_cumulative_last
            .checked_div(Uint128::from(time_elapsed))
            .map_err(|_| ContractError::DivideByZero {})?;

        // Check deviation (allow 20% max)
        let deviation = if oracle_price > twap_price {
            Decimal::from_ratio(oracle_price - twap_price, twap_price)
        } else {
            Decimal::from_ratio(twap_price - oracle_price, twap_price)
        };

        if deviation > Decimal::percent(20) {
            return Err(ContractError::OraclePriceDeviation {
                oracle: oracle_price,
                twap: twap_price,
            });
        }
    }
    Ok(())
}

//deposit liquidity in pool
pub fn execute_deposit_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    amount0: Uint128, // native amount
    amount1: Uint128, // CW20 amount
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

    const NATIVE_DENOM: &str = "stake";
    let paid_native = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    let (liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;
    // Ensure the user sent enough native tokens
    if paid_native < actual_amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }

    if let Some(min0) = min_amount0 {
        if actual_amount0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: actual_amount0,
                token: "native".to_string(),
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

    // Refund excess native tokens
    let refund_amount = paid_native.checked_sub(actual_amount0)?;
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

    let mut pos_id = NEXT_POSITION_ID.load(deps.storage)?;
    pos_id += 1;
    NEXT_POSITION_ID.save(deps.storage, &pos_id)?;
    let position_id = pos_id.to_string();

    let metadata = TokenMetadata {
        name: Some(format!("LP Position #{}", position_id)),
        description: Some(format!("Pool Liquidity Position")),
    };

    let mint_liquidity_nft = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(), // Use the NFT contract address!
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
    let fee_multiplier = calculate_fee_multiplier(liquidity);
    let position = Position {
        liquidity,
        owner: user.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_multiplier,
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
        liquidity_position.fee_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier,
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
        let native_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: fees_owed_0,
            }],
        };
        response = response.add_message(native_msg);
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
    deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

    const NATIVE_DENOM: &str = "stake";
    let paid_native = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    let (additional_liquidity, actual_amount0, actual_amount1) =
        calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;

    if paid_native < actual_amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let mut messages: Vec<CosmosMsg> = vec![];

    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier,
    );

    if let Some(min0) = min_amount0 {
        if actual_amount0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: actual_amount0,
                token: "native".to_string(),
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

    let refund_amount = paid_native.checked_sub(actual_amount0)?;
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

    liquidity_position.liquidity += additional_liquidity;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();
    liquidity_position.fee_multiplier = calculate_fee_multiplier(liquidity_position.liquidity);

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

    if !fees_owed_0.is_zero() {
        let native_msg = BankMsg::Send {
            to_address: user.to_string(),
            amount: vec![Coin {
                denom: NATIVE_DENOM.to_string(),
                amount: fees_owed_0,
            }],
        };
        response = response.add_message(native_msg);
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

// User Remove liquidity
pub fn remove_liquidity(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

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

    if let Some(min0) = min_amount0 {
        if user_share_0 < min0 {
            return Err(ContractError::SlippageExceeded {
                expected: min0,
                actual: user_share_0,
                token: "native".to_string(),
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

    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier,
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

    if !total_amount_0.is_zero() {
        let native_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: total_amount_0,
            }],
        };
        response = response.add_message(native_msg);
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
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    // Specific amount of liquidity to remove
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

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

    if liquidity_to_remove == liquidity_position.liquidity {
        return execute_remove_liquidity(
            deps.branch(),
            env,
            info,
            position_id,
            deadline,
            min_amount0,
            min_amount1,
        );
    }

    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    let fees_owed_0 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier,
    );

    let withdrawal_amount_0 = liquidity_to_remove
        .checked_mul(current_reserve0)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    let withdrawal_amount_1 = liquidity_to_remove
        .checked_mul(current_reserve1)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    let total_amount_0 = withdrawal_amount_0 + fees_owed_0;
    let total_amount_1 = withdrawal_amount_1 + fees_owed_1;

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

    if !total_amount_0.is_zero() {
        let native_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![Coin {
                denom: "stake".to_string(),
                amount: total_amount_0,
            }],
        };
        response = response.add_message(native_msg);
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

fn enforce_deadline(current: Timestamp, deadline: Option<Timestamp>) -> Result<(), ContractError> {
    if let Some(dl) = deadline {
        if current > dl {
            return Err(ContractError::TransactionExpired {});
        }
    }
    Ok(())
}

pub fn execute_remove_partial_liquidity_by_percent(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    percentage: u64,
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    // Validate percentage
    if percentage == 0 {
        return Err(ContractError::InvalidPercent {});
    }

    if percentage >= 100 {
        // Redirect to full removal
        return execute_remove_liquidity(
            deps,
            env,
            info,
            position_id,
            deadline,
            min_amount0,
            min_amount1,
        );
    }

    // Load position to calculate absolute amount
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    // Calculate liquidity amount to remove (simple integer math)
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
        liquidity_to_remove,
        deadline,
        min_amount0,
        min_amount1,
    )
}

// Helper function to calculate liquidity for deposits

fn calc_liquidity_for_deposit(
    deps: Deps,
    amount0: Uint128,
    amount1: Uint128,
) -> Result<(Uint128, Uint128, Uint128), ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    if current_reserve0.is_zero() || current_reserve1.is_zero() {
        // Add specific error to know WHICH is zero
        if current_reserve0.is_zero() {
            return Err(ContractError::Std(StdError::generic_err(
                "Reserve0 is zero",
            )));
        }
        if current_reserve1.is_zero() {
            return Err(ContractError::Std(StdError::generic_err(
                "Reserve1 is zero",
            )));
        }
    }
    if amount0.is_zero() || amount1.is_zero() {
        if amount0.is_zero() {
            return Err(ContractError::Std(StdError::generic_err("amount0 is zero")));
        }
        if amount1.is_zero() {
            return Err(ContractError::Std(StdError::generic_err("amount1 is zero")));
        }
    }

    let optimal_amount1_for_amount0 = (amount0 * current_reserve1) / current_reserve0;
    let optimal_amount0_for_amount1 = (amount1 * current_reserve0) / current_reserve1;

    let (final_amount0, final_amount1) = if optimal_amount1_for_amount0 <= amount1 {
        // User provided enough amount1, use all of amount0
        (amount0, optimal_amount1_for_amount0)
    } else {
        // User didn't provide enough amount1, use all their amount1 and scale down amount0
        (optimal_amount0_for_amount1, amount1)
    };

    if final_amount0.is_zero() || final_amount1.is_zero() {
        return Err(ContractError::InsufficientLiquidity {});
    }

    let product = final_amount0.checked_mul(final_amount1)?;
    let liquidity = integer_sqrt(product).max(Uint128::new(1));

    if liquidity.is_zero() {
        return Err(ContractError::InsufficientLiquidityMinted {});
    }

    Ok((liquidity, final_amount0, final_amount1))
}

fn integer_sqrt(value: Uint128) -> Uint128 {
    if value.is_zero() {
        return Uint128::zero();
    }

    let mut x = value;
    let mut y = (value + Uint128::one()) / Uint128::new(2);

    while y < x {
        x = y;
        y = (y + value / y) / Uint128::new(2);
    }

    x
}
pub fn accumulate_prices(
    env: Env,
    pool_state: &PoolState,
    x: Uint128,
    y: Uint128,
) -> StdResult<Option<(Uint128, Uint128, u64)>> {
    let block_time = env.block.time.seconds();
    if block_time <= pool_state.block_time_last {
        return Ok(None);
    }

    let time_elapsed = Uint128::from(block_time - pool_state.block_time_last);

    let mut pcl0 = pool_state.price0_cumulative_last;
    let mut pcl1 = pool_state.price1_cumulative_last;

    if !x.is_zero() && !y.is_zero() {
        let price_precision = Uint128::from(10u128.pow(TWAP_PRECISION.into()));
        pcl0 = pool_state.price0_cumulative_last.wrapping_add(
            time_elapsed
                .checked_mul(price_precision)?
                .multiply_ratio(y, x),
        );
        pcl1 = pool_state.price1_cumulative_last.wrapping_add(
            time_elapsed
                .checked_mul(price_precision)?
                .multiply_ratio(x, y),
        );
    };

    Ok(Some((pcl0, pcl1, block_time)))
}

pub fn compute_swap(
    offer_pool: Uint128,
    ask_pool: Uint128,
    offer_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let offer_amount: Uint256 = offer_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;

    let cp: Uint256 = offer_pool * ask_pool;
    let return_amount: Uint256 = (Decimal256::from_ratio(ask_pool, 1u8)
        - Decimal256::from_ratio(cp, offer_pool + offer_amount))
    .numerator()
        / Decimal256::one().denominator();

    // Calculate spread & commission
    let spread_amount: Uint256 = (offer_amount
        * Decimal256::from_ratio(ask_pool, offer_pool).numerator()
        / Decimal256::from_ratio(ask_pool, offer_pool).denominator())
        - return_amount;
    let commission_amount: Uint256 =
        return_amount * commission_rate.numerator() / commission_rate.denominator();

    let return_amount: Uint256 = return_amount - commission_amount;
    Ok((
        return_amount.try_into()?,
        spread_amount.try_into()?,
        commission_amount.try_into()?,
    ))
}

pub fn validate_factory_address(
    stored_factory_addr: &Addr,
    candidate_factory_addr: &Addr,
) -> Result<(), ContractError> {
    if stored_factory_addr != candidate_factory_addr {
        return Err(ContractError::InvalidFactory {});
    }
    Ok(())
}

fn compute_offer_amount(
    offer_pool: Uint128,
    ask_pool: Uint128,
    ask_amount: Uint128,
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let _cp = Uint256::from(offer_pool) * Uint256::from(ask_pool);
    let one_minus_commission = Decimal256::one() - decimal2decimal256(commission_rate)?;
    let inv_one_minus_commission = Decimal256::one() / one_minus_commission;

    let ask_amount_256: Uint256 = ask_amount.into();
    let offer_amount: Uint256 = Uint256::from(
        ask_pool.checked_sub(
            (ask_amount_256 * inv_one_minus_commission.numerator()
                / inv_one_minus_commission.denominator())
            .try_into()?,
        )?,
    );

    let spread_amount = (offer_amount * Decimal256::from_ratio(ask_pool, offer_pool).numerator()
        / Decimal256::from_ratio(ask_pool, offer_pool).denominator())
    .checked_sub(offer_amount)?
    .try_into()?;
    let commission_amount = offer_amount * decimal2decimal256(commission_rate)?.numerator()
        / decimal2decimal256(commission_rate)?.denominator();
    Ok((
        offer_amount.try_into()?,
        spread_amount,
        commission_amount.try_into()?,
    ))
}

pub fn trigger_threshold_payout(
    storage: &mut dyn Storage,
    pool_info: &PoolInfo,
    pool_state: &mut PoolState,
    pool_fee_state: &mut PoolFeeState,
    commit_config: &CommitInfo,
    payout: &ThresholdPayout,
    fee_info: &FeeInfo,
    env: &Env,
) -> StdResult<Vec<CosmosMsg>> {
    let mut msgs = Vec::<CosmosMsg>::new();
    let total =
        payout.creator_amount + payout.bluechip_amount + payout.pool_amount + payout.commit_amount;

    if total != Uint128::new(1_200_000_000_000) {
        return Err(StdError::generic_err(
            "Threshold payout corruption detected",
        ));
    }

    let creator_ratio = payout.creator_amount.multiply_ratio(100u128, total);
    if creator_ratio < Uint128::new(26) || creator_ratio > Uint128::new(28) {
        return Err(StdError::generic_err("Invalid creator ratio"));
    }
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.creator_address,
        payout.creator_amount,
    )?);

    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.bluechip_address,
        payout.bluechip_amount,
    )?);

    msgs.push(mint_tokens(
        &pool_info.token_address,
        &env.contract.address,
        payout.pool_amount + commit_config.commit_amount_for_threshold,
    )?);

    let held_amount = payout.commit_amount;
    for payer_res in COMMIT_LEDGER.keys(storage, None, None, Order::Ascending) {
        // unwrap the StdResult<Addr> into an Addr
        let payer: Addr = payer_res?;

        let usd_paid = COMMIT_LEDGER.load(storage, &payer)?;
        let reward = Uint128::try_from(
            (Uint256::from(usd_paid) * Uint256::from(held_amount))
                / Uint256::from(commit_config.commit_limit_usd),
        )?;

        if !reward.is_zero() {
            msgs.push(mint_tokens(&pool_info.token_address, &payer, reward)?);
        }
    }
    COMMIT_LEDGER.clear(storage);

    let denom = match &pool_info.pair_info.asset_infos[0] {
        AssetInfo::NativeToken { denom, .. } => denom,
        _ => "stake", // fallback if first asset isn't native
    };
    let native_seed = Uint128::new(23_500);
    msgs.push(get_bank_transfer_to_msg(
        &env.contract.address,
        denom,
        native_seed,
    )?);

    pool_state.reserve0 = native_seed; // No LP positions created yet
    pool_state.reserve1 = payout.pool_amount; // No LP positions created yet
                                              //Initial seed liquidity is not owned by anyone and cannot be withdrawn. This is intentional to prevent pool draining attacks
    pool_state.total_liquidity = Uint128::zero();
    pool_fee_state.fee_growth_global_0 = Decimal::zero();
    pool_fee_state.fee_growth_global_1 = Decimal::zero();
    pool_fee_state.total_fees_collected_0 = Uint128::zero();
    pool_fee_state.total_fees_collected_1 = Uint128::zero();

    POOL_STATE.save(storage, pool_state)?;
    POOL_FEE_STATE.save(storage, pool_fee_state)?;

    Ok(msgs)
}

pub fn assert_max_spread(
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    offer_amount: Uint128,
    return_amount: Uint128,
    spread_amount: Uint128,
) -> Result<(), ContractError> {
    let default_spread = Decimal::from_str(DEFAULT_SLIPPAGE)?;
    let max_allowed_spread = Decimal::from_str(MAX_ALLOWED_SLIPPAGE)?;

    let max_spread = max_spread.unwrap_or(default_spread);
    if belief_price == Some(Decimal::zero()) {
        return Err(ContractError::InvalidBeliefPrice {});
    }
    if max_spread.gt(&max_allowed_spread) {
        return Err(ContractError::AllowedSpreadAssertion {});
    }

    if let Some(belief_price) = belief_price {
        let expected_return = offer_amount * belief_price.inv().unwrap().numerator()
            / belief_price.inv().unwrap().denominator();
        let spread_amount = expected_return
            .checked_sub(return_amount)
            .unwrap_or_else(|_| Uint128::zero());

        if return_amount < expected_return
            && Decimal::from_ratio(spread_amount, expected_return) > max_spread
        {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    } else if Decimal::from_ratio(spread_amount, return_amount + spread_amount) > max_spread {
        return Err(ContractError::MaxSpreadAssertion {});
    }

    Ok(())
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    let version = get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(ContractError::CannotMigrate {
            previous_contract: version.contract,
        });
    }
    if version.version != CONTRACT_VERSION {
        return Err(ContractError::CannotMigrate {
            previous_contract: version.contract,
        });
    }
    Ok(Response::new()
        .add_attribute("previous_contract_name", &version.contract)
        .add_attribute("previous_contract_version", &version.version)
        .add_attribute("new_contract_name", CONTRACT_NAME)
        .add_attribute("new_contract_version", CONTRACT_VERSION))
}

pub fn decimal2decimal256(dec_value: Decimal) -> StdResult<Decimal256> {
    Decimal256::from_atomics(dec_value.atomics(), dec_value.decimal_places()).map_err(|_| {
        StdError::generic_err(format!(
            "Failed to convert Decimal {} to Decimal256",
            dec_value
        ))
    })
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
fn mint_tokens(token_addr: &Addr, recipient: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    // CW20 mint message
    let mint_msg = Cw20ExecuteMsg::Mint {
        recipient: recipient.to_string(),
        amount,
    };

    // wrap in a Wasm execute
    let exec = WasmMsg::Execute {
        contract_addr: token_addr.to_string(),
        msg: to_json_binary(&mint_msg)?,
        funds: vec![],
    };

    Ok(exec.into())
}

pub fn query_check_threshold_limit(deps: Deps) -> StdResult<CommitStatus> {
    let threshold_hit = THRESHOLD_HIT.load(deps.storage)?;
    let commit_config = COMMIT_CONFIG.load(deps.storage)?;
    if threshold_hit {
        Ok(CommitStatus::FullyCommitted)
    } else {
        let usd_raised = USD_RAISED.load(deps.storage)?;
        Ok(CommitStatus::InProgress {
            raised: usd_raised,
            target: commit_config.commit_limit_usd,
        })
    }
}

fn calculate_fees_owed(
    liquidity: Uint128,
    fee_growth_global: Decimal,
    fee_growth_last: Decimal,
    fee_multiplier: Decimal,
) -> Uint128 {
    if fee_growth_global >= fee_growth_last {
        let fee_growth_delta = fee_growth_global - fee_growth_last;
        let earned_base = liquidity * fee_growth_delta;
        let earned_adjusted = earned_base * fee_multiplier; // Apply multiplier
        earned_adjusted
    } else {
        Uint128::zero()
    }
}

pub fn calculate_fee_multiplier(liquidity: Uint128) -> Decimal {
    pub const OPTIMAL_LIQUIDITY: Uint128 = Uint128::new(1_000_000); // Adjust based on your token decimals
    pub const MIN_MULTIPLIER: &str = "0.1"; // 10% fees for tiny positions

    if liquidity >= OPTIMAL_LIQUIDITY {
        Decimal::one() // Full fees
    } else {
        // Linear scaling from 10% to 100%
        let ratio = Decimal::from_ratio(liquidity, OPTIMAL_LIQUIDITY);
        let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap();
        min_mult + (Decimal::one() - min_mult) * ratio
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

fn verify_position_ownership(
    deps: Deps,
    nft_contract: &Addr,
    token_id: &str,
    expected_owner: &Addr,
) -> Result<(), ContractError> {
    let owner_response: cw721::OwnerOfResponse = deps.querier.query_wasm_smart(
        nft_contract,
        &cw721::Cw721QueryMsg::OwnerOf {
            token_id: token_id.to_string(),
            include_expired: None,
        },
    )?;

    if owner_response.owner != expected_owner.to_string() {
        return Err(ContractError::Unauthorized {});
    }

    Ok(())
}

fn calculate_unclaimed_fees(
    liquidity: Uint128,
    fee_growth_inside_last: Decimal,
    fee_growth_global: Decimal,
) -> Uint128 {
    if fee_growth_global > fee_growth_inside_last {
        let fee_growth_delta = fee_growth_global - fee_growth_inside_last;
        liquidity * fee_growth_delta
    } else {
        Uint128::zero()
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::PoolState {} => to_json_binary(&query_pool_state(deps)?),
        QueryMsg::FeeState {} => to_json_binary(&query_fee_state(deps)?),
        QueryMsg::Position { position_id } => to_json_binary(&query_position(deps, position_id)?),
        QueryMsg::Positions { start_after, limit } => {
            to_json_binary(&query_positions(deps, start_after, limit)?)
        }
        QueryMsg::PoolCommits {
            pool_id,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        } => to_json_binary(&query_pool_commiters(
            deps,
            pool_id,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        )?),
        QueryMsg::PositionsByOwner {
            owner,
            start_after,
            limit,
        } => to_json_binary(&query_positions_by_owner(deps, owner, start_after, limit)?),
        QueryMsg::PoolInfo {} => to_json_binary(&query_pool_info(deps)?),
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::Simulation { offer_asset } => {
            to_json_binary(&query_simulation(deps, offer_asset)?)
        }
        QueryMsg::ReverseSimulation { ask_asset } => {
            to_json_binary(&query_reverse_simulation(deps, ask_asset)?)
        }
        QueryMsg::LastCommited { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let response = match COMMIT_INFO.may_load(deps.storage, &addr)? {
                Some(commiting) => LastCommitedResponse {
                    has_commited: true,
                    last_commited: Some(commiting.last_commited),
                    last_payment_native: Some(commiting.last_payment_native),
                    last_payment_usd: Some(commiting.last_payment_usd),
                },
                None => LastCommitedResponse {
                    has_commited: false,
                    last_commited: None,
                    last_payment_native: None,
                    last_payment_usd: None,
                },
            };
            to_json_binary(&response)
        }
        QueryMsg::CumulativePrices {} => to_json_binary(&query_cumulative_prices(deps, env)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),
        QueryMsg::IsFullyCommited {} => to_json_binary(&query_check_threshold_limit(deps)?),
        QueryMsg::CommitingInfo { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let info = COMMIT_INFO.may_load(deps.storage, &addr)?;
            to_json_binary(&info)
        }
    }
}

pub fn query_pair_info(deps: Deps) -> StdResult<PairInfo> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pair_info)
}

pub fn query_pool(deps: Deps) -> StdResult<PoolResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info)?;

    let resp = PoolResponse { assets };

    Ok(resp)
}

pub fn query_simulation(deps: Deps, offer_asset: Asset) -> StdResult<SimulationResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let contract_addr = pool_info.pair_info.contract_addr.clone();

    let pools: [Asset; 2] = pool_info
        .pair_info
        .query_pools(&deps.querier, contract_addr)?;

    let offer_pool: Asset;
    let ask_pool: Asset;
    if offer_asset.info.equal(&pools[0].info) {
        offer_pool = pools[0].clone();
        ask_pool = pools[1].clone();
    } else if offer_asset.info.equal(&pools[1].info) {
        offer_pool = pools[1].clone();
        ask_pool = pools[0].clone();
    } else {
        return Err(StdError::generic_err(
            "Given offer asset does not belong in the pair",
        ));
    }

    let fee_info = FEEINFO.load(deps.storage)?;
    let commission_rate = fee_info.bluechip_fee + fee_info.creator_fee;

    let (return_amount, spread_amount, commission_amount) = compute_swap(
        offer_pool.amount,
        ask_pool.amount,
        offer_asset.amount,
        commission_rate,
    )?;

    Ok(SimulationResponse {
        return_amount,
        spread_amount,
        commission_amount,
    })
}

pub fn query_reverse_simulation(
    deps: Deps,
    ask_asset: Asset,
) -> StdResult<ReverseSimulationResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let contract_addr = pool_info.pair_info.contract_addr.clone();

    let pools: [Asset; 2] = pool_info
        .pair_info
        .query_pools(&deps.querier, contract_addr)?;

    let offer_pool: Asset;
    let ask_pool: Asset;
    if ask_asset.info.equal(&pools[0].info) {
        ask_pool = pools[0].clone();
        offer_pool = pools[1].clone();
    } else if ask_asset.info.equal(&pools[1].info) {
        ask_pool = pools[1].clone();
        offer_pool = pools[0].clone();
    } else {
        return Err(StdError::generic_err(
            "Given ask asset doesn't belong to pairs",
        ));
    }

    let fee_info = FEEINFO.load(deps.storage)?;
    let commission_rate = fee_info.bluechip_fee + fee_info.creator_fee;

    let (offer_amount, spread_amount, commission_amount) = compute_offer_amount(
        offer_pool.amount,
        ask_pool.amount,
        ask_asset.amount,
        commission_rate,
    )?;

    Ok(ReverseSimulationResponse {
        offer_amount,
        spread_amount,
        commission_amount,
    })
}

pub fn query_cumulative_prices(deps: Deps, env: Env) -> StdResult<CumulativePricesResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info.clone())?;

    let mut price0_cumulative_last = pool_state.price0_cumulative_last;
    let mut price1_cumulative_last = pool_state.price1_cumulative_last;

    if let Some((price0_cumulative_new, price1_cumulative_new, _)) =
        accumulate_prices(env, &pool_state, assets[0].amount, assets[1].amount)?
    {
        price0_cumulative_last = price0_cumulative_new;
        price1_cumulative_last = price1_cumulative_new;
    }

    let resp = CumulativePricesResponse {
        assets,
        price0_cumulative_last,
        price1_cumulative_last,
    };

    Ok(resp)
}

pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(ConfigResponse {
        block_time_last: pool_state.block_time_last,
        params: None,
    })
}

pub fn query_fee_info(deps: Deps) -> StdResult<FeeInfoResponse> {
    let fee_info = FEEINFO.load(deps.storage)?;
    Ok(FeeInfoResponse { fee_info })
}

pub fn query_check_commit(deps: Deps) -> StdResult<bool> {
    let commit_info = COMMIT_CONFIG.load(deps.storage)?;
    let usd_raised = COMMITSTATUS.load(deps.storage)?;
    // true once we've raised at least the USD threshold
    Ok(usd_raised >= commit_info.commit_limit_usd)
}

fn query_pool_state(deps: Deps) -> StdResult<PoolStateResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(PoolStateResponse {
        nft_ownership_accepted: pool_state.nft_ownership_accepted,
        reserve0: pool_state.reserve0,
        reserve1: pool_state.reserve1,
        total_liquidity: pool_state.total_liquidity,
        block_time_last: pool_state.block_time_last,
    })
}

fn query_fee_state(deps: Deps) -> StdResult<PoolFeeStateResponse> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?; // Since fees are in PoolState
    Ok(PoolFeeStateResponse {
        fee_growth_global_0: pool_fee_state.fee_growth_global_0,
        fee_growth_global_1: pool_fee_state.fee_growth_global_1,
        total_fees_collected_0: pool_fee_state.total_fees_collected_0,
        total_fees_collected_1: pool_fee_state.total_fees_collected_1,
    })
}

fn query_position(deps: Deps, position_id: String) -> StdResult<PositionResponse> {
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let unclaimed_fees_0 = calculate_unclaimed_fees(
        liquidity_position.liquidity,
        liquidity_position.fee_growth_inside_0_last,
        pool_fee_state.fee_growth_global_0,
    );
    let unclaimed_fees_1 = calculate_unclaimed_fees(
        liquidity_position.liquidity,
        liquidity_position.fee_growth_inside_1_last,
        pool_fee_state.fee_growth_global_1,
    );

    Ok(PositionResponse {
        position_id,
        liquidity: liquidity_position.liquidity,
        owner: liquidity_position.owner,
        fee_growth_inside_0_last: liquidity_position.fee_growth_inside_0_last,
        fee_growth_inside_1_last: liquidity_position.fee_growth_inside_1_last,
        created_at: liquidity_position.created_at,
        last_fee_collection: liquidity_position.last_fee_collection,
        unclaimed_fees_0,
        unclaimed_fees_1,
    })
}

fn query_positions(
    deps: Deps,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PositionsResponse> {
    let limit = limit.unwrap_or(10).min(30) as usize;
    let start = start_after.as_ref().map(|s| Bound::exclusive(s.as_str()));

    let liquidity_positions: StdResult<Vec<_>> = LIQUIDITY_POSITIONS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .map(|item| {
            let (position_id, _position) = item?;
            query_position(deps, position_id)
        })
        .collect();

    Ok(PositionsResponse {
        positions: liquidity_positions?,
    })
}

fn query_positions_by_owner(
    deps: Deps,
    owner: String,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PositionsResponse> {
    let owner_addr = deps.api.addr_validate(&owner)?;
    let limit = limit.unwrap_or(10).min(30) as usize;
    let start = start_after.as_ref().map(|s| Bound::exclusive(s.as_str()));

    let positions: StdResult<Vec<_>> = LIQUIDITY_POSITIONS
        .range(deps.storage, start, None, Order::Ascending)
        .filter(|item| {
            item.as_ref()
                .map(|(_, position)| position.owner == owner_addr)
                .unwrap_or(false)
        })
        .take(limit)
        .map(|item| {
            let (position_id, _position) = item?;
            query_position(deps, position_id)
        })
        .collect();

    Ok(PositionsResponse {
        positions: positions?,
    })
}

fn query_pool_info(deps: Deps) -> StdResult<PoolInfoResponse> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let next_position_id = NEXT_POSITION_ID.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;

    Ok(PoolInfoResponse {
        pool_state: PoolStateResponse {
            nft_ownership_accepted: pool_state.nft_ownership_accepted,
            reserve0: pool_state.reserve0,
            reserve1: pool_state.reserve1,
            total_liquidity: pool_state.total_liquidity,
            block_time_last: pool_state.block_time_last,
        },
        fee_state: PoolFeeStateResponse {
            fee_growth_global_0: pool_fee_state.fee_growth_global_0,
            fee_growth_global_1: pool_fee_state.fee_growth_global_1,
            total_fees_collected_0: pool_fee_state.total_fees_collected_0,
            total_fees_collected_1: pool_fee_state.total_fees_collected_1,
        },
        total_positions: next_position_id,
    })
}

fn query_pool_commiters(
    deps: Deps,
    pool_id: u64,
    min_payment_usd: Option<Uint128>,
    after_timestamp: Option<u64>,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PoolCommitResponse> {
    let limit = limit.unwrap_or(30).min(100) as usize;

    // Create the bound - handle the lifetime properly
    let start_addr = start_after
        .map(|addr_str| deps.api.addr_validate(&addr_str))
        .transpose()?;

    let start = start_addr.as_ref().map(Bound::exclusive);

    let mut commiters = vec![];
    let mut count = 0;

    for item in COMMIT_INFO.range(deps.storage, start, None, Order::Ascending) {
        let (commiter_addr, commiting) = item?;

        // Filter by pool_id
        if commiting.pool_id != pool_id {
            continue;
        }

        // Apply optional filters
        if let Some(min_usd) = min_payment_usd {
            if commiting.last_payment_usd < min_usd {
                continue;
            }
        }

        if let Some(after_ts) = after_timestamp {
            if commiting.last_commited.seconds() < after_ts {
                continue;
            }
        }

        commiters.push(CommiterInfo {
            wallet: commiter_addr.to_string(),
            last_payment_native: commiting.last_payment_native,
            last_payment_usd: commiting.last_payment_usd,
            last_commited: commiting.last_commited,
            total_paid_usd: commiting.total_paid_usd,
        });

        count += 1;

        // Stop if we've collected enough
        if commiters.len() >= limit {
            break;
        }
    }

    Ok(PoolCommitResponse {
        total_count: count,
        commiters,
    })
}
