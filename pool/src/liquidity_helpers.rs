use std::str::FromStr;

use cosmwasm_std::{
    Addr, Decimal, Decimal256, Deps, Fraction, StdError, StdResult, Uint128, Uint256,
};

use crate::{error::ContractError, generic_helpers::decimal2decimal256, state::POOL_STATE};

pub fn calculate_unclaimed_fees(
    liquidity: Uint128,
    //the fee_growth_global number the last time the position collected fees
    fee_growth_inside_last: Decimal,
    //fee growth of pool PER liquidty unit
    fee_growth_global: Decimal,
) -> Uint128 {
    if fee_growth_global > fee_growth_inside_last {
        let fee_growth_delta = fee_growth_global - fee_growth_inside_last;
        //number of liquidity units * delta
        liquidity * fee_growth_delta
    } else {
        Uint128::zero()
    }
}

// calculates swap amounts using constant product formula (x * y = k)
pub fn compute_swap(
    //pool balance of offer amount
    offer_pool: Uint128,
    //pool balance of requested amount
    ask_pool: Uint128,
    //amount being offered
    offer_amount: Uint128,
    //pool fee rate
    commission_rate: Decimal,
) -> StdResult<(Uint128, Uint128, Uint128)> {
    let offer_pool: Uint256 = offer_pool.into();
    let ask_pool: Uint256 = ask_pool.into();
    let offer_amount: Uint256 = offer_amount.into();
    let commission_rate = decimal2decimal256(commission_rate)?;
    // constant product
    let cp: Uint256 = offer_pool * ask_pool;

    let return_amount: Uint256 = (Decimal256::from_ratio(ask_pool, 1u8)
        - Decimal256::from_ratio(cp, offer_pool + offer_amount))
    .numerator()
        / Decimal256::one().denominator();

    // calculate spread(slippage) & commission
    let spread_amount: Uint256 = (offer_amount
        * Decimal256::from_ratio(ask_pool, offer_pool).numerator()
        / Decimal256::from_ratio(ask_pool, offer_pool).denominator())
        - return_amount;
    let commission_amount: Uint256 =
        return_amount * commission_rate.numerator() / commission_rate.denominator();
    //subtract commission from return amount
    let return_amount: Uint256 = return_amount - commission_amount;
    Ok((
        //amount trader recieves
        return_amount.try_into()?,
        //slippage
        spread_amount.try_into()?,
        //fee to liquidity holders
        commission_amount.try_into()?,
    ))
}

//find fee growth per unit of liquidity and then multiply it by the amount of liquidity units owned by the postiion.
pub fn calculate_fees_owed(
    liquidity: Uint128,
    fee_growth_global: Decimal,
    fee_growth_last: Decimal,
    fee_multiplier: Decimal,
) -> Uint128 {
    if fee_growth_global >= fee_growth_last {
        let fee_growth_delta = fee_growth_global - fee_growth_last;
        let earned_base = liquidity * fee_growth_delta;
        //apply size base multipliers
        let earned_adjusted = earned_base * fee_multiplier;
        earned_adjusted
    } else {
        Uint128::zero()
    }
}
//used to protect against many small liquidity positions
pub fn calculate_fee_size_multiplier(liquidity: Uint128) -> Decimal {
    //if position has optimal liquidty they will not be punished
    pub const OPTIMAL_LIQUIDITY: Uint128 = Uint128::new(1_000_000);
    // 10% fees for tiny positions
    pub const MIN_MULTIPLIER: &str = "0.1";

    if liquidity >= OPTIMAL_LIQUIDITY {
        //provide full fees for optimal size
        Decimal::one()
    } else {
        // linear scaling from 10% to 100% relative to position size
        let ratio = Decimal::from_ratio(liquidity, OPTIMAL_LIQUIDITY);
        let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap();
        min_mult + (Decimal::one() - min_mult) * ratio
    }
}

//geometric mean for liquidity providing.
pub fn integer_sqrt(value: Uint128) -> Uint128 {
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

//calculate optimal deposit amounts
pub fn calc_liquidity_for_deposit(
    deps: Deps,
    amount0: Uint128,
    amount1: Uint128,
) -> Result<(Uint128, Uint128, Uint128), ContractError> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    let current_reserve0 = pool_state.reserve0;
    let current_reserve1 = pool_state.reserve1;

    //ensure reserves exists
    if current_reserve0.is_zero() || current_reserve1.is_zero() {
        // specific error to know WHICH is the culprit of being zero
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
    // Add specific error to know WHICH is the culprit of being zero
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
        //not enough amount1, use all of amount0
        (amount0, optimal_amount1_for_amount0)
    } else {
        // not enough amount1, use all their amount1 and scale down amount0
        (optimal_amount0_for_amount1, amount1)
    };

    if final_amount0.is_zero() || final_amount1.is_zero() {
        return Err(ContractError::InsufficientLiquidity {});
    }

    let product = final_amount0.checked_mul(final_amount1)?;
    //geometric mean
    let liquidity = integer_sqrt(product).max(Uint128::new(1));

    if liquidity.is_zero() {
        return Err(ContractError::InsufficientLiquidityMinted {});
    }

    Ok((liquidity, final_amount0, final_amount1))
}

//check to make sure liquidity positions cant be tampered with by non owners
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
