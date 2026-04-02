use std::str::FromStr;

use crate::state::{
    PoolFeeState, PoolInfo, Position, TokenMetadata, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY,
    NEXT_POSITION_ID, OWNER_POSITIONS, POOL_FEE_STATE, POOL_INFO, POOL_STATE,
};
use cosmwasm_std::Storage;
use crate::{error::ContractError, state::CREATOR_EXCESS_POSITION};
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128, WasmMsg,
};
use pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg;
use crate::asset::get_bluechip_denom;

pub const OPTIMAL_LIQUIDITY: Uint128 = Uint128::new(1_000_000);
pub const MIN_MULTIPLIER: &str = "0.1";

pub fn calculate_unclaimed_fees(
    liquidity: Uint128,
    fee_growth_inside_last: Decimal,
    fee_growth_global: Decimal,
) -> StdResult<Uint128> {
    if fee_growth_global > fee_growth_inside_last {
        let fee_growth_delta = fee_growth_global - fee_growth_inside_last;
        liquidity.checked_mul_floor(fee_growth_delta).map_err(|e| {
            StdError::generic_err(format!("Fee calculation overflow: {}", e))
        })
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
    if fee_growth_global >= fee_growth_last {
        let fee_growth_delta = fee_growth_global - fee_growth_last;
        let earned_base = liquidity.checked_mul_floor(fee_growth_delta).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!("Fee base overflow: {}", e)))
        })?;
        let earned_adjusted = earned_base.checked_mul_floor(fee_multiplier).map_err(|e| {
            ContractError::Std(StdError::generic_err(format!("Fee multiplier overflow: {}", e)))
        })?;
        Ok(earned_adjusted)
    } else {
        Ok(Uint128::zero())
    }
}

pub fn calc_capped_fees(
    position: &Position,
    pool_fee_state: &PoolFeeState,
) -> Result<(Uint128, Uint128), ContractError> {
    let fees_0 = calculate_fees_owed(
        position.liquidity,
        pool_fee_state.fee_growth_global_0,
        position.fee_growth_inside_0_last,
        position.fee_size_multiplier,
    )?
    .checked_add(position.unclaimed_fees_0)?;

    let fees_1 = calculate_fees_owed(
        position.liquidity,
        pool_fee_state.fee_growth_global_1,
        position.fee_growth_inside_1_last,
        position.fee_size_multiplier,
    )?
    .checked_add(position.unclaimed_fees_1)?;

    Ok((
        fees_0.min(pool_fee_state.fee_reserve_0),
        fees_1.min(pool_fee_state.fee_reserve_1),
    ))
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
    let deviation_bps = if raw > u16::MAX as u128 { u16::MAX } else { raw as u16 };

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
        let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap_or(Decimal::percent(10));
        min_mult + (Decimal::one() - min_mult) * ratio
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

pub fn execute_claim_creator_excess(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let excess_position = CREATOR_EXCESS_POSITION.load(deps.storage)?;
    let pool_info = POOL_INFO.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    if info.sender != excess_position.creator {
        return Err(ContractError::Unauthorized {});
    }

    if env.block.time < excess_position.unlock_time {
        return Err(ContractError::PositionLocked {
            unlock_time: excess_position.unlock_time,
        });
    }

    let position_counter =
        NEXT_POSITION_ID.update(deps.storage, |n| -> StdResult<_> {
            n.checked_add(1).ok_or_else(|| StdError::generic_err("Position ID overflow"))
        })?;
    let position_id = position_counter.to_string();

    let metadata = TokenMetadata {
        name: Some("Creator excess position".to_string()),
        description: Some("Claim for excess bluechip/token liquidity".to_string()),
    };

    let mint_liquidity_nft = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(),
        msg: to_json_binary(&Cw721ExecuteMsg::<TokenMetadata>::Mint {
            token_id: position_id.clone(),
            owner: excess_position.creator.to_string(),
            token_uri: None,
            extension: metadata,
        })?,
        funds: vec![],
    };

    let product = excess_position
        .bluechip_amount
        .checked_mul(excess_position.token_amount)
        .map_err(|_| ContractError::Std(StdError::generic_err("overflow on multiplication")))?;
    let liquidity = integer_sqrt(product).max(Uint128::new(1));

    let fee_size_multiplier = calculate_fee_size_multiplier(liquidity);

    let position = Position {
        liquidity,
        owner: excess_position.creator.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier,
        unclaimed_fees_0: Uint128::zero(),
        unclaimed_fees_1: Uint128::zero(),
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;
    OWNER_POSITIONS.save(deps.storage, (&excess_position.creator, &position_id), &true)?;

    // Move excess tokens from held-aside into active reserves
    let mut pool_state = POOL_STATE.load(deps.storage)?;
    pool_state.reserve0 = pool_state
        .reserve0
        .checked_add(excess_position.bluechip_amount)?;
    pool_state.reserve1 = pool_state
        .reserve1
        .checked_add(excess_position.token_amount)?;
    pool_state.total_liquidity = pool_state.total_liquidity.checked_add(liquidity)?;
    POOL_STATE.save(deps.storage, &pool_state)?;

    CREATOR_EXCESS_POSITION.remove(deps.storage);

    Ok(Response::new()
        .add_message(CosmosMsg::Wasm(mint_liquidity_nft))
        .add_attribute("action", "claim_creator_excess")
        .add_attribute("position_id", position_id)
        .add_attribute("liquidity", liquidity.to_string()))
}
