#![allow(non_snake_case)]
use crate::asset::{call_pool_info, Asset, AssetInfo, PairType};
use crate::error::ContractError;
use crate::msg::{
    CommitStatus, ConfigResponse, CumulativePricesResponse, Cw20HookMsg, ExecuteMsg, FeeInfo,
    FeeInfoResponse, LastSubscribedResponse, MigrateMsg, PoolFeeStateResponse, PoolInfoResponse,
    PoolInitParams, PoolInstantiateMsg, PoolResponse, PoolStateResponse, PoolSubscribersResponse,
    PositionResponse, PositionsResponse, QueryMsg, ReverseSimulationResponse, SimulationResponse,
    SubscriberInfo,
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
    REENTRANCY_GUARD,
    THRESHOLD_HIT,
    THRESHOLD_PAYOUT,
    USD_RAISED,
    USER_LAST_COMMIT, //ACCUMULATED_BLUECHIP_FEES, ACCUMULATED_CREATOR_FEES,
};
use crate::state::{
    PoolState, Position, Subscription, TokenMetadata, LIQUIDITY_POSITIONS, NEXT_POSITION_ID,
    SUB_INFO,
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

/// ## Description
/// Creates a new contract with the specified parameters in the [`InstantiateMsg`].
/// Returns the [`Response`] with the specified attributes if the operation was successful, or a [`ContractError`] if
/// the contract was not created.
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
/// * **env** is an object of type [`Env`].
///
/// * **_info** is an object of type [`MessageInfo`].
/// * **msg** is a message of type [`InstantiateMsg`] which contains the basic settings for creating a contract.

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
    let init_params = if let Some(params_binary) = msg.init_params {
        let params: PoolInitParams = from_json(&params_binary)?;
        // CRITICAL: Validate the params match expected values
        validate_pool_init_params(&params)?;
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
        fee_multiplier: Decimal::one()
    };

    let pool_specs = PoolSpecs {
        subscription_period: 2592000,   // 30 days in seconds
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayout {
        creator_amount: init_params.creator_amount,
        bluechip_amount: init_params.bluechip_amount,
        pool_amount: init_params.pool_amount,
        commit_amount: init_params.commit_amount,
    };

    let commit_config = CommitInfo {
        commit_limit: msg.commit_limit,
        commit_limit_usd: msg.commit_limit_usd,
        available_payment: msg.available_payment.clone(),
        available_payment_usd: msg.available_payment_usd.clone(),
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
fn validate_pool_init_params(params: &PoolInitParams) -> Result<(), ContractError> {
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
        } => commit(deps, env, info, asset, amount, deadline),
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
        ExecuteMsg::DepositLiquidity { amount0, amount1 } => {
            // Check threshold requirement (same as swap)
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            execute_deposit_liquidity(deps, env, info, sender, amount0, amount1, None, None, None)
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
                sender,
                position_id,
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
        } => execute_remove_partial_liquidity(
            deps,
            env,
            info,
            position_id,
            liquidity_to_remove,
            deadline,
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
        } => execute_remove_partial_liquidity_by_percent(deps, env, info, position_id, percentage),
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

/// ## Description
/// Receives a message of type [`Cw20ReceiveMsg`] and processes it depending on the received template.
/// If the template is not found in the received message, then an [`ContractError`] is returned,
/// otherwise it returns the [`Response`] with the specified attributes if the operation was successful.
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
/// * **env** is an object of type [`Env`].
///
/// * **info** is an object of type [`MessageInfo`].
///
/// * **cw20_msg** is an object of type [`Cw20ReceiveMsg`]. This is the CW20 message that has to be processed.
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
                None,
            )
        }
        Ok(Cw20HookMsg::DepositLiquidity { amount0 }) => execute_deposit_liquidity(
            deps,
            env,
            info,
            Addr::unchecked(cw20_msg.sender),
            amount0,
            cw20_msg.amount,
            None,
            None,
            None,
        ),
        Ok(Cw20HookMsg::AddToPosition {
            position_id,
            amount0,
        }) => execute_add_to_position(
            deps,
            env,
            info,
            Addr::unchecked(cw20_msg.sender),
            position_id,
            amount0,
            cw20_msg.amount,
            None,
            None,
            None,
        ),
        Err(err) => Err(ContractError::Std(err)),
    }
}

/// ## Description
/// Performs an swap operation with the specified parameters. The trader must approve the
/// pool contract to transfer offer assets from their wallet.
/// Returns an [`ContractError`] on failure, otherwise returns the [`Response`] with the specified attributes if the operation was successful.
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
/// * **env** is an object of type [`Env`].
///
/// * **info** is an object of type [`MessageInfo`].
///
/// * **sender** is an object of type [`Addr`]. This is the sender of the swap operation.
///
/// * **offer_asset** is an object of type [`Asset`]. Proposed asset for swapping.
///
/// * **belief_price** is an object of type [`Option<Decimal>`]. Used to calculate the maximum swap spread.
///
/// * **max_spread** is an object of type [`Option<Decimal>`]. Sets the maximum spread of the swap operation.
///
/// * **to** is an object of type [`Option<Addr>`]. Sets the recipient of the swap operation.
/// NOTE - the address that wants to swap should approve the pair contract to pull the offer token.
#[allow(clippy::too_many_arguments)]
pub fn simple_swap(
    deps: DepsMut,
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
    // Load necessary data
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;

    // Query current pool balances
    let (offer_pool_idx, offer_pool, ask_pool) =
        if offer_asset.info.equal(&pool_info.pair_info.asset_infos[0]) {
            (0, pool_state.reserve0, pool_state.reserve1)
        } else if offer_asset.info.equal(&pool_info.pair_info.asset_infos[1]) {
            (1, pool_state.reserve1, pool_state.reserve0)
        } else {
            return Err(ContractError::AssetMismatch {});
        };

    // Perform swap calculations
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, offer_asset.amount, pool_specs.lp_fee)?;

    // Validate slippage
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

    // Update price accumulator
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
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
    // Reentrancy protection - check and set guard
    let reentrancy_guard = REENTRANCY_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if reentrancy_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    REENTRANCY_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    // Rate limiting check
    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        REENTRANCY_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    // Your existing function logic here...
    let result = execute_commit_logic(&mut deps, env, info, asset, amount, None, None);

    // Always clear the guard before returning (even on error)
    REENTRANCY_GUARD.save(deps.storage, &false)?;

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

    // Update the last commit time
    USER_LAST_COMMIT.save(deps.storage, sender, &env.block.time.seconds())?;

    Ok(())
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
    // Load all necessary data from separate storage
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
    let usd_value = native_to_usd(oracle_data.price, asset.amount, oracle_data.expo)?;

    match asset.info {
        AssetInfo::NativeToken { denom } if denom == "stake" => {
            // Verify funds were actually sent
            let sent = info
                .funds
                .iter()
                .find(|c| c.denom == denom)
                .map(|c| c.amount)
                .unwrap_or_default();
            if sent < amount {
                return Err(ContractError::MismatchAmount {});
            }

            // Payment validation
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

            if !THRESHOLD_HIT.load(deps.storage)? {
                // Get current USD raised
                let current_usd_raised = USD_RAISED.load(deps.storage)?;

                // Calculate how much USD is needed to reach threshold
                let usd_to_threshold = commit_config
                    .commit_limit_usd
                    .checked_sub(current_usd_raised)
                    .unwrap_or(Uint128::zero());

                // Check if this commit will cross the threshold
                if usd_value > usd_to_threshold && usd_to_threshold > Uint128::zero() {
                    // SPLIT COMMIT SCENARIO
                    // Calculate the native amount that corresponds to reaching exactly $25k
                    let native_to_threshold = usd_to_native(usd_to_threshold, oracle_data.price)?;

                    // Calculate the excess that will be swapped
                    let native_excess = asset.amount.checked_sub(native_to_threshold)?;
                    let usd_excess = usd_value.checked_sub(usd_to_threshold)?;

                    // Part 1: Process the amount needed to reach threshold
                    // Update commit ledger with only the threshold portion
                    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                        Ok(v.unwrap_or_default() + usd_to_threshold)
                    })?;

                    // Update total USD raised to exactly the threshold
                    USD_RAISED.save(deps.storage, &commit_config.commit_limit_usd)?;
                    COMMITSTATUS.save(deps.storage, &commit_config.commit_limit_usd)?;

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

                    // Update subscription with threshold portion
                    SUB_INFO.update(
                        deps.storage,
                        &sender,
                        |maybe_sub| -> Result<_, ContractError> {
                            match maybe_sub {
                                Some(mut sub) => {
                                    sub.total_paid_native += native_to_threshold;
                                    sub.total_paid_usd += usd_to_threshold;
                                    sub.last_payment_native = native_to_threshold;
                                    sub.last_payment_usd = usd_to_threshold;
                                    sub.last_subscribed = env.block.time;
                                    Ok(sub)
                                }
                                None => Ok(Subscription {
                                    pool_id: pool_info.pool_id,
                                    subscriber: sender.clone(),
                                    total_paid_native: native_to_threshold,
                                    total_paid_usd: usd_to_threshold,
                                    last_subscribed: env.block.time,
                                    last_payment_native: native_to_threshold,
                                    last_payment_usd: usd_to_threshold,
                                }),
                            }
                        },
                    )?;

                    let mut return_amt = Uint128::zero();
                    let mut spread_amt = Uint128::zero();
                    let mut commission_amt = Uint128::zero();
                    // Part 2: Process the excess as a swap
                    if native_excess > Uint128::zero() {
                        // Calculate fees on the excess amount
                        // Net amount for swap
                        let net_swap_amount = native_excess;

                        // Load current pool state (may have been modified by threshold payout)
                        let mut pool_state = POOL_STATE.load(deps.storage)?;
                        let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

                        // Perform swap with excess amount
                        let offer_pool = pool_state.reserve0;
                        let ask_pool = pool_state.reserve1;

                        if !ask_pool.is_zero() && !offer_pool.is_zero() {
                            let (ret_amt, sp_amt, comm_amt) = compute_swap(
                                offer_pool,
                                ask_pool,
                                net_swap_amount,
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
                                net_swap_amount,
                                return_amt,
                                spread_amt,
                            )?;
                        }

                        // Update reserves
                        pool_state.reserve0 = offer_pool.checked_add(net_swap_amount)?;
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

                        // Update subscription with swap portion
                        SUB_INFO.update(
                            deps.storage,
                            &sender,
                            |maybe_sub| -> Result<_, ContractError> {
                                match maybe_sub {
                                    Some(mut sub) => {
                                        sub.total_paid_native += native_excess;
                                        sub.total_paid_usd += usd_excess;
                                        Ok(sub)
                                    }
                                    None => unreachable!("Subscription was created above"),
                                }
                            },
                        )?;
                    }

                    // Return response for split commit
                    return Ok(Response::new()
                        .add_messages(messages)
                        .add_attribute("action", "commit")
                        .add_attribute("phase", "threshold_crossing")
                        .add_attribute("committer", sender)
                        .add_attribute("total_amount_native", asset.amount.to_string())
                        .add_attribute("threshold_amount_native", native_to_threshold.to_string())
                        .add_attribute("swap_amount_native", native_excess.to_string())
                        .add_attribute("threshold_amount_usd", usd_to_threshold.to_string())
                        .add_attribute("swap_amount_usd", usd_excess.to_string())
                        .add_attribute("native_excess_spread", spread_amt.to_string())
                        .add_attribute("native_excess_returned", return_amt.to_string())
                        .add_attribute("native_excess_commission", commission_amt.to_string()));
                }

                // NORMAL PRE-THRESHOLD COMMIT (doesn't cross threshold)
                // Update commit ledger normally
                COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                    Ok(v.unwrap_or_default() + usd_value)
                })?;

                // Update total USD raised
                let usd_total =
                    USD_RAISED.update::<_, ContractError>(deps.storage, |r| Ok(r + usd_value))?;
                COMMITSTATUS.save(deps.storage, &usd_total)?;

                // Check if we've exactly hit the threshold
                if usd_total >= commit_config.commit_limit_usd {
                    THRESHOLD_HIT.save(deps.storage, &true)?;
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
                }

                // Update subscription normally
                SUB_INFO.update(
                    deps.storage,
                    &sender,
                    |maybe_sub| -> Result<_, ContractError> {
                        match maybe_sub {
                            Some(mut sub) => {
                                sub.total_paid_native += asset.amount;
                                sub.total_paid_usd += usd_value;
                                sub.last_payment_native = asset.amount;
                                sub.last_payment_usd = usd_value;
                                sub.last_subscribed = env.block.time;
                                Ok(sub)
                            }
                            None => Ok(Subscription {
                                pool_id: pool_info.pool_id,
                                subscriber: sender.clone(),
                                total_paid_native: asset.amount,
                                total_paid_usd: usd_value,
                                last_subscribed: env.block.time,
                                last_payment_native: asset.amount,
                                last_payment_usd: usd_value,
                            }),
                        }
                    },
                )?;

                // Return early for pre-threshold commits
                return Ok(Response::new()
                    .add_messages(messages)
                    .add_attribute("action", "commit")
                    .add_attribute("phase", "funding")
                    .add_attribute("committer", sender)
                    .add_attribute("block_committed", env.block.time.to_string())
                    .add_attribute("commit_amount_native", asset.amount.to_string())
                    .add_attribute("commit_amount_usd", usd_value.to_string()));
            }
            // Post-threshold: handle swap for subscription
            let net_amount = asset
                .amount
                .checked_sub(bluechip_fee_amt + creator_fee_amt)?;

            // Load current pool state and fee state
            let mut pool_state = POOL_STATE.load(deps.storage)?;
            let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

            // Load current pool balances
            let offer_pool = pool_state.reserve0;
            let ask_pool = pool_state.reserve1;

            // Calculate swap output
            let (return_amt, spread_amt, commission_amt) =
                compute_swap(offer_pool, ask_pool, net_amount, pool_specs.lp_fee)?;

            assert_max_spread(
                belief_price,
                max_spread,
                net_amount, // offer_amount (what we're swapping)
                return_amt, // return_amount (what we're getting back)
                spread_amt, // spread_amount from compute_swap
            )?;

            // UPDATE RESERVES
            // Native (token0) increases, CW20 (token1) decreases
            pool_state.reserve0 = offer_pool.checked_add(net_amount)?;
            pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;

            // UPDATE FEE GROWTH
            update_fee_growth(
                &mut pool_fee_state,
                &pool_state,
                0, // Native is always index 0, fees always collected in token0
                commission_amt,
            )?;
            // Save fee state
            POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

            // Update price accumulator with new reserves
            update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

            // Save pool state
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

            // Record/extend subscription
            SUB_INFO.update(
                deps.storage,
                &sender,
                |maybe_sub| -> Result<_, ContractError> {
                    match maybe_sub {
                        Some(mut sub) => {
                            // Update totals
                            sub.total_paid_native += asset.amount;
                            sub.total_paid_usd += usd_value;
                            // Track most recent payment
                            sub.last_payment_native = asset.amount;
                            sub.last_payment_usd = usd_value;
                            sub.last_subscribed = env.block.time;
                            Ok(sub)
                        }
                        None => Ok(Subscription {
                            pool_id: pool_info.pool_id,
                            subscriber: sender.clone(),
                            total_paid_native: asset.amount,
                            total_paid_usd: usd_value,
                            last_subscribed: env.block.time,
                            last_payment_native: asset.amount,
                            last_payment_usd: usd_value,
                        }),
                    }
                },
            )?;

            // Return response
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
        _ => Err(ContractError::AssetMismatch {}),
    }
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

    if resp.price <= 0 {
        return Err(StdError::generic_err(
            "Invalid zero or negative price from oracle",
        ));
    }

    if current_time.saturating_sub(resp.publish_time) > MAX_ORACLE_AGE {
        return Err(StdError::generic_err("Oracle price too stale"));
    }
    Ok(OracleData {
        price: Uint128::from(resp.price as u64),
        expo: resp.expo,
    })
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

    // 1. Validate the native deposit (token0)
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

    // 2. Load the pool and update fee tracking
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // 3. Transfer CW20 tokens from user to pool (if amount1 > 0)
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
    // 4. Compute liquidity amount

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

    // 5. Generate position ID
    let mut pos_id = NEXT_POSITION_ID.load(deps.storage)?;
    pos_id += 1;
    NEXT_POSITION_ID.save(deps.storage, &pos_id)?;
    let position_id = pos_id.to_string();

    // 6. Create NFT metadata with position info
    let metadata = TokenMetadata {
        name: Some(format!("LP Position #{}", position_id)),
        description: Some(format!("Pool Liquidity Position")),
    };

    // 7. Mint the NFT on the external NFT contract
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
    // 8. Create and store the position with current fee growth
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

    // 9. Update pool state
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
    // 1. Load config
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;
    // 2. Verify NFT ownership through external NFT contract
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    // 3. Load position
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    // 4. Calculate fees owed to this position
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier
    );

    // 5. Update position's fee growth tracking
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    // 6. Prepare fee payments
    let mut response = Response::new()
        .add_attribute("action", "collect_fees")
        .add_attribute("position_id", position_id)
        .add_attribute("fees_0", fees_owed_0)
        .add_attribute("fees_1", fees_owed_1);

    // 7. Send native token fees (token0)
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

    // 8. Send CW20 token fees (token1)
    if !fees_owed_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(), // Using config.token_address
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

pub fn execute_add_to_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    position_id: String,
    amount0: Uint128, // native amount to add
    amount1: Uint128, // CW20 amount to add
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    deadline: Option<Timestamp>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

    // 1. Validate the native deposit (token0)
    const NATIVE_DENOM: &str = "stake";
    let paid_native = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    // 2. Load config
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    // 3. Verify NFT ownership through external NFT contract
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
    // 4. Load position
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let mut messages: Vec<CosmosMsg> = vec![];

    // 5. Calculate any pending fees FIRST (before diluting the position)
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier
    );

    // 6. Calculate new liquidity for the additional deposit

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
    // 7. Transfer only the actual CW20 amount needed
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

    // 8. Refund excess native tokens
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

    // 7. Update position with new totals and reset fee tracking
    liquidity_position.liquidity += additional_liquidity;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

    // 8. Update config state (just total liquidity)
    pool_state.total_liquidity += additional_liquidity;

    // add actual deposit amounts
    pool_state.reserve0 = pool_state.reserve0.checked_add(actual_amount0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_add(actual_amount1)?;

    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    // 9. Save updated position
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;

    // Note: Pool reserves will be automatically updated when tokens are transferred in
    // NFT metadata is immutable after minting in standard CW721

    // 10. Prepare response
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

    // 11. Send any pending fees to user (from before the addition)
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
            contract_addr: pool_info.token_address.to_string(), // Using config.token_address
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

// User Remove liquidity and burn NFT
pub fn execute_remove_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    deadline: Option<Timestamp>,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

    // 1. Load config
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    // 2. Load and validate position
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    // 3. Verify NFT ownership through external NFT contract
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    // 4. Get current pool reserves

    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    // 5. Calculate user's share of the pool (using your decimal logic)
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

    // 6. Calculate any remaining fees owed
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier
    );

    // 7. Total amounts to send (principal + fees)
    let total_amount_0 = user_share_0 + fees_owed_0;
    let total_amount_1 = user_share_1 + fees_owed_1;

    // 8. Update config state (total liquidity)
    let liquidity_to_subtract = liquidity_position.liquidity;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_subtract)?;

    // 9. Burn the NFT (on external NFT contract)
    /* let burn_msg = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(), // External NFT contract
        msg: to_json_binary(&cw721::Cw721ExecuteMsg::Burn {
            token_id: position_id.clone(),
        })?,
        funds: vec![],
    };*/

    // 10. Remove position from storage
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(user_share_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(user_share_1)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    LIQUIDITY_POSITIONS.remove(deps.storage, &position_id);

    // 11. Prepare response with token transfers
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

    // 12. Send native token (token0) - principal + fees
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

    // 13. Send CW20 token (token1) - principal + fees
    if !total_amount_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(), // Using config.token_address
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

pub fn execute_remove_partial_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    liquidity_to_remove: Uint128,
    deadline: Option<Timestamp>,
    // Specific amount of liquidity to remove
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;

    // 2. Load and validate position
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // 3. Verify NFT ownership through external NFT contract
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    // 4. Validate removal amount
    if liquidity_to_remove.is_zero() {
        return Err(ContractError::InvalidAmount {});
    }

    if liquidity_to_remove == liquidity_position.liquidity {
        return execute_remove_liquidity(deps, env, info, position_id, None, None, None);
    }

    if liquidity_to_remove > liquidity_position.liquidity {
        return Err(ContractError::InsufficientLiquidity {});
    }

    // 5. Get current pool reserves
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    // 6. Calculate propotional pending fees first (before any changes)
    let fees_owed_0 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
        liquidity_position.fee_multiplier
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_to_remove,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
        liquidity_position.fee_multiplier
    );

    // 7. Calculate partial withdrawal amounts (principal only, not fees)
    let withdrawal_amount_0 = liquidity_to_remove
        .checked_mul(current_reserve0)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    let withdrawal_amount_1 = liquidity_to_remove
        .checked_mul(current_reserve1)?
        .checked_div(pool_state.total_liquidity)
        .map_err(|_| ContractError::DivideByZero)?;

    // 8. Total amounts to send (partial principal + designated portion of accumulated fees)
    let total_amount_0 = withdrawal_amount_0 + fees_owed_0;
    let total_amount_1 = withdrawal_amount_1 + fees_owed_1;

    // 9. Update pool state
    pool_state.reserve0 = pool_state.reserve0.checked_sub(withdrawal_amount_0)?;
    pool_state.reserve1 = pool_state.reserve1.checked_sub(withdrawal_amount_1)?;
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_remove)?;
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    // 10. Update position
    liquidity_position.liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;

    // 11. Prepare response
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

    // 12. Send native token (token0) - partial principal + partial fees
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

    // 13. Send CW20 token (token1) - partial principal + partial fees
    if !total_amount_1.is_zero() {
        let cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(), // Using config.token_address
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
) -> Result<Response, ContractError> {
    // Validate percentage
    if percentage == 0 {
        return Err(ContractError::InvalidPercent {});
    }

    if percentage >= 100 {
        // Redirect to full removal
        return execute_remove_liquidity(deps, env, info, position_id, None, None, None);
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
    execute_remove_partial_liquidity(deps, env, info, position_id, liquidity_to_remove, None)
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
        return Err(ContractError::InsufficientLiquidity {});
    }

    if amount0.is_zero() || amount1.is_zero() {
        return Err(ContractError::InsufficientLiquidity {});
    }

    // 1. PROPORTIONAL DEPOSIT REQUIREMENT
    // Force deposits to maintain current pool ratio
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

    // 2. GEOMETRIC MEAN SHARE CALCULATION
    // Always use geometric mean for liquidity calculation (for ALL deposits)
    let product = final_amount0.checked_mul(final_amount1)?;
    let liquidity = integer_sqrt(product).max(Uint128::new(1));

    if liquidity.is_zero() {
        return Err(ContractError::InsufficientLiquidityMinted {});
    }

    Ok((liquidity, final_amount0, final_amount1))
}

/// ## Description
/// Accumulate token prices for the assets in the pool.
/// Note that this function shifts **block_time** when any of the token prices is zero in order to not
/// fill an accumulator with a null price for that period.
/// ## Params
/// * **env** is an object of type [`Env`].
///
/// * **config** is an object of type [`Config`].
///
/// * **x** is an object of type [`Uint128`]. This is the balance of asset\[\0] in the pool.
///
/// * **y** is an object of type [`Uint128`]. This is the balance of asset\[\1] in the pool.
///
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

    // We have to shift block_time when any price is zero in order to not fill an accumulator with a null price for that period
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

/// ## Description
/// Returns the result of a swap.
/// ## Params
/// * **offer_pool** is an object of type [`Uint128`]. This is the total amount of offer assets in the pool.
///
/// * **ask_pool** is an object of type [`Uint128`]. This is the total amount of ask assets in the pool.
///
/// * **offer_amount** is an object of type [`Uint128`]. This is the amount of offer assets to swap.
///
/// * **commission_rate** is an object of type [`Decimal`]. This is the total amount of fees charged for the swap.
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

    // offer => ask
    // ask_amount = (ask_pool - cp / (offer_pool + offer_amount))
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

    // The commision (minus the part that goes to the contract) will be absorbed by the pool
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
/// ## Description
/// Returns an amount of offer assets for a specified amount of ask assets.
/// ## Params
/// * **offer_pool** is an object of type [`Uint128`]. This is the total amount of offer assets in the pool.
///
/// * **ask_pool** is an object of type [`Uint128`]. This is the total amount of ask assets in the pool.
///
/// * **ask_amount** is an object of type [`Uint128`]. This is the amount of ask assets to swap to.
///
/// * **commission_rate** is an object of type [`Decimal`]. This is the total amount of fees charged for the swap.
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

    // Additional: Verify amounts match expected ratios
    // Creator should be ~27%, BlueChip ~2%, Pool ~29%, Commit ~42%
    let creator_ratio = payout.creator_amount.multiply_ratio(100u128, total);
    if creator_ratio < Uint128::new(26) || creator_ratio > Uint128::new(28) {
        return Err(StdError::generic_err("Invalid creator ratio"));
    }
    // 1. creator tokens
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.creator_address,
        payout.creator_amount,
    )?);

    // 2. bluechip tokens
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &fee_info.bluechip_address,
        payout.bluechip_amount,
    )?);

    // 3. pool + committed tokens to the pair contract itself
    msgs.push(mint_tokens(
        &pool_info.token_address,
        &env.contract.address,
        payout.pool_amount + commit_config.commit_limit,
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
            // now &payer is a &Addr
            msgs.push(mint_tokens(&pool_info.token_address, &payer, reward)?);
        }
    }
    COMMIT_LEDGER.clear(storage);

    // 4. seed the pool with 2350 native units
    let denom = match &pool_info.pair_info.asset_infos[0] {
        AssetInfo::NativeToken { denom, .. } => denom,
        _ => "stake", // fallback if first asset isn't native
    };
    let native_seed = Uint128::new(23_500); // Corrected comment
    msgs.push(get_bank_transfer_to_msg(
        &env.contract.address,
        denom,
        native_seed,
    )?);

    // 5. Initialize the pool state in CONFIG (instead of creating a new POOLS entry)
    // Note: The actual reserves will be tracked by token balances
    // We're just initializing the fee tracking and liquidity state
    pool_state.reserve0 = native_seed; // No LP positions created yet
    pool_state.reserve1 = payout.pool_amount; // No LP positions created yet
    pool_state.total_liquidity = Uint128::zero(); // No LP positions created yet
    pool_fee_state.fee_growth_global_0 = Decimal::zero();
    pool_fee_state.fee_growth_global_1 = Decimal::zero();
    pool_fee_state.total_fees_collected_0 = Uint128::zero();
    pool_fee_state.total_fees_collected_1 = Uint128::zero();

    POOL_STATE.save(storage, pool_state)?;
    POOL_FEE_STATE.save(storage, pool_fee_state)?;

    Ok(msgs)
}
/// ## Description
/// Returns a [`ContractError`] on failure.
/// If `belief_price` and `max_spread` are both specified, we compute a new spread,
/// otherwise we just use the swap spread to check `max_spread`.
/// ## Params
/// * **belief_price** is an object of type [`Option<Decimal>`]. This is the belief price used in the swap.
///
/// * **max_spread** is an object of type [`Option<Decimal>`]. This is the
/// max spread allowed so that the swap can be executed successfuly.
///
/// * **offer_amount** is an object of type [`Uint128`]. This is the amount of assets to swap.
///
/// * **return_amount** is an object of type [`Uint128`]. This is the amount of assets to receive from the swap.
///
/// * **spread_amount** is an object of type [`Uint128`]. This is the spread used in the swap.

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

/// ## Description
/// Used for the contract migration. Returns a default object of type [`Response`].
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
//
/// * **_env** is an object of type [`Env`].
///
/// * **_msg** is an object of type [`MigrateMsg`].
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

/// ## Description
/// Converts [`Decimal`] to [`Decimal256`].
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
    fee_multiplier: Decimal
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
        QueryMsg::PoolSubscribers {
            pool_id,
            min_payment_usd,
            after_timestamp,
            start_after,
            limit,
        } => to_json_binary(&query_pool_subscribers(
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
        QueryMsg::LastSubscribed { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let response = match SUB_INFO.may_load(deps.storage, &addr)? {
                Some(sub) => LastSubscribedResponse {
                    has_subscribed: true,
                    last_subscribed: Some(sub.last_subscribed),
                    last_payment_native: Some(sub.last_payment_native),
                    last_payment_usd: Some(sub.last_payment_usd),
                },
                None => LastSubscribedResponse {
                    has_subscribed: false,
                    last_subscribed: None,
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
        QueryMsg::SubscriptionInfo { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let info = SUB_INFO.may_load(deps.storage, &addr)?; // Option<Subscription>
            to_json_binary(&info)
        }
    }
}

/// ## Description
/// Returns information about the pair contract in an object of type [`PairInfo`].
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_pair_info(deps: Deps) -> StdResult<PairInfo> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pair_info)
}

/// ## Description
/// Returns the amounts of assets in the pair contract as well as the amount of LP
/// tokens currently minted in an object of type [`PoolResponse`].
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_pool(deps: Deps) -> StdResult<PoolResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info)?;

    let resp = PoolResponse { assets };

    Ok(resp)
}

/// ## Description
/// Returns information about a swap simulation in a [`SimulationResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
///
/// * **offer_asset** is an object of type [`Asset`]. This is the asset to swap as well as an amount of the said asset.
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

    // Get fee info from the factory contract

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

/// ## Description
/// Returns information about a reverse swap simulation in a [`ReverseSimulationResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
///
/// * **ask_asset** is an object of type [`Asset`]. This is the asset to swap to as well as the desired
/// amount of ask assets to receive from the swap.
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

    // Get fee info from factory

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

/// ## Description
/// Returns information about cumulative prices for the assets in the pool using a [`CumulativePricesResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
///
/// * **env** is an object of type [`Env`].
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

/// ## Description
/// Returns the pair contract configuration in a [`ConfigResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(ConfigResponse {
        block_time_last: pool_state.block_time_last,
        params: None,
    })
}

/// ## Description
/// Returns the pair contract configuration in a [`FeeInfoResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_fee_info(deps: Deps) -> StdResult<FeeInfoResponse> {
    let fee_info = FEEINFO.load(deps.storage)?;
    Ok(FeeInfoResponse { fee_info })
}

/// ## Description
/// Returns the pair contract configuration in a [`bool`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
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

    // Optionally calculate unclaimed fees
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

fn query_pool_subscribers(
    deps: Deps,
    pool_id: u64,
    min_payment_usd: Option<Uint128>,
    after_timestamp: Option<u64>,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<PoolSubscribersResponse> {
    let limit = limit.unwrap_or(30).min(100) as usize;

    // Create the bound - handle the lifetime properly
    let start_addr = start_after
        .map(|addr_str| deps.api.addr_validate(&addr_str))
        .transpose()?;

    let start = start_addr.as_ref().map(Bound::exclusive);

    let mut subscribers = vec![];
    let mut count = 0;

    // Iterate through all subscriptions
    for item in SUB_INFO.range(deps.storage, start, None, Order::Ascending) {
        let (subscriber_addr, sub) = item?;

        // Filter by pool_id
        if sub.pool_id != pool_id {
            continue;
        }

        // Apply optional filters
        if let Some(min_usd) = min_payment_usd {
            if sub.last_payment_usd < min_usd {
                continue;
            }
        }

        if let Some(after_ts) = after_timestamp {
            if sub.last_subscribed.seconds() < after_ts {
                continue;
            }
        }

        subscribers.push(SubscriberInfo {
            wallet: subscriber_addr.to_string(),
            last_payment_native: sub.last_payment_native,
            last_payment_usd: sub.last_payment_usd,
            last_subscribed: sub.last_subscribed,
            total_paid_usd: sub.total_paid_usd,
        });

        count += 1;

        // Stop if we've collected enough
        if subscribers.len() >= limit {
            break;
        }
    }

    Ok(PoolSubscribersResponse {
        total_count: count,
        subscribers,
    })
}

// Helper function to calculate unclaimed fees
