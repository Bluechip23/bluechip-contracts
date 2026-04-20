use crate::asset::get_bluechip_denom;
use crate::state::{
    CreatorFeePot, PoolFeeState, PoolInfo, Position, COMMITFEEINFO, CREATOR_FEE_POT,
    LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, OWNER_POSITIONS, POOL_INFO, POOL_STATE,
};
use crate::{error::ContractError, state::CREATOR_EXCESS_POSITION};
use cosmwasm_std::Storage;
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128, WasmMsg,
};

pub const OPTIMAL_LIQUIDITY: Uint128 = Uint128::new(1_000_000);
const MIN_MULTIPLIER: Decimal = Decimal::percent(10);

pub fn calculate_unclaimed_fees(
    liquidity: Uint128,
    fee_growth_inside_last: Decimal,
    fee_growth_global: Decimal,
) -> StdResult<Uint128> {
    if fee_growth_global > fee_growth_inside_last {
        let fee_growth_delta = fee_growth_global - fee_growth_inside_last;
        liquidity
            .checked_mul_floor(fee_growth_delta)
            .map_err(|e| StdError::generic_err(format!("Fee calculation overflow: {}", e)))
    } else {
        Ok(Uint128::zero())
    }
}

pub fn calculate_fees_owed(
    liquidity: Uint128,
    fee_growth_global: Decimal,
    fee_growth_last: Decimal,
    fee_multiplier: Decimal,
) -> Result<Uint128, ContractError> {
    Ok(calculate_fees_owed_split(
        liquidity,
        fee_growth_global,
        fee_growth_last,
        fee_multiplier,
    )?
    .0)
}

/// Same as `calculate_fees_owed` but also returns the clipped portion:
/// `earned_base - earned_adjusted`. Callers route that slice into
/// `CREATOR_FEE_POT` so it doesn't stay orphaned inside `fee_reserve_*`.
pub fn calculate_fees_owed_split(
    liquidity: Uint128,
    fee_growth_global: Decimal,
    fee_growth_last: Decimal,
    fee_multiplier: Decimal,
) -> Result<(Uint128, Uint128), ContractError> {
    if fee_growth_global >= fee_growth_last {
        let fee_growth_delta = fee_growth_global - fee_growth_last;
        let earned_base = liquidity.checked_mul_floor(fee_growth_delta).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!("Fee base overflow: {}", e)))
        })?;
        let earned_adjusted = earned_base.checked_mul_floor(fee_multiplier).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "Fee multiplier overflow: {}",
                e
            )))
        })?;
        // Clipped portion is never negative because `fee_multiplier <= 1`
        // (see `calculate_fee_size_multiplier`), so earned_adjusted <=
        // earned_base by definition. `saturating_sub` defends against any
        // future drift in that invariant without panicking here.
        let clipped = earned_base.saturating_sub(earned_adjusted);
        Ok((earned_adjusted, clipped))
    } else {
        Ok((Uint128::zero(), Uint128::zero()))
    }
}

pub fn calc_capped_fees(
    position: &Position,
    pool_fee_state: &PoolFeeState,
) -> Result<(Uint128, Uint128), ContractError> {
    let (capped, _, _) = calc_capped_fees_with_clip(position, pool_fee_state)?;
    Ok(capped)
}

/// Extended variant that returns `(capped_fees, raw_fees, clipped_fees)`.
///
/// - `capped_fees.0/1`: what the LP actually receives (clamped to fee_reserve).
/// - `clipped_fees.0/1`: slice the multiplier removed, to be routed to the
///   creator fee pot.
/// - `raw_fees` (internal): the uncapped total before the fee_reserve clamp;
///   exposed so callers can decide how to split the clamp between LP and pot
///   when `capped < raw`.
///
/// Returning both lets the fee-collection callers debit fee_reserve for
/// both portions and credit the clipped slice to `CREATOR_FEE_POT` in one
/// place, keeping the accounting symmetric.
pub fn calc_capped_fees_with_clip(
    position: &Position,
    pool_fee_state: &PoolFeeState,
) -> Result<((Uint128, Uint128), (Uint128, Uint128), (Uint128, Uint128)), ContractError> {
    let (adj_0, clip_0) = calculate_fees_owed_split(
        position.liquidity,
        pool_fee_state.fee_growth_global_0,
        position.fee_growth_inside_0_last,
        position.fee_size_multiplier,
    )?;
    let (adj_1, clip_1) = calculate_fees_owed_split(
        position.liquidity,
        pool_fee_state.fee_growth_global_1,
        position.fee_growth_inside_1_last,
        position.fee_size_multiplier,
    )?;

    // Fold preserved unclaimed fees into the adjusted amount only: those
    // were already multiplier-applied when they were preserved in
    // `remove_partial_liquidity`.
    let adj_0 = adj_0.checked_add(position.unclaimed_fees_0)?;
    let adj_1 = adj_1.checked_add(position.unclaimed_fees_1)?;

    // LP side capped at the reserve. Creator-clip is capped at whatever
    // reserve is left AFTER the LP payout, so the two debits together
    // never exceed what's actually in fee_reserve.
    let lp_0 = adj_0.min(pool_fee_state.fee_reserve_0);
    let lp_1 = adj_1.min(pool_fee_state.fee_reserve_1);
    let pot_cap_0 = pool_fee_state.fee_reserve_0.saturating_sub(lp_0);
    let pot_cap_1 = pool_fee_state.fee_reserve_1.saturating_sub(lp_1);
    let clip_0 = clip_0.min(pot_cap_0);
    let clip_1 = clip_1.min(pot_cap_1);

    Ok(((lp_0, lp_1), (adj_0, adj_1), (clip_0, clip_1)))
}

pub fn build_fee_transfer_msgs(
    pool_info: &PoolInfo,
    recipient: &Addr,
    amount_0: Uint128,
    amount_1: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut msgs = Vec::new();
    if !amount_0.is_zero() {
        let native_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;
        msgs.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: recipient.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: native_denom,
                amount: amount_0,
            }],
        }));
    }
    if !amount_1.is_zero() {
        msgs.push(CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: cosmwasm_std::to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: recipient.to_string(),
                amount: amount_1,
            })?,
            funds: vec![],
        }));
    }
    Ok(msgs)
}

pub fn check_slippage(
    actual: Uint128,
    min: Option<Uint128>,
    token: &str,
) -> Result<(), ContractError> {
    if let Some(min_val) = min {
        if actual < min_val {
            return Err(ContractError::SlippageExceeded {
                expected: min_val,
                actual,
                token: token.to_string(),
            });
        }
    }
    Ok(())
}

pub fn check_ratio_deviation(
    actual_amount0: Uint128,
    actual_amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<(), ContractError> {
    let max_deviation_bps = match max_ratio_deviation_bps {
        Some(v) => v,
        None => return Ok(()),
    };
    let (min0, min1) = match (min_amount0, min_amount1) {
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(()),
    };
    if min0.is_zero() || min1.is_zero() || actual_amount0.is_zero() || actual_amount1.is_zero() {
        return Ok(());
    }
    let expected_ratio = Decimal::from_ratio(min0, min1);
    let actual_ratio = Decimal::from_ratio(actual_amount0, actual_amount1);
    let (larger, smaller) = if actual_ratio > expected_ratio {
        (actual_ratio, expected_ratio)
    } else {
        (expected_ratio, actual_ratio)
    };
    let diff = larger
        .checked_sub(smaller)
        .map_err(|_| StdError::generic_err("Ratio calculation overflow"))?;
    let raw = (diff
        .checked_mul(Decimal::from_ratio(10000u128, 1u128))
        .map_err(|_| StdError::generic_err("Deviation calculation overflow"))?
        / smaller)
        .to_uint_floor()
        .u128();
    let deviation_bps = if raw > u16::MAX as u128 {
        u16::MAX
    } else {
        raw as u16
    };

    if deviation_bps > max_deviation_bps {
        return Err(ContractError::RatioDeviationExceeded {
            expected_ratio,
            actual_ratio,
            max_deviation_bps,
            actual_deviation_bps: deviation_bps,
        });
    }
    Ok(())
}

/// Linear scaling from MIN_MULTIPLIER (10%) to 100% based on position size
/// relative to OPTIMAL_LIQUIDITY. Penalizes small positions to discourage
/// dust griefing.
pub fn calculate_fee_size_multiplier(liquidity: Uint128) -> Decimal {
    if liquidity >= OPTIMAL_LIQUIDITY {
        Decimal::one()
    } else {
        let ratio = Decimal::from_ratio(liquidity, OPTIMAL_LIQUIDITY);
        MIN_MULTIPLIER + (Decimal::one() - MIN_MULTIPLIER) * ratio
    }
}

pub fn integer_sqrt(value: Uint128) -> Uint128 {
    if value.is_zero() {
        return Uint128::zero();
    }
    let mut x = value;
    let mut y = value.saturating_add(Uint128::one()) / Uint128::new(2);
    while y < x {
        x = y;
        y = (y.saturating_add(value / y)) / Uint128::new(2);
    }
    x
}

pub fn calc_liquidity_for_deposit(
    deps: Deps,
    amount0: Uint128,
    amount1: Uint128,
) -> Result<(Uint128, Uint128, Uint128), ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;
    let total_liquidity = pool_state.total_liquidity;

    if current_reserve0.is_zero() || current_reserve1.is_zero() || total_liquidity.is_zero() {
        if amount0.is_zero() || amount1.is_zero() {
            return Err(ContractError::Std(StdError::generic_err(
                "Initial deposit requires both assets",
            )));
        }

        let (final_amount0, final_amount1) =
            if !current_reserve0.is_zero() && !current_reserve1.is_zero() {
                // Post-threshold: maintain existing ratio
                let optimal_amount1 = current_reserve1.multiply_ratio(amount0, current_reserve0);
                let optimal_amount0 = current_reserve0.multiply_ratio(amount1, current_reserve1);

                if optimal_amount1 <= amount1 {
                    (amount0, optimal_amount1)
                } else {
                    (optimal_amount0, amount1)
                }
            } else {
                (amount0, amount1)
            };

        if final_amount0.is_zero() || final_amount1.is_zero() {
            return Err(ContractError::InsufficientLiquidity {});
        }

        let product = final_amount0.checked_mul(final_amount1)?;
        let raw_liquidity = integer_sqrt(product).max(Uint128::new(1));

        let liquidity = if current_reserve0.is_zero() && current_reserve1.is_zero() {
            // First deposit ever: lock MINIMUM_LIQUIDITY permanently
            if raw_liquidity <= MINIMUM_LIQUIDITY {
                return Err(ContractError::InsufficientLiquidityMinted {});
            }
            raw_liquidity.checked_sub(MINIMUM_LIQUIDITY)?
        } else {
            raw_liquidity
        };

        if liquidity.is_zero() {
            return Err(ContractError::InsufficientLiquidityMinted {});
        }

        return Ok((liquidity, final_amount0, final_amount1));
    }

    if amount0.is_zero() || amount1.is_zero() {
        if amount0.is_zero() {
            return Err(ContractError::Std(StdError::generic_err("amount0 is zero")));
        }
        if amount1.is_zero() {
            return Err(ContractError::Std(StdError::generic_err("amount1 is zero")));
        }
    }

    let optimal_amount1_for_amount0 = current_reserve1.multiply_ratio(amount0, current_reserve0);
    let optimal_amount0_for_amount1 = current_reserve0.multiply_ratio(amount1, current_reserve1);

    let (final_amount0, final_amount1) = if optimal_amount1_for_amount0 <= amount1 {
        (amount0, optimal_amount1_for_amount0)
    } else {
        (optimal_amount0_for_amount1, amount1)
    };

    if final_amount0.is_zero() || final_amount1.is_zero() {
        return Err(ContractError::InsufficientLiquidity {});
    }

    let liquidity_from_amount0 = total_liquidity.multiply_ratio(final_amount0, current_reserve0);
    let liquidity_from_amount1 = total_liquidity.multiply_ratio(final_amount1, current_reserve1);
    let liquidity = liquidity_from_amount0.min(liquidity_from_amount1);

    if liquidity.is_zero() {
        return Err(ContractError::InsufficientLiquidityMinted {});
    }

    Ok((liquidity, final_amount0, final_amount1))
}

pub fn verify_position_ownership(
    deps: Deps,
    nft_contract: &Addr,
    token_id: &str,
    expected_owner: &Addr,
) -> Result<(), ContractError> {
    let owner_response: pool_factory_interfaces::cw721_msgs::OwnerOfResponse =
        deps.querier.query_wasm_smart(
            nft_contract,
            &pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf {
                token_id: token_id.to_string(),
                include_expired: None,
            },
        )?;

    if owner_response.owner != expected_owner.to_string() {
        return Err(ContractError::Unauthorized {});
    }

    Ok(())
}

/// Detects NFT transfers and resets fee checkpoints so the new owner
/// cannot claim fees that accrued before the transfer.
pub fn sync_position_on_transfer(
    storage: &mut dyn Storage,
    position: &mut Position,
    position_id: &str,
    current_owner: &Addr,
    pool_fee_state: &PoolFeeState,
) -> Result<bool, ContractError> {
    if position.owner == *current_owner {
        return Ok(false);
    }

    let old_owner = position.owner.clone();

    position.fee_growth_inside_0_last = pool_fee_state.fee_growth_global_0;
    position.fee_growth_inside_1_last = pool_fee_state.fee_growth_global_1;
    position.unclaimed_fees_0 = Uint128::zero();
    position.unclaimed_fees_1 = Uint128::zero();

    position.owner = current_owner.clone();

    OWNER_POSITIONS.remove(storage, (&old_owner, position_id));
    OWNER_POSITIONS.save(storage, (current_owner, position_id), &true)?;

    LIQUIDITY_POSITIONS.save(storage, position_id, position)?;

    Ok(true)
}

/// Empties the CREATOR_FEE_POT to the creator wallet configured at pool
/// instantiation. Only the creator wallet can call this. Clip-slice fees
/// accumulate in the pot via `execute_collect_fees`, `add_to_position`,
/// `remove_all_liquidity`, and `remove_partial_liquidity`.
pub fn execute_claim_creator_fees(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transaction_deadline: Option<cosmwasm_std::Timestamp>,
) -> Result<Response, ContractError> {
    crate::generic_helpers::enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    if info.sender != fee_info.creator_wallet_address {
        return Err(ContractError::Unauthorized {});
    }

    let pot = CREATOR_FEE_POT.may_load(deps.storage)?.unwrap_or_default();
    if pot.amount_0.is_zero() && pot.amount_1.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let mut messages: Vec<CosmosMsg> = vec![];

    if !pot.amount_0.is_zero() {
        let native_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: fee_info.creator_wallet_address.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: native_denom,
                amount: pot.amount_0,
            }],
        }));
    }
    if !pot.amount_1.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: fee_info.creator_wallet_address.to_string(),
                amount: pot.amount_1,
            })?,
            funds: vec![],
        }));
    }

    // Reset the pot AFTER building the messages so a serialization error
    // in building the CW20 transfer would leave the pot intact.
    CREATOR_FEE_POT.save(
        deps.storage,
        &CreatorFeePot {
            amount_0: cosmwasm_std::Uint128::zero(),
            amount_1: cosmwasm_std::Uint128::zero(),
        },
    )?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "claim_creator_fees")
        .add_attribute("creator", fee_info.creator_wallet_address.to_string())
        .add_attribute("amount_0", pot.amount_0.to_string())
        .add_attribute("amount_1", pot.amount_1.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

pub fn execute_claim_creator_excess(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transaction_deadline: Option<cosmwasm_std::Timestamp>,
) -> Result<Response, ContractError> {
    // Same deadline semantics as commit / swap / liquidity handlers: when
    // provided, reject txs that landed past the caller's deadline. Keeps
    // claims from being ambushed by a mempool replay long after the
    // creator expected their tx to be final.
    crate::generic_helpers::enforce_transaction_deadline(env.block.time, transaction_deadline)?;

    let excess_position = CREATOR_EXCESS_POSITION.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;

    if info.sender != excess_position.creator {
        return Err(ContractError::Unauthorized {});
    }

    if env.block.time < excess_position.unlock_time {
        return Err(ContractError::PositionLocked {
            unlock_time: excess_position.unlock_time,
        });
    }

    CREATOR_EXCESS_POSITION.remove(deps.storage);

    // Send tokens directly to the creator instead of creating an LP position.
    // The creator can deposit as liquidity themselves if they choose to.
    let mut messages: Vec<CosmosMsg> = vec![];

    if !excess_position.bluechip_amount.is_zero() {
        let native_denom = get_bluechip_denom(&pool_info.pool_info.asset_infos)?;
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: excess_position.creator.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: native_denom,
                amount: excess_position.bluechip_amount,
            }],
        }));
    }

    if !excess_position.token_amount.is_zero() {
        messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.token_address.to_string(),
            msg: to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                recipient: excess_position.creator.to_string(),
                amount: excess_position.token_amount,
            })?,
            funds: vec![],
        }));
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "claim_creator_excess")
        .add_attribute("creator", excess_position.creator.to_string())
        .add_attribute(
            "bluechip_amount",
            excess_position.bluechip_amount.to_string(),
        )
        .add_attribute("token_amount", excess_position.token_amount.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}
