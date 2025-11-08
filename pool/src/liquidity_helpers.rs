use std::str::FromStr;

use crate::state::{
    Position, TokenMetadata, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, POOL_FEE_STATE, POOL_INFO,
    POOL_STATE,
};
use crate::{error::ContractError, state::CREATOR_EXCESS_POSITION};
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128, WasmMsg,
};
use cw721_base::ExecuteMsg as CW721BaseExecuteMsg;
pub const OPTIMAL_LIQUIDITY: Uint128 = Uint128::new(1_000_000);
// 10% fees for tiny positions
pub const MIN_MULTIPLIER: &str = "0.1";
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

    // Generate position ID
    let position_counter =
        NEXT_POSITION_ID.update(deps.storage, |n| -> StdResult<_> { Ok(n + 1) })?;
    let position_id = format!("position_{}", position_counter);

    // Create metadata for the NFT
    let metadata = TokenMetadata {
        name: Some("Creator excess position".to_string()),
        description: Some("Claim for excess bluechip/token liquidity".to_string()),
    };

    // Use your existing NFT minting pattern
    let mint_liquidity_nft = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(),
        msg: to_json_binary(
            &CW721BaseExecuteMsg::<TokenMetadata, cosmwasm_std::Empty>::Mint {
                token_id: position_id.clone(),
                owner: excess_position.creator.to_string(),
                token_uri: None,
                extension: metadata,
            },
        )?,
        funds: vec![],
    };

    // Calculate liquidity value for this position
    let product = excess_position
        .bluechip_amount
        .checked_mul(excess_position.token_amount)
        .map_err(|_| ContractError::Std(StdError::generic_err("overflow on multiplication")))?;
    let liquidity = integer_sqrt(product).max(Uint128::new(1));

    let fee_size_multiplier = calculate_fee_size_multiplier(liquidity);

    // Store position using your existing structure
    let position = Position {
        liquidity,
        owner: excess_position.creator.clone(),
        fee_growth_inside_0_last: pool_fee_state.fee_growth_global_0,
        fee_growth_inside_1_last: pool_fee_state.fee_growth_global_1,
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier,
    };

    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;

    // Clean up the excess position record
    CREATOR_EXCESS_POSITION.remove(deps.storage);

    Ok(Response::new()
        .add_message(CosmosMsg::Wasm(mint_liquidity_nft))
        .add_attribute("action", "claim_creator_excess")
        .add_attribute("position_id", position_id)
        .add_attribute("liquidity", liquidity.to_string()))
}
