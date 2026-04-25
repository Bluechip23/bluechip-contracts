use crate::asset::TokenType;
use crate::error::ContractError;
use crate::state::{
    PoolFeeState, PoolInfo, Position, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, OWNER_POSITIONS,
    POOL_STATE,
};
use cosmwasm_std::Storage;
use cosmwasm_std::{Addr, CosmosMsg, Decimal, Deps, StdError, StdResult, Uint128};

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

/// Build transfer messages for the two fee amounts, dispatching per-asset
/// on the pair's actual `TokenType` rather than the old
/// "asset 0 = native, asset 1 = CW20" assumption. Works for every pair
/// shape — commit pools (native/CW20), standard native/CW20,
/// standard native/native (e.g. the ATOM/bluechip anchor),
/// standard CW20/CW20.
pub fn build_fee_transfer_msgs(
    pool_info: &PoolInfo,
    recipient: &Addr,
    amount_0: Uint128,
    amount_1: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let mut msgs = Vec::new();
    for (asset_info, amount) in [
        (&pool_info.pool_info.asset_infos[0], amount_0),
        (&pool_info.pool_info.asset_infos[1], amount_1),
    ] {
        if amount.is_zero() {
            continue;
        }
        msgs.push(build_transfer_msg(asset_info, recipient, amount)?);
    }
    Ok(msgs)
}

/// Builds a single outgoing transfer message for `amount` of `asset_info`
/// going to `recipient`. Used by `build_fee_transfer_msgs` (fee payouts,
/// liquidity removal) and anywhere else the pool pushes assets out.
pub fn build_transfer_msg(
    asset_info: &TokenType,
    recipient: &Addr,
    amount: Uint128,
) -> Result<CosmosMsg, ContractError> {
    match asset_info {
        TokenType::Native { denom } => Ok(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: recipient.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: denom.clone(),
                amount,
            }],
        })),
        TokenType::CreatorToken { contract_addr } => {
            Ok(CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
                contract_addr: contract_addr.to_string(),
                msg: cosmwasm_std::to_json_binary(&cw20::Cw20ExecuteMsg::Transfer {
                    recipient: recipient.to_string(),
                    amount,
                })?,
                funds: vec![],
            }))
        }
    }
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

        // Reject first-deposits too small to absorb the MINIMUM_LIQUIDITY
        // lock. The lock itself is now applied by `execute_deposit_liquidity`
        // via `Position.locked_liquidity = MINIMUM_LIQUIDITY` rather than by
        // subtracting from the returned liquidity here, so the depositor's
        // Position carries the FULL `raw_liquidity` and accrues fees against
        // the full amount. They simply cannot withdraw the locked slice
        // (enforced in remove_*).
        if current_reserve0.is_zero()
            && current_reserve1.is_zero()
            && raw_liquidity <= MINIMUM_LIQUIDITY
        {
            return Err(ContractError::InsufficientLiquidityMinted {});
        }

        if raw_liquidity.is_zero() {
            return Err(ContractError::InsufficientLiquidityMinted {});
        }

        return Ok((raw_liquidity, final_amount0, final_amount1));
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_sqrt_basics() {
        assert_eq!(integer_sqrt(Uint128::zero()), Uint128::zero());
        assert_eq!(integer_sqrt(Uint128::new(1)), Uint128::new(1));
        assert_eq!(integer_sqrt(Uint128::new(4)), Uint128::new(2));
        assert_eq!(integer_sqrt(Uint128::new(100)), Uint128::new(10));
        // Non-perfect square floors.
        assert_eq!(integer_sqrt(Uint128::new(10)), Uint128::new(3));
        assert_eq!(integer_sqrt(Uint128::new(99)), Uint128::new(9));
        // Large value — must not overflow.
        let sq = integer_sqrt(Uint128::new(1_000_000_000_000_000_000));
        assert_eq!(sq, Uint128::new(1_000_000_000));
    }

    #[test]
    fn calculate_fees_owed_split_no_growth_is_zero() {
        let (owed, clipped) = calculate_fees_owed_split(
            Uint128::new(1_000_000),
            Decimal::zero(),
            Decimal::zero(),
            Decimal::one(),
        )
        .unwrap();
        assert_eq!(owed, Uint128::zero());
        assert_eq!(clipped, Uint128::zero());
    }

    #[test]
    fn calculate_fees_owed_split_full_multiplier_zero_clip() {
        // multiplier = 1.0 → nothing clipped
        let (owed, clipped) = calculate_fees_owed_split(
            Uint128::new(1_000_000),
            Decimal::percent(10),       // fee_growth_global
            Decimal::zero(),             // fee_growth_last
            Decimal::one(),              // multiplier
        )
        .unwrap();
        assert_eq!(owed, Uint128::new(100_000));
        assert_eq!(clipped, Uint128::zero());
    }

    #[test]
    fn calculate_fees_owed_split_with_clip() {
        // multiplier = 0.3 → 70% clipped
        let (owed, clipped) = calculate_fees_owed_split(
            Uint128::new(1_000_000),
            Decimal::percent(10),
            Decimal::zero(),
            Decimal::percent(30),
        )
        .unwrap();
        assert_eq!(owed, Uint128::new(30_000));
        assert_eq!(clipped, Uint128::new(70_000));
    }

    #[test]
    fn calculate_fee_size_multiplier_scales_linearly() {
        // At OPTIMAL_LIQUIDITY (1_000_000), multiplier is 1.0.
        assert_eq!(
            calculate_fee_size_multiplier(OPTIMAL_LIQUIDITY),
            Decimal::one()
        );
        // At zero, multiplier is MIN_MULTIPLIER (10%).
        assert_eq!(
            calculate_fee_size_multiplier(Uint128::zero()),
            Decimal::percent(10)
        );
        // At OPTIMAL/2, multiplier is MIN + (1 - MIN) * 0.5 = 0.1 + 0.45 = 0.55.
        let half = calculate_fee_size_multiplier(Uint128::new(500_000));
        assert_eq!(half, Decimal::percent(55));
        // Above OPTIMAL stays at 1.0.
        assert_eq!(
            calculate_fee_size_multiplier(Uint128::new(10_000_000)),
            Decimal::one()
        );
    }

    #[test]
    fn check_slippage_ok_when_at_or_above_min() {
        assert!(check_slippage(Uint128::new(100), Some(Uint128::new(100)), "asset0").is_ok());
        assert!(check_slippage(Uint128::new(101), Some(Uint128::new(100)), "asset0").is_ok());
        // No min means no check.
        assert!(check_slippage(Uint128::zero(), None, "asset0").is_ok());
    }

    #[test]
    fn check_slippage_rejects_below_min() {
        let r = check_slippage(Uint128::new(99), Some(Uint128::new(100)), "asset0");
        assert!(matches!(
            r,
            Err(ContractError::SlippageExceeded { .. })
        ));
    }

    #[test]
    fn check_ratio_deviation_ok_when_exact_ratio() {
        // 10:20 == 100:200 → 0 bps deviation → any tolerance passes
        let r = check_ratio_deviation(
            Uint128::new(100),
            Uint128::new(200),
            Some(Uint128::new(10)),
            Some(Uint128::new(20)),
            Some(50), // 50 bps = 0.5%
        );
        assert!(r.is_ok());
    }

    #[test]
    fn check_ratio_deviation_rejects_over_tolerance() {
        // 10:20 expected, got 100:50 → 4:1 vs 0.5:1 → way over any tolerance.
        let r = check_ratio_deviation(
            Uint128::new(100),
            Uint128::new(50),
            Some(Uint128::new(10)),
            Some(Uint128::new(20)),
            Some(100), // 1%
        );
        assert!(matches!(
            r,
            Err(ContractError::RatioDeviationExceeded { .. })
        ));
    }

    #[test]
    fn check_ratio_deviation_skipped_when_no_tolerance() {
        // Any ratio is ok when max_ratio_deviation_bps is None.
        let r = check_ratio_deviation(
            Uint128::new(1),
            Uint128::new(1_000_000),
            Some(Uint128::new(100)),
            Some(Uint128::new(100)),
            None,
        );
        assert!(r.is_ok());
    }
}
