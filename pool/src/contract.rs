#![allow(non_snake_case)]
use crate::asset::{PoolPairType, TokenInfo, TokenType};
use crate::error::ContractError;

use crate::generic_helpers::{
    check_rate_limit, enforce_transaction_deadline, get_bank_transfer_to_msg,
    process_distribution_batch, trigger_threshold_payout, update_pool_fee_growth,
    validate_pool_threshold_payments,
};
use crate::liquidity::{
    execute_add_to_position, execute_collect_fees, execute_deposit_liquidity,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent,
};
use crate::liquidity_helpers::execute_claim_creator_excess;
use crate::msg::{Cw20HookMsg, ExecuteMsg, MigrateMsg, PoolConfigUpdate, PoolInstantiateMsg};
use crate::query::query_check_commit;
// use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitLimitInfo, DistributionState, EmergencyWithdrawalInfo, ExpectedFactory, OracleInfo,
    PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, RecoveryType, ThresholdPayoutAmounts,
    COMMITFEEINFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
    DEFAULT_MAX_GAS_PER_TX, DISTRIBUTION_BOUNTY, DISTRIBUTION_STATE, EMERGENCY_WITHDRAWAL,
    EMERGENCY_WITHDRAW_DELAY_SECONDS, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT,
    MINIMUM_LIQUIDITY, NATIVE_RAISED_FROM_COMMIT, ORACLE_INFO, OWNER_POSITIONS, PENDING_EMERGENCY_WITHDRAW,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE, RATE_LIMIT_GUARD,
    THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::state::{
    Commiting, PoolState, Position, COMMIT_INFO, CREATOR_EXCESS_POSITION, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID,
};
use crate::swap_helper::{
    assert_max_spread, compute_swap, get_bluechip_value,
    get_usd_value_with_staleness_check, update_price_accumulator,
};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, Fraction,
    MessageInfo, Order, Reply, Response, StdError, StdResult, Storage, Timestamp, Uint128, WasmMsg,
};
use cw2::set_contract_version;
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
// use protobuf::Message;
use std::vec;
// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";
// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;
// Contract name that is used for migration.
const CONTRACT_NAME: &str = "bluechip-contracts-pool";
// Contract version that is used for migration.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    // M-1 FIX: Removed circular validate_factory_address call (was saving then loading
    // the same address and comparing it to itself). The real security check is below:
    // pools are instantiated via SubMsg from the factory, so info.sender IS the factory.
    let cfg = ExpectedFactory {
        expected_factory_address: msg.used_factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;
    if info.sender != cfg.expected_factory_address {
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
    let is_standard_pool = msg.is_standard_pool.unwrap_or(false);

    let threshold_payout_amounts = if is_standard_pool {
        ThresholdPayoutAmounts {
            creator_reward_amount: Uint128::zero(),
            bluechip_reward_amount: Uint128::zero(),
            pool_seed_amount: Uint128::zero(),
            commit_return_amount: Uint128::zero(),
        }
    } else {
        if let Some(params_binary) = msg.threshold_payout {
            let params: ThresholdPayoutAmounts = from_json(&params_binary)?;
            //make sure params match - no funny business with token minting.
            //checks total value and predetermined amounts for creator, BlueChip, original subscribers (commit amount), and the pool itself
            validate_pool_threshold_payments(&params)?;
            params
        } else {
            return Err(ContractError::InvalidThresholdParams {
                msg: format!("Your params could not be validated during pool instantiation."),
            });
        }
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
        owner: env.contract.address.clone(),
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

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: msg.commit_threshold_limit_usd,
        commit_amount_for_threshold: msg.commit_amount_for_threshold,
        max_bluechip_lock_per_pool: msg.max_bluechip_lock_per_pool,
        creator_excess_liquidity_lock_days: msg.creator_excess_liquidity_lock_days,
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
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    };

    USD_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    COMMITFEEINFO.save(deps.storage, &msg.commit_fee_info)?;
    NATIVE_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    IS_THRESHOLD_HIT.save(deps.storage, &is_standard_pool)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT_AMOUNTS.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_LIMIT_INFO.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    OWNER_POSITIONS.save(deps.storage, (&env.contract.address, "0"), &true)?;
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
        ExecuteMsg::UpdateConfigFromFactory { update } => {
            execute_update_config_from_factory(deps, info, update)
        }
        ExecuteMsg::Pause {} => execute_pause(deps, info),
        ExecuteMsg::Unpause {} => execute_unpause(deps, info),
        ExecuteMsg::EmergencyWithdraw {} => execute_emergency_withdraw(deps, env, info),
        ExecuteMsg::CancelEmergencyWithdraw {} => execute_cancel_emergency_withdraw(deps, info),
        ExecuteMsg::RecoverStuckStates { recovery_type } => {
            execute_recover_stuck_states(deps, env, info, recovery_type)
        }
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
        ExecuteMsg::ContinueDistribution {} => execute_continue_distribution(deps, env, info),
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
            max_ratio_deviation_bps,
        } => execute_remove_partial_liquidity(
            deps,
            env,
            info,
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        ),
        //removes all liquidity for a position - (i have 100 liquidity and I remove 100) - collects all fees.
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount1,
            min_amount0,
            max_ratio_deviation_bps,
        } => execute_remove_all_liquidity(
            deps,
            env,
            info,
            position_id,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        ),
        //removes liquidity based on a specific percent (I have 100 liquidity I want to remove 18% = remove 18.) - will collect fees in proportion of removal to rebalance accounting
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => execute_remove_partial_liquidity_by_percent(
            deps,
            env,
            info,
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        ),
        ExecuteMsg::ClaimCreatorExcessLiquidity {} => execute_claim_creator_excess(deps, env, info),
    }
}
pub fn execute_recover_stuck_states(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    recovery_type: RecoveryType,
) -> Result<Response, ContractError> {
    // Admin only check
    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;
    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    let mut attributes = vec![("action", "recover_stuck_states".to_string())];
    let mut recovered_items = vec![];

    match recovery_type {
        RecoveryType::StuckThreshold => {
            recover_threshold(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckDistribution => {
            recover_distribution(deps.storage, &env, &mut recovered_items)?;
        }
        RecoveryType::StuckReentrancyGuard => {
            // C-3 FIX: Allow factory admin to reset a stuck reentrancy guard
            recover_reentrancy_guard(deps.storage, &mut recovered_items)?;
        }
        RecoveryType::Both => {
            // Try to recover all, don't fail if one isn't stuck
            let _ = recover_threshold(deps.storage, &env, &mut recovered_items);
            let _ = recover_distribution(deps.storage, &env, &mut recovered_items);
            let _ = recover_reentrancy_guard(deps.storage, &mut recovered_items);
        }
    }

    if recovered_items.is_empty() {
        return Err(ContractError::NothingToRecover {});
    }

    attributes.push(("recovered", recovered_items.join(",")));

    Ok(Response::new().add_attributes(attributes))
}

// Helper functions to keep the logic clean
fn recover_threshold(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    // Check if threshold is actually stuck
    let last_threshold_time = LAST_THRESHOLD_ATTEMPT
        .may_load(storage)?
        .unwrap_or(Timestamp::from_seconds(0));

    if env.block.time.seconds() >= last_threshold_time.seconds() + 3600 {
        let was_stuck = THRESHOLD_PROCESSING.may_load(storage)?.unwrap_or(false);
        if was_stuck {
            THRESHOLD_PROCESSING.save(storage, &false)?;
            recovered.push("threshold".to_string());
        }
    }
    Ok(())
}

fn recover_distribution(
    storage: &mut dyn Storage,
    env: &Env,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    if let Some(dist_state) = DISTRIBUTION_STATE.may_load(storage)? {
        let time_since_update = env.block.time.seconds().saturating_sub(dist_state.last_updated.seconds());
        // Check if stuck (no update for 1 hour or too many failures)
        if time_since_update >= 3600 || dist_state.consecutive_failures >= 5 {
            // Restart distribution from the beginning instead of deleting state.
            // Count remaining committers in the ledger to set the correct remaining count.
            let remaining_committers = COMMIT_LEDGER
                .keys(storage, None, None, Order::Ascending)
                .count() as u32;

            if remaining_committers == 0 {
                // No committers left, just clean up
                DISTRIBUTION_STATE.remove(storage);
            } else {
                let restarted = DistributionState {
                    is_distributing: true,
                    total_to_distribute: dist_state.total_to_distribute,
                    total_committed_usd: dist_state.total_committed_usd,
                    last_processed_key: None, // restart from beginning
                    distributions_remaining: remaining_committers,
                    estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
                    max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
                    last_successful_batch_size: None,
                    consecutive_failures: 0,
                    started_at: env.block.time,
                    last_updated: env.block.time,
                };
                DISTRIBUTION_STATE.save(storage, &restarted)?;
            }

            recovered.push(format!(
                "distribution_restarted_{}_remaining",
                remaining_committers
            ));
        }
    }
    Ok(())
}

/// C-3 FIX: Reset the reentrancy guard if it gets stuck in `true` state.
/// This can only be called by the factory admin via RecoverStuckStates.
fn recover_reentrancy_guard(
    storage: &mut dyn Storage,
    recovered: &mut Vec<String>,
) -> StdResult<()> {
    let guard = RATE_LIMIT_GUARD.may_load(storage)?.unwrap_or(false);
    if guard {
        RATE_LIMIT_GUARD.save(storage, &false)?;
        recovered.push("reentrancy_guard".to_string());
    }
    Ok(())
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
        Ok(Cw20HookMsg::DepositLiquidity { .. }) | Ok(Cw20HookMsg::AddToPosition { .. }) => {
            // These operations require both native bluechip tokens (via info.funds)
            // and CW20 tokens. The CW20 Receive hook cannot carry native funds,
            // so these paths can never succeed. Use the direct ExecuteMsg entry
            // points instead, which accept native funds + do TransferFrom for CW20.
            //
            // Returning an error here prevents the user's CW20 tokens (already
            // transferred via Send) from being silently locked in the contract.
            // The CW20 Send will revert atomically, returning tokens to the sender.
            Err(ContractError::Std(StdError::generic_err(
                "DepositLiquidity and AddToPosition must be called directly, not via CW20 Send hook. \
                 These operations require native bluechip funds which cannot be sent via CW20 hooks.",
            )))
        }
        Err(err) => Err(ContractError::Std(err)),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    if msg.id == 42 {
        let res = cw_utils::parse_reply_instantiate_data(msg)
            .map_err(|e| StdError::generic_err(format!("parse error: {}", e)))?;

        let lp: Addr = deps.api.addr_validate(&res.contract_address)?;

        // M-2 FIX: Removed no-op POOL_INFO.update that loaded and saved unchanged state.
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

    let is_paused = POOL_PAUSED.may_load(deps.storage)?.unwrap_or(false);
    if is_paused {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    if pool_state.reserve0 < MINIMUM_LIQUIDITY || pool_state.reserve1 < MINIMUM_LIQUIDITY {
        POOL_PAUSED.save(deps.storage, &true)?;
        return Err(ContractError::InsufficientReserves {});
    }
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, offer_asset.amount, pool_specs.lp_fee)?;
    assert_max_spread(
        belief_price,
        max_spread,
        offer_asset.amount,
        return_amt.checked_add(commission_amt)?,
        spread_amt,
    )?;
    // Offer side: pool receives the full offer amount
    let offer_pool_post = offer_pool.checked_add(offer_asset.amount)?;
    // Ask side: pool pays out return_amt to the user. Commission stays in the pool
    // but is tracked separately in fee_reserve (via update_pool_fee_growth below),
    // so we subtract both from the tradeable reserve to avoid double-counting.
    // Effective ask reserve = ask_pool - return_amt - commission_amt
    let ask_pool_post = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

    if ask_pool_post < MINIMUM_LIQUIDITY {
        return Err(ContractError::InsufficientReserves {});
    }

    // Uniswap V2 pattern: accumulate price using OLD reserves (before this swap).
    // This makes the TWAP resistant to same-block manipulation — an attacker
    // would need to hold a skewed position across blocks to affect the accumulator.
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

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
    let usd_value = get_usd_value_with_staleness_check(
        deps.as_ref(),
        asset.amount,
        env.block.time.seconds(),
    )?;

    if usd_value.is_zero() {
        return Err(ContractError::InvalidOraclePrice {});
    }

    // Identify valid bluechip denom from pool config
    let bluechip_denom = match &pool_info.pool_info.asset_infos[0] {
        TokenType::Bluechip { denom } => denom.clone(),
        _ => match &pool_info.pool_info.asset_infos[1] {
            TokenType::Bluechip { denom } => denom.clone(),
            _ => return Err(ContractError::AssetMismatch {}),
        },
    };

    match &asset.info {
        TokenType::Bluechip { denom } if denom == &bluechip_denom => {
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
            let commit_fee_bluechip_amt = amount
                .checked_mul(fee_info.commit_fee_bluechip.numerator())
                .map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Fee calculation overflow: {}",
                        e
                    )))
                })?
                .checked_div(fee_info.commit_fee_bluechip.denominator())
                .map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Fee calculation error: {}",
                        e
                    )))
                })?;

            let commit_fee_creator_amt = amount
                .checked_mul(fee_info.commit_fee_creator.numerator())
                .map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Fee calculation overflow: {}",
                        e
                    )))
                })?
                .checked_div(fee_info.commit_fee_creator.denominator())
                .map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Fee calculation error: {}",
                        e
                    )))
                })?;

            let total_fees = commit_fee_bluechip_amt
                .checked_add(commit_fee_creator_amt)
                .map_err(|_| {
                    ContractError::Std(StdError::generic_err("Fee calculation overflow"))
                })?;

            if total_fees >= amount {
                return Err(ContractError::InvalidFee {});
            }

            // Also validate there's something left after fees
            let amount_after_fees = amount
                .checked_sub(total_fees)
                .map_err(|_| ContractError::Std(StdError::generic_err("Fee exceeds amount")))?;

            if amount_after_fees.is_zero() {
                return Err(ContractError::InvalidFee {});
            }
            let _total_fee_rate = fee_info.commit_fee_bluechip.checked_add(fee_info.commit_fee_creator)
                .map_err(|_| ContractError::Std(StdError::generic_err("Fee rate overflow")))?;
            // Create fee transfer messages
            if !commit_fee_bluechip_amt.is_zero() {
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
                messages.push(bluechip_transfer);
            }

            if !commit_fee_creator_amt.is_zero() {
                let creator_transfer = get_bank_transfer_to_msg(
                    &fee_info.creator_wallet_address,
                    &denom,
                    commit_fee_creator_amt,
                )
                .map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Creator transfer failed: {}",
                        e
                    )))
                })?;
                messages.push(creator_transfer);
            }
            // load state of threshold of the pool
            let threshold_already_hit = IS_THRESHOLD_HIT.load(deps.storage)?;

            if !threshold_already_hit {
                let current_usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
                let new_total = current_usd_raised.checked_add(usd_value)?;
                // Check if this commit will cross or exceed the threshold
                if new_total >= commit_config.commit_amount_for_threshold_usd {
                    LAST_THRESHOLD_ATTEMPT.save(deps.storage, &env.block.time)?;
                    // Try to acquire the threshold processing lock to trigger crossing
                    let processing = THRESHOLD_PROCESSING
                        .may_load(deps.storage)?
                        .unwrap_or(false);
                    let can_process = if processing {
                        false // a different transaction is processing
                    } else {
                        THRESHOLD_PROCESSING.save(deps.storage, &true)?;
                        true // committer's transaction gets to be the threshold crosser WHICH wins them nothing.
                    };
                    //logic if a different transaction indeed triggered the threshold crossing.
                    if !can_process {
                        //double check to handle race conditions
                        if IS_THRESHOLD_HIT.load(deps.storage)? {
                            // Threshold was hit, process as post-threshold
                            return process_post_threshold_commit(
                                deps,
                                env,
                                sender,
                                asset,
                                amount_after_fees,
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
                        // Pre-fee excess (for accounting/tracking only)
                        let bluechip_excess = asset.amount.checked_sub(bluechip_to_threshold)?;
                        // C-2 FIX: Fees were already deducted from the full `amount` and sent
                        // via BankMsg above (lines 865-893). The post-fee funds remaining in
                        // the contract are `amount_after_fees`. We must split that into the
                        // threshold portion and the excess portion proportionally, rather
                        // than deducting fees again from the excess.
                        let threshold_portion_after_fees = if amount.is_zero() {
                            Uint128::zero()
                        } else {
                            amount_after_fees.multiply_ratio(bluechip_to_threshold, amount)
                        };
                        let effective_bluechip_excess = amount_after_fees
                            .checked_sub(threshold_portion_after_fees)?;
                        let usd_excess = usd_value.checked_sub(usd_to_threshold)?;
                        // Update commit ledger with only the threshold portion
                        COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
                            Ok(v.unwrap_or_default().checked_add(usd_to_threshold)?)
                        })?;
                        // Set USD raised to exactly the threshold
                        USD_RAISED_FROM_COMMIT
                            .save(deps.storage, &commit_config.commit_amount_for_threshold_usd)?;

                        NATIVE_RAISED_FROM_COMMIT
                            .update::<_, ContractError>(deps.storage, |r| {
                                Ok(r.checked_add(bluechip_to_threshold)?)
                            })?;
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
                                        commit.total_paid_bluechip = commit.total_paid_bluechip.checked_add(bluechip_to_threshold)?;
                                        commit.total_paid_usd = commit.total_paid_usd.checked_add(usd_to_threshold)?;
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
                        if effective_bluechip_excess > Uint128::zero() {
                            // Load updated pool state (modified by threshold payout)
                            let mut pool_state = POOL_STATE.load(deps.storage)?;
                            let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
                            // Perform swap with fee-adjusted excess amount
                            let offer_pool = pool_state.reserve0;
                            let ask_pool = pool_state.reserve1;
                            //make sure both assets have been set in the pool properly
                            if !ask_pool.is_zero() && !offer_pool.is_zero() {
                                let (ret_amt, sp_amt, comm_amt) = compute_swap(
                                    offer_pool,
                                    ask_pool,
                                    effective_bluechip_excess,
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
                                    effective_bluechip_excess,
                                    return_amt,
                                    spread_amt,
                                )?;
                            }
                            // Accumulate price with OLD reserves before updating
                            update_price_accumulator(&mut pool_state, env.block.time.seconds())?;

                            // Update reserves with fee-adjusted amount (actual tokens remaining in pool)
                            // C-1 FIX: Subtract both return_amt AND commission_amt from ask reserve
                            // to avoid double-counting commission in both reserve1 and fee_reserve_1.
                            pool_state.reserve0 = offer_pool.checked_add(effective_bluechip_excess)?;
                            pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

                            // Update fee growth
                            update_pool_fee_growth(
                                &mut pool_fee_state,
                                &pool_state,
                                0,
                                commission_amt,
                            )?;
                            // Save states
                            POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
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
                                        commiting.total_paid_bluechip = commiting.total_paid_bluechip.checked_add(bluechip_excess)?;
                                        commiting.total_paid_usd = commiting.total_paid_usd.checked_add(usd_excess)?;
                                        Ok(commiting)
                                    } else {
                                        Err(ContractError::Std(StdError::generic_err(
                                            "Expected existing commit record for excess update"
                                        )))
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
                            .add_attribute("swap_amount_bluechip", effective_bluechip_excess.to_string())
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
                            Ok(v.unwrap_or_default().checked_add(usd_value)?)
                        })?;
                        // Update total USD raised
                        let final_usd = if new_total > commit_config.commit_amount_for_threshold_usd
                        {
                            commit_config.commit_amount_for_threshold_usd
                        } else {
                            new_total
                        };

                        USD_RAISED_FROM_COMMIT.save(deps.storage, &final_usd)?;
                        NATIVE_RAISED_FROM_COMMIT
                            .update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(asset.amount)?))?;
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
                                        commiting.total_paid_bluechip = commiting.total_paid_bluechip.checked_add(asset.amount)?;
                                        commiting.total_paid_usd = commiting.total_paid_usd.checked_add(usd_value)?;
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
                    amount_after_fees,
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

//commit transaction prior to threshold being crossed. commit to ledger and store values for return mint
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
        Ok(v.unwrap_or_default().checked_add(usd_value)?)
    })?;
    // Update total USD raised
    let _usd_total =
        USD_RAISED_FROM_COMMIT.update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(usd_value)?))?;
    NATIVE_RAISED_FROM_COMMIT.update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(asset.amount)?))?;

    COMMIT_INFO.update(
        deps.storage,
        &sender,
        |maybe_commiting| -> Result<_, ContractError> {
            match maybe_commiting {
                Some(mut commiting) => {
                    commiting.total_paid_bluechip = commiting.total_paid_bluechip.checked_add(asset.amount)?;
                    commiting.total_paid_usd = commiting.total_paid_usd.checked_add(usd_value)?;
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
    swap_amount: Uint128,
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
    // Calculate swap output using fee-adjusted amount (actual tokens remaining in pool)
    let (return_amt, spread_amt, commission_amt) =
        compute_swap(offer_pool, ask_pool, swap_amount, pool_specs.lp_fee)?;
    // Check slippage
    assert_max_spread(
        belief_price,
        max_spread,
        swap_amount,
        return_amt,
        spread_amt,
    )?;
    // Accumulate price with OLD reserves before updating
    update_price_accumulator(&mut pool_state, env.block.time.seconds())?;
    // Update reserves with fee-adjusted amount (actual tokens remaining in pool)
    // C-1 FIX: Subtract both return_amt AND commission_amt from ask reserve
    // to avoid double-counting commission in both reserve1 and fee_reserve_1.
    pool_state.reserve0 = offer_pool.checked_add(swap_amount)?;
    pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;
    // Update fee growth
    update_pool_fee_growth(&mut pool_fee_state, &pool_state, 0, commission_amt)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
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
                    commiting.total_paid_bluechip = commiting.total_paid_bluechip.checked_add(asset.amount)?;
                    commiting.total_paid_usd = commiting.total_paid_usd.checked_add(usd_value)?;
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

pub fn execute_continue_distribution(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Permissionless: anyone can trigger the next distribution batch.
    // Safety: amounts, recipients, and totals are all fixed in DISTRIBUTION_STATE.
    let dist_state = DISTRIBUTION_STATE.load(deps.storage)?;
    if !dist_state.is_distributing {
        return Err(ContractError::NothingToRecover {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut msgs = process_distribution_batch(deps.storage, &pool_info, &env)?;

    // M-5 FIX: Pay bounty from the pool's fee reserves (bluechip side) to actually
    // incentivize external callers. The previous self-funded model provided no incentive.
    let bluechip_denom = match &pool_info.pool_info.asset_infos[0] {
        TokenType::Bluechip { denom } => denom.clone(),
        _ => match &pool_info.pool_info.asset_infos[1] {
            TokenType::Bluechip { denom } => denom.clone(),
            _ => return Err(ContractError::AssetMismatch {}),
        },
    };

    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let bounty_paid = if pool_fee_state.fee_reserve_0 >= DISTRIBUTION_BOUNTY {
        // Pay bounty from fee reserves to the caller
        pool_fee_state.fee_reserve_0 = pool_fee_state
            .fee_reserve_0
            .checked_sub(DISTRIBUTION_BOUNTY)?;
        POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
        msgs.push(get_bank_transfer_to_msg(
            &info.sender,
            &bluechip_denom,
            DISTRIBUTION_BOUNTY,
        )?);
        DISTRIBUTION_BOUNTY
    } else {
        // Not enough in fee reserves; distribution still proceeds without bounty
        Uint128::zero()
    };

    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "continue_distribution")
        .add_attribute("remaining", dist_state.distributions_remaining.to_string())
        .add_attribute("bounty_paid", bounty_paid.to_string()))
}

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, msg: MigrateMsg) -> StdResult<Response> {
    match msg {
        MigrateMsg::UpdateFees { new_fees } => {
            // M-6 FIX: Add reasonable fee bounds (max 10%) to prevent
            // migration from setting abusive fee levels
            let max_lp_fee = Decimal::percent(10);
            if new_fees > max_lp_fee {
                return Err(StdError::generic_err(
                    "lp_fee must not exceed 10% (0.1)",
                ));
            }
            POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
                specs.lp_fee = new_fees;
                Ok(specs)
            })?;
        }
        MigrateMsg::UpdateVersion {} => {}
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("version", CONTRACT_VERSION))
}

pub fn execute_update_config_from_factory(
    deps: DepsMut,
    info: MessageInfo,
    update: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let mut attributes = vec![("action", "update_config")];

    if let Some(fee) = update.lp_fee {
        // H-NEW-1 FIX: apply the same 10% cap used in migrate() so the config-update
        // path cannot silently set extractive fees that steal from traders.
        let max_lp_fee = Decimal::percent(10);
        if fee > max_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must not exceed 10% (0.1)",
            )));
        }
        // L-1 FIX: enforce a minimum fee so LPs always earn something from swaps.
        // A fee of zero makes providing liquidity economically irrational and will
        // drain the pool through impermanent loss over time.
        let min_lp_fee = Decimal::permille(1); // 0.1%
        if fee < min_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must be at least 0.1% (0.001)",
            )));
        }
        POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
            specs.lp_fee = fee;
            Ok(specs)
        })?;
        attributes.push(("lp_fee", "updated"));
    }

    if let Some(interval) = update.min_commit_interval {
        // M-9 FIX: cap at 1 day so an admin cannot freeze the pool by setting an
        // absurdly large interval (e.g. u64::MAX would prevent all future commits).
        const MAX_COMMIT_INTERVAL: u64 = 86_400; // 24 hours
        if interval > MAX_COMMIT_INTERVAL {
            return Err(ContractError::Std(StdError::generic_err(
                "min_commit_interval must not exceed 86400 seconds (1 day)",
            )));
        }
        POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
            specs.min_commit_interval = interval;
            Ok(specs)
        })?;
        attributes.push(("min_commit_interval", "updated"));
    }

    if let Some(tolerance) = update.usd_payment_tolerance_bps {
        POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
            specs.usd_payment_tolerance_bps = tolerance;
            Ok(specs)
        })?;
        attributes.push(("usd_payment_tolerance_bps", "updated"));
    }

    if let Some(oracle_addr) = update.oracle_address {
        ORACLE_INFO.update(deps.storage, |mut info| -> StdResult<_> {
            info.oracle_addr = deps.api.addr_validate(&oracle_addr)?;
            Ok(info)
        })?;
        attributes.push(("oracle_address", "updated"));
    }

    Ok(Response::new().add_attributes(attributes))
}

pub fn execute_pause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    POOL_PAUSED.save(deps.storage, &true)?;
    Ok(Response::new().add_attribute("action", "pause"))
}

pub fn execute_unpause(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    POOL_PAUSED.save(deps.storage, &false)?;
    Ok(Response::new().add_attribute("action", "unpause"))
}

/// H-3 FIX (timelock): Two-phase emergency withdraw.
///
/// **Phase 1 — initiate** (first call, no pending state):
///   Pauses the pool and records a pending withdrawal that becomes
///   executable 24 hours later.  No funds are moved yet, giving LPs a
///   window to observe the pending action on-chain.
///
/// **Phase 2 — execute** (second call, after timelock):
///   Drains all reserves, fee reserves, and creator excess to the
///   protocol-controlled `bluechip_wallet_address`.
///
/// The factory admin can cancel a pending-but-unexecuted withdrawal with
/// `CancelEmergencyWithdraw {}`.
pub fn execute_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let now = env.block.time;

    // --- Phase 2: execute if timelock has elapsed ---
    if let Some(effective_after) = PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)? {
        if now < effective_after {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Emergency withdraw timelock not yet elapsed. Executable after: {}",
                effective_after
            ))));
        }

        // Timelock passed — execute the drain.
        PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);

        let mut pool_state = POOL_STATE.load(deps.storage)?;
        let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

        let mut total0 = pool_state.reserve0;
        let mut total1 = pool_state.reserve1;

        total0 = total0.checked_add(pool_fee_state.fee_reserve_0)?;
        total1 = total1.checked_add(pool_fee_state.fee_reserve_1)?;

        if let Ok(excess) = CREATOR_EXCESS_POSITION.load(deps.storage) {
            total0 = total0.checked_add(excess.bluechip_amount)?;
            total1 = total1.checked_add(excess.token_amount)?;
            CREATOR_EXCESS_POSITION.remove(deps.storage);
        }

        let fee_info = COMMITFEEINFO.load(deps.storage)?;
        let recipient = fee_info.bluechip_wallet_address.clone();

        let withdrawal_info = EmergencyWithdrawalInfo {
            withdrawn_at: now.seconds(),
            recipient: recipient.clone(),
            amount0: total0,
            amount1: total1,
            total_liquidity_at_withdrawal: pool_state.total_liquidity,
        };
        EMERGENCY_WITHDRAWAL.save(deps.storage, &withdrawal_info)?;

        pool_state.reserve0 = Uint128::zero();
        pool_state.reserve1 = Uint128::zero();
        POOL_STATE.save(deps.storage, &pool_state)?;

        pool_fee_state.fee_reserve_0 = Uint128::zero();
        pool_fee_state.fee_reserve_1 = Uint128::zero();
        POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

        let mut messages = vec![];

        if !total0.is_zero() {
            messages.push(
                TokenInfo {
                    info: pool_info.pool_info.asset_infos[0].clone(),
                    amount: total0,
                }
                .into_msg(&deps.querier, recipient.clone())?,
            );
        }

        if !total1.is_zero() {
            messages.push(
                TokenInfo {
                    info: pool_info.pool_info.asset_infos[1].clone(),
                    amount: total1,
                }
                .into_msg(&deps.querier, recipient.clone())?,
            );
        }

        return Ok(Response::new()
            .add_messages(messages)
            .add_attribute("action", "emergency_withdraw")
            .add_attribute("recipient", recipient)
            .add_attribute("amount0", total0)
            .add_attribute("amount1", total1)
            .add_attribute("total_liquidity", withdrawal_info.total_liquidity_at_withdrawal));
    }

    // --- Phase 1: initiate — pause pool and set timelock ---
    POOL_PAUSED.save(deps.storage, &true)?;
    let effective_after = now.plus_seconds(EMERGENCY_WITHDRAW_DELAY_SECONDS);
    PENDING_EMERGENCY_WITHDRAW.save(deps.storage, &effective_after)?;

    Ok(Response::new()
        .add_attribute("action", "emergency_withdraw_initiated")
        .add_attribute("effective_after", effective_after.to_string()))
}

/// Cancels a pending emergency withdrawal before its timelock elapses.
/// Only callable by the factory admin.
pub fn execute_cancel_emergency_withdraw(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_none() {
        return Err(ContractError::Std(StdError::generic_err(
            "No pending emergency withdrawal to cancel",
        )));
    }

    PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);
    // Un-pause the pool so trading can resume.
    POOL_PAUSED.save(deps.storage, &false)?;

    Ok(Response::new().add_attribute("action", "emergency_withdraw_cancelled"))
}
