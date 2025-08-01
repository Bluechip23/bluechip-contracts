#![allow(non_snake_case)]
use crate::asset::{
    call_pool_info, Asset, AssetInfo, NativeTierInfo, PairType, PaymentInfoResponse,
    PaymentTiersResponseWithTolerance, USDTierInfoWithTolerance,
};
use crate::error::ContractError;
use crate::msg::{
    CommitStatus, ConfigResponse, CumulativePricesResponse, Cw20HookMsg, ExecuteMsg, FeeInfo,
    FeeInfoResponse, MigrateMsg, PoolInitParams, PoolInstantiateMsg, PoolResponse, QueryMsg,
    ReverseSimulationResponse, SimulationResponse,
};
use crate::oracle::{PriceResponse, PythQueryMsg};
use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitInfo,
    OracleInfo,
    PairInfo,
    PoolFeeState,
    PoolInfo,
    PoolSpecs,
    ThresholdPayout,
    COMMITSTATUS,
    COMMIT_CONFIG,
    COMMIT_LEDGER,
    FEEINFO,
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
    Response, StdError, StdResult, Storage, SubMsgResult, Uint128, Uint256, WasmMsg,
};
use cw2::{get_contract_version, set_contract_version};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use cw721_base::ExecuteMsg as CW721BaseExecuteMsg;
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
    _info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    //++++++++++++++++++++++++++++++++++++++++++++++need to check if the asset info is valid++++++++++++++++++++++++++++++++++++++++++++++++++++++++
    msg.asset_infos[0].check(deps.api)?;
    msg.asset_infos[1].check(deps.api)?;

    if msg.asset_infos[0] == msg.asset_infos[1] {
        return Err(ContractError::DoublingAssets {});
    }

    if (msg.fee_info.bluechip_fee + msg.fee_info.creator_fee) > Decimal::one() {
        return Err(ContractError::InvalidFee {});
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let pool_params: PoolInitParams = match msg.init_params {
        Some(data) => from_json(&data)?,
        None => return Err(StdError::generic_err("Missing init_params").into()),
    };

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
        liquidity: Decimal::zero(),
        owner: Addr::unchecked(""),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
    };

    let pool_specs = PoolSpecs {
        subscription_period: 2592000,   // 30 days in seconds
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayout {
        creator_amount: pool_params.creator_amount,
        bluechip_amount: pool_params.bluechip_amount,
        pool_amount: pool_params.pool_amount,
        commit_amount: pool_params.commit_amount,
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

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::UpdateConfig { .. } => Err(ContractError::NonSupported {}),

        ExecuteMsg::Commit { asset, amount } => commit(deps, env, info, asset, amount),
        // ── standard swap via native coin ──────────────────
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            to,
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
            )
        }

        ExecuteMsg::Receive(cw20_msg) => receive_cw20(deps, env, info, cw20_msg),

        // ── NEW: NFT-based liquidity management ──────────────────
        ExecuteMsg::DepositLiquidity { amount0, amount1 } => {
            // Check threshold requirement (same as swap)
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            execute_deposit_liquidity(deps, env, info, sender, amount0, amount1)
        }

        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
        } => {
            // Check threshold requirement
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            execute_add_to_position(deps, env, info, sender, position_id, amount0, amount1)
        }

        ExecuteMsg::CollectFees { position_id } => {
            execute_collect_fees(deps, env, info, position_id)
        }

        ExecuteMsg::RemovePartialLiquidity {
            position_id,
            liquidity_to_remove,
        } => execute_remove_partial_liquidity(deps, env, info, position_id, liquidity_to_remove),

        ExecuteMsg::RemoveLiquidity { position_id } => {
            execute_remove_liquidity(deps, env, info, position_id)
        }
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
        } => execute_remove_partial_liquidity_by_percent(deps, env, info, position_id, percentage),

        ExecuteMsg::WithdrawPosition {
            position_id,
            liquidity: _,
        } => execute_remove_liquidity(deps, env, info, position_id),

        ExecuteMsg::ReplaceAllPaymentTiers { new_payment_tiers } => {
            execute_replace_all_payment_tiers(deps, env, info, new_payment_tiers)
        }

        ExecuteMsg::AddPaymentTiers { tiers_to_add } => {
            execute_add_payment_tiers(deps, env, info, tiers_to_add)
        }

        ExecuteMsg::RemovePaymentTiers { tiers_to_remove } => {
            execute_remove_payment_tiers(deps, env, info, tiers_to_remove)
        }

        ExecuteMsg::ReplaceAllUsdPaymentTiers {
            new_payment_tiers_usd,
        } => execute_replace_all_usd_payment_tiers(deps, env, info, new_payment_tiers_usd),

        ExecuteMsg::AddUsdPaymentTiers { tiers_to_add_usd } => {
            execute_add_usd_payment_tiers(deps, env, info, tiers_to_add_usd)
        }

        ExecuteMsg::RemoveUsdPaymentTiers {
            tiers_to_remove_usd,
        } => execute_remove_usd_payment_tiers(deps, env, info, tiers_to_remove_usd),
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
pub fn receive_cw20(
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
            )
        }
        Ok(Cw20HookMsg::DepositLiquidity { amount0 }) => execute_deposit_liquidity(
            deps,
            env,
            info,
            Addr::unchecked(cw20_msg.sender),
            amount0,
            cw20_msg.amount,
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
        ),
        Err(err) => Err(ContractError::Std(err)),
    }
}

/// ## Description
/// Returns the amount of pool assets that correspond to an amount of LP tokens.
/// ## Params
/// * **pools** are an array of [`Asset`] type items. These are the assets in the pool.
///
/// * **amount** is an object of type [`Uint128`]. This is the amount of LP tokens to compute a corresponding amount of assets for.
///
/// * **total_share** is an object of type [`Uint128`]. This is the total amount of LP tokens currently minted.
pub fn get_share_in_assets(
    pools: &[Asset; 2],
    amount: Uint128,
    total_share: Uint128,
) -> Vec<Asset> {
    let mut share_ratio = Decimal::zero();
    if !total_share.is_zero() {
        share_ratio = Decimal::from_ratio(amount, total_share);
    }

    pools
        .iter()
        .map(|a| Asset {
            info: a.info.clone(),
            amount: a.amount * share_ratio.numerator() / share_ratio.denominator(),
        })
        .collect()
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
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    // Check if the offer asset is valid
    // Query current pool balances
    let pools: Vec<Asset> = pool_info
        .pair_info
        .query_pools(&deps.querier, env.contract.address.clone())?
        .iter()
        .map(|p| {
            let mut p = p.clone();
            if p.info.equal(&offer_asset.info) {
                p.amount = p
                    .amount
                    .checked_sub(offer_asset.amount)
                    .map_err(|_| ContractError::ShortOfThreshold {})?;
            }
            Ok::<_, ContractError>(p)
        })
        .collect::<Result<_, _>>()?;

    let (offer_pool, ask_pool) = if offer_asset.info.equal(&pools[0].info) {
        (pools[0].clone(), pools[1].clone())
    } else if offer_asset.info.equal(&pools[1].info) {
        (pools[1].clone(), pools[0].clone())
    } else {
        return Err(ContractError::AssetMismatch {});
    };

    let commission_rate = pool_specs.lp_fee; // From POOL_PARAMS
    let offer_amount = offer_asset.amount;

    let (return_amt, spread_amt, commission_amt) = compute_swap(
        offer_pool.amount,
        ask_pool.amount,
        offer_amount,
        commission_rate,
    )?;

    // Guard for slippage
    assert_max_spread(
        belief_price,
        max_spread,
        offer_amount,
        return_amt + commission_amt,
        spread_amt,
    )?;

    // Update fee growth
    if !pool_state.total_liquidity.is_zero() {
        if offer_asset.info.equal(&pools[0].info) {
            // Token0 offered, fees collected in token1
            pool_fee_state.fee_growth_global_1 +=
                Decimal::from_ratio(commission_amt, pool_state.total_liquidity);
            pool_fee_state.total_fees_collected_1 += commission_amt;
        } else {
            // Token1 offered, fees collected in token0
            pool_fee_state.fee_growth_global_0 +=
                Decimal::from_ratio(commission_amt, pool_state.total_liquidity);
            pool_fee_state.total_fees_collected_0 += commission_amt;
        }
    }

    // Update price accumulator if needed
    if let Some((p0_new, p1_new, block_time)) =
        accumulate_prices(env, &pool_state, pools[0].amount, pools[1].amount)?
    {
        pool_state.price0_cumulative_last = p0_new;
        pool_state.price1_cumulative_last = p1_new;
        pool_state.block_time_last = block_time;
    }

    // Save updated pool state
    POOL_STATE.save(deps.storage, &pool_state)?;

    // Prepare return message
    let mut msgs = vec![];
    if !return_amt.is_zero() {
        let return_asset = Asset {
            info: ask_pool.info.clone(),
            amount: return_amt,
        };
        msgs.push(return_asset.into_msg(&deps.querier, to.unwrap_or_else(|| sender.clone()))?);
    }

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "swap")
        .add_attribute("sender", sender)
        .add_attribute("offer_asset", offer_asset.info.to_string())
        .add_attribute("ask_asset", ask_pool.info.to_string())
        .add_attribute("offer_amount", offer_amount.to_string())
        .add_attribute("return_amount", return_amt.to_string())
        .add_attribute("spread_amount", spread_amt.to_string())
        .add_attribute("commission_amount_retained", commission_amt.to_string()))
}

#[allow(clippy::too_many_arguments)]

pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: Asset,
    amount: Uint128,
) -> Result<Response, ContractError> {
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
    let result = execute_commit_logic(&mut deps, env, info, asset, amount);

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
            let mut payment_valid = false;
            let mut payment_type = "unknown";
            let mut matched_tier = Uint128::zero();
            let mut usd_value = Uint128::zero();

            // First check native tiers (exact match, no oracle call needed)
            if commit_config.available_payment.contains(&asset.amount) {
                payment_valid = true;
                payment_type = "native";
                matched_tier = asset.amount;
                // Calculate USD for tracking/recording purposes
                usd_value = native_to_usd(
                    &deps.querier,
                    &oracle_info.oracle_addr,
                    &oracle_info.oracle_symbol,
                    asset.amount,
                )?;
            } else {
                // Not a native tier, so convert to USD and check USD tiers
                usd_value = native_to_usd(
                    &deps.querier,
                    &oracle_info.oracle_addr,
                    &oracle_info.oracle_symbol,
                    asset.amount,
                )?;

                // Check each USD tier with tolerance
                for &tier in commit_config.available_payment_usd.iter() {
                    if is_within_tolerance(usd_value, tier, pool_specs.usd_payment_tolerance_bps) {
                        payment_valid = true;
                        payment_type = "usd";
                        matched_tier = tier; // Store the tier they matched
                        break;
                    }
                }
            }

            // If payment doesn't match any tier, return detailed error
            if !payment_valid {
                let native_tiers: Vec<String> = commit_config
                    .available_payment
                    .iter()
                    .map(|tier| format!("{} {}", tier.u128() as f64 / 1_000_000.0, denom))
                    .collect();

                let tolerance_pct = pool_specs.usd_payment_tolerance_bps as f64 / 100.0;
                let usd_tiers: Vec<String> = commit_config
                    .available_payment_usd
                    .iter()
                    .map(|tier| {
                        format!(
                            "${:.2} (±{:.1}%)",
                            tier.u128() as f64 / 1_000_000.0,
                            tolerance_pct
                        )
                    })
                    .collect();

                return Err(ContractError::InvalidPaymentAmount {
                    sent_native: format!("{} {}", asset.amount.u128() as f64 / 1_000_000.0, denom),
                    sent_usd: format!("{:.2}", usd_value.u128() as f64 / 1_000_000.0),
                    available_native: native_tiers,
                    available_usd: usd_tiers,
                });
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

            // Handle pre-threshold funding phase
            if !THRESHOLD_HIT.load(deps.storage)? {
                // Update commit ledger
                COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                    Ok(v.unwrap_or_default() + usd_value)
                })?;

                // Update total USD raised
                let usd_total =
                    USD_RAISED.update::<_, ContractError>(deps.storage, |r| Ok(r + usd_value))?;
                COMMITSTATUS.save(deps.storage, &usd_total)?;

                // Check for threshold crossing
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

                // Return early for pre-threshold commits
                return Ok(Response::new()
                    .add_messages(messages)
                    .add_attribute("action", "subscribe")
                    .add_attribute("phase", "funding")
                    .add_attribute("subscriber", sender)
                    .add_attribute("payment_type", payment_type)
                    .add_attribute("matched_tier", matched_tier.to_string())
                    .add_attribute("commit_amount_native", asset.amount.to_string())
                    .add_attribute("commit_amount_usd", usd_value.to_string()));
            }

            // Post-threshold: handle swap for subscription
            let net_amount = asset
                .amount
                .checked_sub(bluechip_fee_amt + creator_fee_amt)?;

            // Load current pool state
            let pools = pool_info
                .pair_info
                .query_pools(&deps.querier, env.contract.address.clone())?;

            // Determine which pool is native and which is CW20
            let (offer_pool, ask_pool) = if pools[0].info.is_native_token() {
                (pools[0].clone(), pools[1].clone())
            } else {
                (pools[1].clone(), pools[0].clone())
            };

            // Calculate swap output
            let (return_amt, _spread_amt, commission_amt) = compute_swap(
                offer_pool.amount,
                ask_pool.amount,
                net_amount,
                pool_specs.lp_fee,
            )?;

            // UPDATE FEE GROWTH
            if !pool_state.total_liquidity.is_zero() && !commission_amt.is_zero() {
                POOL_FEE_STATE.update(
                    deps.storage,
                    |mut fee_state| -> Result<_, ContractError> {
                        fee_state.fee_growth_global_1 +=
                            Decimal::from_ratio(commission_amt, pool_state.total_liquidity);
                        fee_state.total_fees_collected_1 += commission_amt;
                        Ok(fee_state)
                    },
                )?;
            }

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

            // Update price accumulator if needed
            if let Some((p0_new, p1_new, block_time)) =
                accumulate_prices(env.clone(), &pool_state, pools[0].amount, pools[1].amount)?
            {
                POOL_STATE.update(deps.storage, |mut state| -> Result<_, ContractError> {
                    state.price0_cumulative_last = p0_new;
                    state.price1_cumulative_last = p1_new;
                    state.block_time_last = block_time;
                    Ok(state)
                })?;
            }

            // Record/extend subscription
            let new_expiry = env.block.time.plus_seconds(pool_specs.subscription_period);
            SUB_INFO.save(
                deps.storage,
                &sender,
                &Subscription {
                    expires: new_expiry,
                    total_paid: asset.amount,
                    total_paid_usd: usd_value, // Track USD value for subscription
                },
            )?;

            // Return response for post-threshold
            Ok(Response::new()
                .add_messages(messages)
                .add_attribute("action", "subscribe")
                .add_attribute("phase", "active")
                .add_attribute("subscriber", sender)
                .add_attribute("payment_type", payment_type)
                .add_attribute("matched_tier", matched_tier.to_string())
                .add_attribute("commit_amount_native", asset.amount.to_string())
                .add_attribute("commit_amount_usd", usd_value.to_string())
                .add_attribute("tokens_received", return_amt.to_string()))
        }
        _ => Err(ContractError::AssetMismatch {}),
    }
}
#[allow(dead_code)]
fn get_token_amount(native_amount: Uint128) -> Uint128 {
    // example: 1 native unit (10^6) → 20 creator tokens
    const TOKENS_PER_NATIVE: u128 = 20;
    Uint128::from(native_amount.u128() * TOKENS_PER_NATIVE)
}

fn native_to_usd(
    querier: &QuerierWrapper,
    oracle_addr: &Addr,
    symbol: &str,
    native_amount: Uint128, // micro-native
) -> StdResult<Uint128> {
    // 1. query oracle
    let resp: PriceResponse = querier
        .query_wasm_smart(
            oracle_addr.clone(),
            &PythQueryMsg::GetPrice {
                price_id: symbol.into(),
            },
        )
        .map_err(|e| StdError::generic_err(format!("Oracle query failed: {}", e)))?;
    let price_8dec = resp.price; // e.g. 1.25 USD = 125_000_000

    // 2. convert: (µnative × price) / 10^(8-6) = µUSD
    // pool/src/contract.rs  – helper native_to_usd
    let usd_micro_u256 =
        (Uint256::from(native_amount) * Uint256::from(price_8dec)) / Uint256::from(100_000_000u128); // 10^(8-6) = 100

    let usd_micro = Uint128::try_from(usd_micro_u256)?;
    Ok(usd_micro)
}

pub fn usd_to_native(
    querier: &QuerierWrapper,
    oracle_addr: &Addr,
    symbol: &str,
    usd_amount: Uint128, // micro-USD (6 decimals)
) -> StdResult<Uint128> {
    // 1. Query oracle - same as native_to_usd
    let resp: PriceResponse = querier
        .query_wasm_smart(
            oracle_addr.clone(),
            &PythQueryMsg::GetPrice {
                price_id: symbol.into(),
            },
        )
        .map_err(|e| StdError::generic_err(format!("Oracle query failed: {}", e)))?;
    let price_8dec = resp.price; // e.g. 1.25 USD = 125_000_000

    // 2. Reverse conversion: µnative = (µUSD × 10^2) / price
    // If native_to_usd: µUSD = (µnative × price) / 100
    // Then usd_to_native: µnative = (µUSD × 100) / price

    let native_micro_u256 =
        (Uint256::from(usd_amount) * Uint256::from(100u128)) / Uint256::from(price_8dec);

    let native_micro = Uint128::try_from(native_micro_u256)
        .map_err(|_| StdError::generic_err("Overflow in USD to native conversion"))?;

    Ok(native_micro)
}
//deposit liquidity in pool
pub fn execute_deposit_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    user: Addr,
    amount0: Uint128, // native amount
    amount1: Uint128, // CW20 amount
) -> Result<Response, ContractError> {
    // 1. Validate the native deposit (token0)
    const NATIVE_DENOM: &str = "stake";
    let paid_native = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    if paid_native != amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }

    // 2. Load the pool and update fee tracking
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // 3. Transfer CW20 tokens from user to pool (if amount1 > 0)
    let mut messages = vec![];

    if !amount1.is_zero() {
        let transfer_cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                owner: info.sender.to_string(), // Transfer from the sender
                recipient: env.contract.address.to_string(), // To the pool
                amount: amount1,
            })?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(transfer_cw20_msg));
    }
    if !pool_state.nft_ownership_accepted {
        let accept_msg = WasmMsg::Execute {
            contract_addr: pool_info.position_nft_address.to_string(),
            msg: to_json_binary(&cw721_base::ExecuteMsg::<Empty, Empty>::UpdateOwnership(
                cw721_base::Action::AcceptOwnership {},
            ))?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(accept_msg)); // Add to messages
        pool_state.nft_ownership_accepted = true;
        // Don't return here! Continue with the deposit
    }
    // 4. Compute liquidity amount
    let liquidity = calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;

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

    // 8. Create and store the position with current fee growth
    let position = Position {
        liquidity,
        owner: user.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;

    // 9. Update pool state
    pool_state.total_liquidity += Uint128::from(liquidity.atomics());
    POOL_STATE.save(deps.storage, &pool_state)?;

    Ok(Response::new()
        .add_messages(messages) // Add all messages
        .add_attribute("action", "deposit_liquidity")
        .add_attribute("position_id", position_id)
        .add_attribute("depositor", user)
        .add_attribute("liquidity", liquidity.to_string()))
}

pub fn execute_collect_fees(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    // 1. Load config
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_Info = POOL_INFO.load(deps.storage)?;
    // 2. Verify NFT ownership through external NFT contract
    verify_position_ownership(
        deps.as_ref(),
        &pool_Info.position_nft_address,
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
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
    );

    // 5. Update position's fee growth tracking
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &liquidity_position)?;

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
            contract_addr: pool_Info.token_address.to_string(), // Using config.token_address
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
) -> Result<Response, ContractError> {
    // 1. Validate the native deposit (token0)
    const NATIVE_DENOM: &str = "stake";
    let paid_native = info
        .funds
        .iter()
        .find(|c| c.denom == NATIVE_DENOM)
        .map(|c| c.amount)
        .unwrap_or_default();

    if paid_native != amount0 {
        return Err(ContractError::InvalidNativeAmount {});
    }

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

    // 4. Load position
    let mut liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    let mut messages: Vec<CosmosMsg> = vec![];
    if !amount1.is_zero() {
        let transfer_cw20_msg = WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::TransferFrom {
                owner: info.sender.to_string(),
                recipient: env.contract.address.to_string(),
                amount: amount1,
            })?,
            funds: vec![],
        };
        messages.push(CosmosMsg::Wasm(transfer_cw20_msg));
    }

    // 5. Calculate any pending fees FIRST (before diluting the position)
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
    );

    // 6. Calculate new liquidity for the additional deposit
    let additional_liquidity = calc_liquidity_for_deposit(deps.as_ref(), amount0, amount1)?;

    // 7. Update position with new totals and reset fee tracking
    liquidity_position.liquidity += additional_liquidity;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

    // 8. Update config state (just total liquidity)
    pool_state.total_liquidity += Uint128::from(additional_liquidity.atomics());
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
        .add_attribute("amount0_added", amount0)
        .add_attribute("amount1_added", amount1)
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
) -> Result<Response, ContractError> {
    // 1. Load config
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut pool_state = POOL_STATE.load(deps.storage)?;

    // 2. Load and validate position
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    // 3. Verify NFT ownership through external NFT contract
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;

    // 4. Get current pool reserves
    let pools = pool_info
        .pair_info
        .query_pools(&deps.querier, env.contract.address.clone())?;
    let current_reserve0 = pools[0].amount;
    let current_reserve1 = pools[1].amount;

    // 5. Calculate user's share of the pool (using your decimal logic)
    let pool_reserve0_decimal = Decimal::from_atomics(current_reserve0, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;
    let pool_reserve1_decimal = Decimal::from_atomics(current_reserve1, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;
    let pool_total_liquidity_decimal = Decimal::from_atomics(pool_state.total_liquidity, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;

    let user_share_0_decimal =
        (liquidity_position.liquidity * pool_reserve0_decimal) / pool_total_liquidity_decimal;
    let user_share_1_decimal =
        (liquidity_position.liquidity * pool_reserve1_decimal) / pool_total_liquidity_decimal;

    // Convert back to Uint128 for token transfers
    let user_share_0 = Uint128::from(user_share_0_decimal.atomics());
    let user_share_1 = Uint128::from(user_share_1_decimal.atomics());

    // 6. Calculate any remaining fees owed
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
    );

    // 7. Total amounts to send (principal + fees)
    let total_amount_0 = user_share_0 + fees_owed_0;
    let total_amount_1 = user_share_1 + fees_owed_1;

    // 8. Update config state (total liquidity)
    let liquidity_to_subtract = Uint128::from(liquidity_position.liquidity.atomics());
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_subtract)?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    // Note: Pool reserves will be automatically updated when tokens are transferred out

    // 9. Burn the NFT (on external NFT contract)
    let burn_msg = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(), // External NFT contract
        msg: to_json_binary(&cw721::Cw721ExecuteMsg::Burn {
            token_id: position_id.clone(),
        })?,
        funds: vec![],
    };

    // 10. Remove position from storage
    LIQUIDITY_POSITIONS.remove(deps.storage, &position_id);

    // 11. Prepare response with token transfers
    let mut response = Response::new()
        .add_message(burn_msg)
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
    liquidity_to_remove: Decimal, // Specific amount of liquidity to remove
) -> Result<Response, ContractError> {
    // 1. Load config

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

    if liquidity_to_remove >= liquidity_position.liquidity {
        return Err(ContractError::InvalidAmount {});
    }

    // 5. Get current pool reserves
    let pools = pool_info
        .pair_info
        .query_pools(&deps.querier, env.contract.address.clone())?;
    let current_reserve0 = pools[0].amount;
    let current_reserve1 = pools[1].amount;

    // 6. Calculate ALL pending fees first (before any changes)
    let fees_owed_0 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_0,
        liquidity_position.fee_growth_inside_0_last,
    );

    let fees_owed_1 = calculate_fees_owed(
        liquidity_position.liquidity,
        pool_fee_state.fee_growth_global_1,
        liquidity_position.fee_growth_inside_1_last,
    );

    // 7. Calculate partial withdrawal amounts (principal only, not fees)
    let pool_reserve0_decimal = Decimal::from_atomics(current_reserve0, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;
    let pool_reserve1_decimal = Decimal::from_atomics(current_reserve1, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;
    let pool_total_liquidity_decimal = Decimal::from_atomics(pool_state.total_liquidity, 0)
        .map_err(|_| ContractError::InsufficientLiquidity {})?;

    let withdrawal_amount_0_decimal =
        (liquidity_to_remove * pool_reserve0_decimal) / pool_total_liquidity_decimal;
    let withdrawal_amount_1_decimal =
        (liquidity_to_remove * pool_reserve1_decimal) / pool_total_liquidity_decimal;

    let withdrawal_amount_0 = Uint128::from(withdrawal_amount_0_decimal.atomics());
    let withdrawal_amount_1 = Uint128::from(withdrawal_amount_1_decimal.atomics());

    // 8. Total amounts to send (partial principal + all accumulated fees)
    let total_amount_0 = withdrawal_amount_0 + fees_owed_0;
    let total_amount_1 = withdrawal_amount_1 + fees_owed_1;

    // 9. Update config state (remove the liquidity being withdrawn)
    let liquidity_to_remove_uint = Uint128::from(liquidity_to_remove.atomics());
    pool_state.total_liquidity = pool_state
        .total_liquidity
        .checked_sub(liquidity_to_remove_uint)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    // 10. Update position - reduce liquidity and reset fee tracking
    liquidity_position.liquidity = liquidity_position
        .liquidity
        .checked_sub(liquidity_to_remove)?;
    liquidity_position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    liquidity_position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    liquidity_position.last_fee_collection = env.block.time.seconds();

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

    // 12. Send native token (token0) - partial principal + fees
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

    // 13. Send CW20 token (token1) - partial principal + fees
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

pub fn execute_remove_partial_liquidity_by_percent(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
    percentage: u64,
) -> Result<Response, ContractError> {
    // Validate percentage
    if percentage == 0 || percentage >= 100 {
        return Err(ContractError::InvalidAmount {});
    }

    // Load position to calculate absolute amount
    let liquidity_position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;

    // Calculate liquidity amount to remove using proper Decimal math
    let percentage_decimal = Decimal::from_ratio(percentage, 100u128);
    let liquidity_to_remove = liquidity_position.liquidity * percentage_decimal;

    // Call the main partial removal function
    execute_remove_partial_liquidity(deps, env, info, position_id, liquidity_to_remove)
}

// Helper function to calculate liquidity for deposits
fn calc_liquidity_for_deposit(
    deps: Deps,
    amount0: Uint128,
    amount1: Uint128,
) -> Result<Decimal, ContractError> {
    // Changed return type to Decimal
    let pool_state = POOL_STATE.load(deps.storage)?;
    // If this is the first deposit (empty pool), use geometric mean
    if pool_state.total_liquidity.is_zero() {
        // First liquidity provider gets sqrt(amount0 * amount1)
        // This is the standard AMM approach (like Uniswap V2)
        let product = amount0.checked_mul(amount1)?;
        let liquidity_uint = integer_sqrt(product);

        // Ensure minimum liquidity (prevent division by zero issues)
        if liquidity_uint < Uint128::from(1000u128) {
            return Err(ContractError::InsufficientLiquidity {});
        }

        // Convert to Decimal
        Ok(Decimal::from_atomics(liquidity_uint, 0)
            .map_err(|_| ContractError::InsufficientLiquidity {})?)
    } else {
        // Subsequent deposits: maintain proportional share
        // liquidity = min(amount0/reserve0, amount1/reserve1) * total_liquidity

        if pool_state.reserve0.is_zero() || pool_state.reserve1.is_zero() {
            return Err(ContractError::InsufficientLiquidity {});
        }

        if amount0.is_zero() || amount1.is_zero() {
            return Err(ContractError::InsufficientLiquidity {});
        }

        let optimal_amount1_for_amount0 = (amount0 * pool_state.reserve1) / pool_state.reserve0; // "If I use all of amount0, how much amount1 do I need?"
        let optimal_amount0_for_amount1 = (amount1 * pool_state.reserve0) / pool_state.reserve1; // "If I use all of amount1, how much amount0 do I need?"

        let (final_amount0, final_amount1) = if optimal_amount1_for_amount0 <= amount1 {
            // User provided enough amount1, use all of amount0
            (amount0, optimal_amount1_for_amount0)
        } else {
            // User didn't provide enough amount1, use all of amount1
            (optimal_amount0_for_amount1, amount1)
        };

        // Sanity check the final amounts
        if final_amount0.is_zero() || final_amount1.is_zero() {
            return Err(ContractError::InsufficientLiquidity {});
        }

        // Calculate liquidity with the adjusted amounts
        let liquidity_uint = (final_amount0 * pool_state.total_liquidity) / pool_state.reserve0;

        if liquidity_uint.is_zero() {
            return Err(ContractError::InsufficientLiquidity {});
        }

        // Convert to Decimal
        Ok(Decimal::from_atomics(liquidity_uint, 0)
            .map_err(|_| ContractError::InsufficientLiquidity {})?)
    }
}

// Helper function for integer square root (for first deposit)
fn integer_sqrt(value: Uint128) -> Uint128 {
    if value.is_zero() {
        return Uint128::zero();
    }

    let mut x = value;
    let mut y = (value + Uint128::one()) / Uint128::from(2u128);

    while y < x {
        x = y;
        y = (x + value / x) / Uint128::from(2u128);
    }

    x
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
/// Calculates the amount of fees the Maker contract gets according to specified pair parameters.
/// Returns a [`None`] if the Maker fee is zero, otherwise returns a [`Asset`] struct with the specified attributes.
/// ## Params
/// * **pool_info** is an object of type [`AssetInfo`]. Contains information about the pool asset for which the commission will be calculated.
///
/// * **commission_amount** is an object of type [`Env`]. This is the total amount of fees charged for a swap.
///
/// * **maker_commission_rate** is an object of type [`MessageInfo`]. This is the percentage of fees that go to the Maker contract.
#[allow(non_snake_case)]
pub fn calculate_maker_fee(
    pool_info: AssetInfo,
    commission_amount: Uint128,
    maker_commission_rate: Decimal,
) -> Option<Asset> {
    let maker_fee: Uint128 =
        commission_amount * maker_commission_rate.numerator() / maker_commission_rate.denominator();
    if maker_fee.is_zero() {
        return None;
    }

    Some(Asset {
        info: pool_info,
        amount: maker_fee,
    })
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::PaymentInfo {} => to_json_binary(&query_payment_info(deps)?),
        QueryMsg::CreatorTierInfo {} => to_json_binary(&query_payment_tiers_with_tolerance(deps)?),
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::Pool {} => to_json_binary(&query_pool(deps)?),
        QueryMsg::Simulation { offer_asset } => {
            to_json_binary(&query_simulation(deps, offer_asset)?)
        }
        QueryMsg::ReverseSimulation { ask_asset } => {
            to_json_binary(&query_reverse_simulation(deps, ask_asset)?)
        }
        QueryMsg::CumulativePrices {} => to_json_binary(&query_cumulative_prices(deps, env)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),
        QueryMsg::IsFullyCommited {} => to_json_binary(&query_check_threshold_limit(deps)?),
        QueryMsg::IsSubscribed { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let active = SUB_INFO
                .may_load(deps.storage, &addr)?
                .map_or(false, |sub| sub.expires > env.block.time);
            to_json_binary(&active)
        }
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
pub fn query_payment_info(deps: Deps) -> StdResult<PaymentInfoResponse> {
    let fee_info = FEEINFO.load(deps.storage)?;
    let commit_config = COMMIT_CONFIG.load(deps.storage)?;
    Ok(PaymentInfoResponse {
        creator: fee_info.creator_address,
        available_payment_tiers: commit_config.available_payment,
    })
}

pub fn query_payment_tiers_with_tolerance(
    deps: Deps,
) -> StdResult<PaymentTiersResponseWithTolerance> {
    let commit_config = COMMIT_CONFIG.load(deps.storage)?;
    let pool_specs = POOL_SPECS.load(deps.storage)?;
    let oracle_info = ORACLE_INFO.load(deps.storage)?;
    // Native tiers remain the same (no tolerance)
    let native_tiers: Vec<NativeTierInfo> = commit_config
        .available_payment
        .iter()
        .map(|&native_amount| {
            let usd_value = native_to_usd(
                &deps.querier,
                &oracle_info.oracle_addr,
                &oracle_info.oracle_symbol,
                native_amount,
            )
            .unwrap_or_default();

            NativeTierInfo {
                native_amount,
                current_usd_value: usd_value,
            }
        })
        .collect();

    // USD tiers now show tolerance ranges
    let usd_tiers: Vec<USDTierInfoWithTolerance> = commit_config
        .available_payment_usd
        .iter()
        .map(|&usd_amount| {
            // Calculate USD tolerance range
            let min_usd = usd_amount.multiply_ratio(
                10000u128 - pool_specs.usd_payment_tolerance_bps as u128,
                10000u128,
            );
            let max_usd = usd_amount.multiply_ratio(
                10000u128 + pool_specs.usd_payment_tolerance_bps as u128,
                10000u128,
            );

            // Convert to native amounts
            let native_exact = usd_to_native(
                &deps.querier,
                &oracle_info.oracle_addr,
                &oracle_info.oracle_symbol,
                usd_amount,
            )
            .unwrap_or_default();

            let native_min = usd_to_native(
                &deps.querier,
                &oracle_info.oracle_addr,
                &oracle_info.oracle_symbol,
                min_usd,
            )
            .unwrap_or_default();

            let native_max = usd_to_native(
                &deps.querier,
                &oracle_info.oracle_addr,
                &oracle_info.oracle_symbol,
                max_usd,
            )
            .unwrap_or_default();

            USDTierInfoWithTolerance {
                usd_amount,
                tolerance_bps: pool_specs.usd_payment_tolerance_bps,
                min_usd_accepted: min_usd,
                max_usd_accepted: max_usd,
                current_native_required: native_exact,
                min_native_accepted: native_min,
                max_native_accepted: native_max,
            }
        })
        .collect();

    // Return the correct type!
    Ok(PaymentTiersResponseWithTolerance {
        native_tiers,
        usd_tiers,
    })
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

/// ## Description
/// Returns an amount of coins. For each coin in the specified vector, if the coin is null, we return `Uint128::zero()`,
/// otherwise we return the specified coin amount.
/// ## Params
/// * **coins** is an array of [`Coin`] type items. This is a list of coins for which we return amounts.
///
/// * **denom** is an object of type [`String`]. This is the denomination used for the coins.
pub fn amount_of(coins: &[Coin], denom: String) -> Uint128 {
    match coins.iter().find(|x| x.denom == denom) {
        Some(coin) => coin.amount,
        None => Uint128::zero(),
    }
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

    // The commision (minus the part that goes to the Maker contract) will be absorbed by the pool
    let return_amount: Uint256 = return_amount - commission_amount;
    Ok((
        return_amount.try_into()?,
        spread_amount.try_into()?,
        commission_amount.try_into()?,
    ))
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

fn trigger_threshold_payout(
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
    let native_seed = Uint128::new(2350); // Corrected comment
    msgs.push(get_bank_transfer_to_msg(
        &env.contract.address,
        denom,
        native_seed,
    )?);

    // 5. Initialize the pool state in CONFIG (instead of creating a new POOLS entry)
    // Note: The actual reserves will be tracked by token balances
    // We're just initializing the fee tracking and liquidity state
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
/// This is an internal function that enforces slippage tolerance for swaps.
/// Returns a [`ContractError`] on failure, otherwise returns [`Ok`].
/// ## Params
/// * **slippage_tolerance** is an object of type [`Option<Decimal>`]. This is the slippage tolerance to enforce.
///
/// * **deposits** are an array of [`Uint128`] type items. These are offer and ask amounts for a swap.
///
/// * **pools** are an array of [`Asset`] type items. These are total amounts of assets in the pool.
#[allow(dead_code)]
fn assert_slippage_tolerance(
    slippage_tolerance: Option<Decimal>,
    deposits: &[Uint128; 2],
    pools: &[Asset; 2],
) -> Result<(), ContractError> {
    let default_slippage = Decimal::from_str(DEFAULT_SLIPPAGE)?;
    let max_allowed_slippage = Decimal::from_str(MAX_ALLOWED_SLIPPAGE)?;

    let slippage_tolerance = slippage_tolerance.unwrap_or(default_slippage);
    if slippage_tolerance.gt(&max_allowed_slippage) {
        return Err(ContractError::AllowedSpreadAssertion {});
    }

    let slippage_tolerance: Decimal256 = decimal2decimal256(slippage_tolerance)?;
    let one_minus_slippage_tolerance = Decimal256::one() - slippage_tolerance;
    let deposits: [Uint256; 2] = [deposits[0].into(), deposits[1].into()];
    let pools: [Uint256; 2] = [pools[0].amount.into(), pools[1].amount.into()];

    // Ensure each price does not change more than what the slippage tolerance allows
    if Decimal256::from_ratio(deposits[0], deposits[1]) * one_minus_slippage_tolerance
        > Decimal256::from_ratio(pools[0], pools[1])
        || Decimal256::from_ratio(deposits[1], deposits[0]) * one_minus_slippage_tolerance
            > Decimal256::from_ratio(pools[1], pools[0])
    {
        return Err(ContractError::MaxSlippageAssertion {});
    }

    Ok(())
}

pub fn update_fee_growth(
    pool: &mut PoolFeeState,
    fee_amount_0: Uint128,
    fee_amount_1: Uint128,
    total_liquidity: Uint128,
) {
    if !total_liquidity.is_zero() {
        let decimal_scale = 9;
        // Add fees to global tracking (fees per unit of liquidity)
        let fee_per_liquidity_0 =
            (fee_amount_0 * Uint128::from(1_000_000_000u128)) / total_liquidity;
        let fee_per_liquidity_1 =
            (fee_amount_1 * Uint128::from(1_000_000_000u128)) / total_liquidity;

        pool.fee_growth_global_0 += Decimal::from_atomics(fee_per_liquidity_0, decimal_scale)
            .unwrap_or_else(|_| Decimal::zero());
        pool.fee_growth_global_1 += Decimal::from_atomics(fee_per_liquidity_1, decimal_scale)
            .unwrap_or_else(|_| Decimal::zero());
        pool.total_fees_collected_0 += fee_amount_0;
        pool.total_fees_collected_1 += fee_amount_1;
    }
}
/// ## Description
/// Used for the contract migration. Returns a default object of type [`Response`].
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
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
#[allow(dead_code)]
fn validate_input_amount(
    actual_funds: &[Coin],
    amount: Uint128,
    denom: &String,
) -> Result<(), ContractError> {
    let actual = get_amount_for_denom(actual_funds, &denom);

    if amount == Uint128::zero() {
        return Err(ContractError::InsufficientFunds {});
    }
    if actual.amount != amount {
        return Err(ContractError::InsufficientFunds {});
    }
    if &actual.denom != denom {
        return Err(ContractError::IncorrectNativeDenom {
            provided: actual.denom,
            required: denom.to_string(),
        });
    }
    Ok(())
}
#[allow(dead_code)]
fn get_amount_for_denom(coins: &[Coin], denom: &str) -> Coin {
    let amount: Uint128 = coins
        .iter()
        .filter(|c| c.denom == denom)
        .map(|c| c.amount)
        .sum();
    Coin {
        amount,
        denom: denom.to_string(),
    }
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
    liquidity: Decimal, // Changed to accept Decimal
    fee_growth_global: Decimal,
    fee_growth_last: Decimal,
) -> Uint128 {
    if fee_growth_global >= fee_growth_last {
        let fee_growth_delta = fee_growth_global - fee_growth_last;
        // Convert liquidity to Uint128 for calculation
        let liquidity_uint = Uint128::from(liquidity.atomics());
        (liquidity_uint * fee_growth_delta) / Uint128::from(1_000_000_000u128)
    } else {
        Uint128::zero()
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

pub fn execute_replace_all_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    new_payment_tiers: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    // Validate payment tiers
    if new_payment_tiers.is_empty() {
        return Err(ContractError::InvalidPaymentTiers {});
    }

    // Check for duplicates
    let mut unique_tiers = new_payment_tiers.clone();
    unique_tiers.sort();
    unique_tiers.dedup();
    if unique_tiers.len() != new_payment_tiers.len() {
        return Err(ContractError::Std(StdError::generic_err(
            "Duplicate payment tiers not allowed",
        )));
    }

    // Update the config with new payment tiers
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        config.available_payment = new_payment_tiers.clone();
        Ok(config)
    })?;

    Ok(Response::new()
        .add_attribute("action", "update_replace_all_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("new_tiers", format!("{:?}", new_payment_tiers)))
}

// Add new payment tiers to existing ones
pub fn execute_add_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    tiers_to_add: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    if tiers_to_add.is_empty() {
        return Err(ContractError::Std(StdError::generic_err("No tiers to add")));
    }

    // Update the config
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        // Add new tiers
        config.available_payment.extend(tiers_to_add.clone());

        // Remove duplicates while preserving order
        config.available_payment.sort();
        config.available_payment.dedup();

        // Ensure we still have at least one tier
        if config.available_payment.is_empty() {
            return Err(ContractError::InvalidPaymentTiers {});
        }

        Ok(config)
    })?;

    Ok(Response::new()
        .add_attribute("action", "add_payment_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("added_tiers", format!("{:?}", tiers_to_add)))
}

// Remove specific payment tiers
pub fn execute_remove_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    tiers_to_remove: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    if tiers_to_remove.is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "No tiers to remove",
        )));
    }

    // Get initial count
    let initial_count = COMMIT_CONFIG.load(deps.storage)?.available_payment.len();

    // Update the config
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        // Remove specified tiers
        config
            .available_payment
            .retain(|tier| !tiers_to_remove.contains(tier));

        // Ensure we still have at least one tier
        if config.available_payment.is_empty() {
            return Err(ContractError::Std(StdError::generic_err(
                "Cannot remove all payment tiers - at least one must remain",
            )));
        }

        Ok(config)
    })?;

    // Calculate how many were actually removed
    let final_count = COMMIT_CONFIG.load(deps.storage)?.available_payment.len();
    let removed_count = initial_count - final_count;

    Ok(Response::new()
        .add_attribute("action", "remove_payment_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("removed_tiers", format!("{:?}", tiers_to_remove))
        .add_attribute("removed_count", removed_count.to_string()))
}

pub fn execute_replace_all_usd_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    new_payment_tiers_usd: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    // Validate payment tiers
    if new_payment_tiers_usd.is_empty() {
        return Err(ContractError::InvalidPaymentTiers {});
    }

    // Check for duplicates
    let mut unique_tiers = new_payment_tiers_usd.clone();
    unique_tiers.sort();
    unique_tiers.dedup();
    if unique_tiers.len() != new_payment_tiers_usd.len() {
        return Err(ContractError::Std(StdError::generic_err(
            "Duplicate payment tiers not allowed",
        )));
    }

    // Update the config with new payment tiers
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        config.available_payment = new_payment_tiers_usd.clone();
        Ok(config)
    })?;

    Ok(Response::new()
        .add_attribute("action", "update_replace_all_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("new_tiers", format!("{:?}", new_payment_tiers_usd)))
}

pub fn execute_add_usd_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    tiers_to_add_usd: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    if tiers_to_add_usd.is_empty() {
        return Err(ContractError::Std(StdError::generic_err("No tiers to add")));
    }

    // Update the config
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        // Add new tiers
        config
            .available_payment_usd
            .extend(tiers_to_add_usd.clone());

        // Remove duplicates while preserving order
        config.available_payment_usd.sort();
        config.available_payment_usd.dedup();

        // Ensure we still have at least one tier
        if config.available_payment_usd.is_empty() {
            return Err(ContractError::InvalidPaymentTiers {});
        }

        Ok(config)
    })?;

    Ok(Response::new()
        .add_attribute("action", "add_payment_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("added_tiers", format!("{:?}", tiers_to_add_usd)))
}

// Remove specific USD payment tiers
pub fn execute_remove_usd_payment_tiers(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    tiers_to_remove_usd: Vec<Uint128>,
) -> Result<Response, ContractError> {
    // Load fee info to get creator address
    let fee_info = FEEINFO.load(deps.storage)?;

    // Check if sender is the creator
    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    if tiers_to_remove_usd.is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "No tiers to remove",
        )));
    }

    // Get initial count
    let initial_count = COMMIT_CONFIG
        .load(deps.storage)?
        .available_payment_usd
        .len();

    // Update the config
    COMMIT_CONFIG.update(deps.storage, |mut config| -> Result<_, ContractError> {
        // Remove specified tiers
        config
            .available_payment_usd
            .retain(|tier| !tiers_to_remove_usd.contains(tier));

        Ok(config)
    })?;

    // Calculate how many were actually removed
    let final_count = COMMIT_CONFIG.load(deps.storage)?.available_payment.len();
    let removed_count = initial_count - final_count;

    Ok(Response::new()
        .add_attribute("action", "remove_payment_tiers")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("removed_tiers", format!("{:?}", tiers_to_remove_usd))
        .add_attribute("removed_count", removed_count.to_string()))
}

pub fn execute_update_usd_payment_tolerance(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    tolerance_bps: u16,
) -> Result<Response, ContractError> {
    let fee_info = FEEINFO.load(deps.storage)?;

    if info.sender != fee_info.creator_address {
        return Err(ContractError::UnauthorizedNotCreator {});
    }

    // Reasonable limits: 0% to 10% tolerance
    if tolerance_bps > 1000 {
        return Err(ContractError::Std(StdError::generic_err(
            "Tolerance cannot exceed 10% (1000 basis points)",
        )));
    }

    POOL_SPECS.update(deps.storage, |mut pool_specs| -> Result<_, ContractError> {
        pool_specs.usd_payment_tolerance_bps = tolerance_bps;
        Ok(pool_specs)
    })?;

    Ok(Response::new()
        .add_attribute("action", "update_usd_payment_tolerance")
        .add_attribute("creator", fee_info.creator_address)
        .add_attribute("tolerance_bps", tolerance_bps.to_string())
        .add_attribute(
            "tolerance_percent",
            format!("{:.2}%", tolerance_bps as f64 / 100.0),
        ))
}
fn is_within_tolerance(payment_usd: Uint128, tier_usd: Uint128, tolerance_bps: u16) -> bool {
    // Calculate tolerance range
    // For 1% tolerance (100 bps): multiplier is 10000 ± 100
    let lower_bound = tier_usd.multiply_ratio(10000u128 - tolerance_bps as u128, 10000u128);
    let upper_bound = tier_usd.multiply_ratio(10000u128 + tolerance_bps as u128, 10000u128);

    payment_usd >= lower_bound && payment_usd <= upper_bound
}
