
use std::str::FromStr;

use cosmwasm_std::{testing::{mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, }, to_json_binary, Addr, BankMsg, Binary, Coin, ContractResult, CosmosMsg, Decimal, OwnedDeps, SystemError, SystemResult, Timestamp, Uint128, WasmMsg, WasmQuery
};

use cw721::OwnerOfResponse;

use crate::{asset::PoolPairType, contract::{execute,}, liquidity::{execute_add_to_position, execute_collect_fees, execute_deposit_liquidity, execute_remove_all_liquidity}, liquidity_helpers::{calculate_fee_size_multiplier, MIN_MULTIPLIER}, msg::{CommitFeeInfo}, state::{CommitLimitInfo, OracleInfo, PoolFeeState, PoolInfo, PoolSpecs, PoolState, ThresholdPayoutAmounts, COMMITFEEINFO, COMMITSTATUS, COMMIT_LIMIT_INFO, LIQUIDITY_POSITIONS, NATIVE_RAISED_FROM_COMMIT, ORACLE_INFO, POOL_INFO, POOL_SPECS, THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING
    }};
use crate::msg::ExecuteMsg;
use crate::state::{
    IS_THRESHOLD_HIT, USD_RAISED_FROM_COMMIT,
 POOL_STATE, POOL_FEE_STATE, Position, NEXT_POSITION_ID, PoolDetails
};
use crate::error::ContractError;
use crate::asset::{TokenInfo, TokenType};
const OPTIMAL_LIQUIDITY: u128 = 1_000_000;

#[test]
fn test_deposit_liquidity_first_position() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let bluechip_amount = Uint128::new(1_000_000_000); // 1k bluechip
    let token_amount = Uint128::new(14_893_617_021); // Approximately correct ratio
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: bluechip_amount,
    }]);
    
    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user.clone(),
        bluechip_amount,
        token_amount,
        None, // min_amount0
        None, // min_amount1
        None, // transaction_deadline
    ).unwrap();
    
    assert!(res.messages.iter().any(|msg| {
        matches!(&msg.msg, CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) 
            if contract_addr == "nft_contract")
    }));
    
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap(); // ID starts at 1, increments to 2
    assert_eq!(position.owner, user);
    assert!(position.liquidity > Uint128::zero());
    
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.total_liquidity > Uint128::new(91_104_335_791)); // Initial + new
    
    assert_eq!(NEXT_POSITION_ID.load(&deps.storage).unwrap(), 2);
}

#[test]
fn test_deposit_liquidity_with_slippage() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let bluechip_amount = Uint128::new(1_000_000_000);
    let token_amount = Uint128::new(10_000_000_000); // Incorrect ratio
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: bluechip_amount,
    }]);
    
    let err = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user,
        bluechip_amount,
        token_amount,
        Some(Uint128::new(950_000_000)), // min_amount0 - Expect at least 95% of bluechip
        Some(Uint128::new(14_000_000_000)), // min_amount1 - Expect significant token amount
        None,
    ).unwrap_err();
    
    match err {
        ContractError::SlippageExceeded { .. } => (),
        _ => panic!("Expected SlippageExceeded error"),
    }
}

#[test]
fn test_add_to_existing_position() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    // Mock ownership query response
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let bluechip_amount = Uint128::new(500_000_000); // 500 bluechip
    let token_amount = Uint128::new(7_500_000_000); // Approximately correct ratio
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: bluechip_amount,
    }]);
    
    let res = execute_add_to_position(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        user, // position_id
        bluechip_amount,
        token_amount,
        None, // min_amount0
        None, // min_amount1
        None, // transaction_deadline
    ).unwrap();
    
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(position.liquidity > Uint128::new(1_000_000));
    
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "add_to_position");
}

#[test]
fn test_add_to_position_not_owner() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position owned by someone else
    create_test_position(&mut deps, 1, "other_user", Uint128::new(1_000_000));
    
    // Mock NFT ownership check - different owner
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "other_user".to_string(), // Different owner
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let err = execute_add_to_position(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        user,
        Uint128::new(1_000_000),
        Uint128::new(15_000_000),
        None,
        None,
        None,
    ).unwrap_err();
    
    match err {
        ContractError::Unauthorized {} => (),
        _ => panic!("Expected Unauthorized error"),
    }
}

#[test]
fn test_collect_fees_with_accrued_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position with significant liquidity
    create_test_position(&mut deps, 1, "fee_collector", Uint128::new(10_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "fee_collector".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::percent(1); // 1% fees
    fee_state.fee_growth_global_1 = Decimal::percent(2); // 2% fees
    fee_state.total_fees_collected_0 = Uint128::new(100_000);
    fee_state.total_fees_collected_1 = Uint128::new(200_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    let env = mock_env();
    let info = mock_info("fee_collector", &[]);
    
    let res = execute_collect_fees(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
    ).unwrap();
    
    // Verify fee collection messages (bluechip and CW20)
    assert!(res.messages.len() >= 1); // At least one fee transfer
    
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.fee_growth_inside_0_last, fee_state.fee_growth_global_0);
    assert_eq!(position.fee_growth_inside_1_last, fee_state.fee_growth_global_1);
}
#[test]
fn test_remove_all_liquidity() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let initial_liquidity = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    
    // Create position to increase liquidity
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Verify liquidity increased
    let after_add = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert!(after_add > initial_liquidity);
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let res = execute_remove_all_liquidity(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        None, // transaction_deadline
        None, // min_amount0
        None, // min_amount1
    ).unwrap();
    
    assert!(res.messages.len() >= 2);
    
    assert!(LIQUIDITY_POSITIONS.load(&deps.storage, "1").is_err());
    
    // Verify pool liquidity decreased back to initial amount
    let final_liquidity = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert_eq!(final_liquidity, initial_liquidity);
}

#[test]
fn test_deposit_liquidity_imbalanced_amounts() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    // Provide way too much bluechip token
    let bluechip_amount = Uint128::new(10_000_000_000); // 10k bluechip
    let token_amount = Uint128::new(1_000_000_000); // Only 1k tokens (should need ~149k)
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: bluechip_amount,
    }]);
    
    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user,
        bluechip_amount,
        token_amount,
        None,
        None,
        None,
    ).unwrap();
    
    let refund_msg = res.messages.iter().find(|msg| {
        matches!(&msg.msg, CosmosMsg::Bank(BankMsg::Send { .. }))
    });
    assert!(refund_msg.is_some());
    
    let refund_attr = res.attributes.iter().find(|a| a.key == "refunded_amount0").unwrap();
    assert!(Uint128::new(refund_attr.value.parse::<u128>().unwrap()) > Uint128::zero());
}

#[test]
fn test_remove_liquidity_with_slippage_protection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(20_000_000_000); // Reduce reserves
    pool_state.reserve1 = Uint128::new(300_000_000_000);
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let msg = ExecuteMsg::RemoveAllLiquidity {
        position_id: "1".to_string(),
        transaction_deadline: None,
        min_amount0: Some(Uint128::new(1_000_000_000)), // Expect high amount
        min_amount1: Some(Uint128::new(15_000_000_000)),

    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::SlippageExceeded { .. } => (),
        _ => panic!("Expected SlippageExceeded error"),
    }
}


#[test]
fn test_remove_partial_liquidity_amount() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));

     deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    // Mock ownership query response
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    // Create position with 1M liquidity
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Remove 300k liquidity
    let msg = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::new(300_000),
        transaction_deadline: None,
        min_amount0: None,
        min_amount1: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.liquidity, Uint128::new(700_000)); // 1M - 300k
    
    assert!(res.messages.len() >= 2); // Asset returns
}

#[test]
fn test_remove_partial_liquidity_by_percent() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    let initial_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 25,
        transaction_deadline: None,
        min_amount0: None,
        min_amount1: None, // 25%
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify the action
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "remove_partial_liquidity");
    
    // Verify 25% was removed from position
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.liquidity, Uint128::new(750_000)); // 75% remaining
    
    assert_eq!(
        res.attributes.iter().find(|a| a.key == "liquidity_removed").unwrap().value, 
        "250000" // 25% of 1M
    );
    
    let final_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        final_pool_state.total_liquidity,
        initial_pool_state.total_liquidity - Uint128::new(250_000)
    );
    
    assert!(res.messages.len() >= 2);
}

#[test]
fn test_zero_liquidity_fee_collection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Set pool to zero liquidity
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.total_liquidity = Uint128::zero();
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    
    // Try to update fee growth
    let env = mock_env();
    let info = mock_info("trader", &[Coin {
        denom: "bluechip".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "bluechip".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    
    execute(deps.as_mut(), env, info, msg).unwrap(); 
    
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(fee_state.fee_growth_global_0, Decimal::zero());
}

#[test]
fn test_price_accumulator_zero_reserves() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps); // Pre-threshold, zero reserves
    
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(1000);
    
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::zero());
    assert_eq!(pool_state.reserve1, Uint128::zero());
    
    assert_eq!(pool_state.price0_cumulative_last, Uint128::zero());
    assert_eq!(pool_state.price1_cumulative_last, Uint128::zero());
}

#[test]
fn test_collect_fees_no_fees_accrued() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position but don't accrue any fees
    create_test_position(&mut deps, 1, "fee_collector", Uint128::new(1_000_000));
    
    
        deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "fee_collector".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    let env = mock_env();
    let info = mock_info("fee_collector", &[]);
    let msg = ExecuteMsg::CollectFees {
        position_id: "1".to_string(),
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    assert_eq!(res.messages.len(), 0);
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "collect_fees");
}

#[test]
fn test_invalid_percentage_removal() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Try to remove more than 100%
    let msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 0,
        transaction_deadline: None,
        min_amount0: None,
        min_amount1: None, // Invalid
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::InvalidPercent {} => (),
        _ => panic!("Expected InvalidPercent error"),
    }
}

// Sets up a pool in pre-threshold state with all necessary configuration
pub fn setup_pool_storage(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    let pool_info = PoolInfo {
        pool_id: 1u64,
        pool_info: PoolDetails {
            asset_infos: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken{
                    contract_addr: Addr::unchecked("token_contract"),
                },
            ],
            contract_addr: Addr::unchecked("pool_contract"),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: Addr::unchecked("factory_contract"),
        token_address: Addr::unchecked("token_contract"),
        position_nft_address: Addr::unchecked("nft_contract"),
    };
    POOL_INFO.save(&mut deps.storage, &pool_info).unwrap();

    let pool_state = PoolState {
        pool_contract_address: Addr::unchecked("pool_contract"),
        nft_ownership_accepted: true,
        reserve0: Uint128::zero(), // No reserves pre-threshold
        reserve1: Uint128::zero(),
        total_liquidity: Uint128::zero(),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOL_FEE_STATE.save(&mut deps.storage, &pool_fee_state).unwrap();

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::percent(3) / Uint128::new(10), // 0.3% fee (3/1000)
        min_commit_interval: 60, // 1 minute minimum between commits
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(100_000_000), // 100 bluechip tokens
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    COMMIT_LIMIT_INFO.save(&mut deps.storage, &commit_config).unwrap();

    let threshold_payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000), // 325k tokens
        bluechip_reward_amount: Uint128::new(25_000_000_000), // 25k tokens
        pool_seed_amount: Uint128::new(350_000_000_000),     // 350k tokens
        commit_return_amount: Uint128::new(500_000_000_000),   // 500k tokens
    };
    THRESHOLD_PAYOUT_AMOUNTS.save(&mut deps.storage, &threshold_payout).unwrap();
    let commit_fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip_treasury"),
        creator_wallet_address: Addr::unchecked("creator_wallet"),
        commit_fee_bluechip: Decimal::percent(1), // 1%
        commit_fee_creator: Decimal::percent(5),   // 5%
    };
    COMMITFEEINFO.save(&mut deps.storage, &commit_fee_info).unwrap();

    let oracle_info = OracleInfo {
        oracle_addr: Addr::unchecked("oracle_contract"),
    };
    ORACLE_INFO.save(&mut deps.storage, &oracle_info).unwrap();

    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
    IS_THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NATIVE_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();
}

pub fn setup_pool_post_threshold(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    // First set up basic pool
    setup_pool_storage(deps);
    COMMITSTATUS.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap();
    // Mark threshold as hit
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap(); // $25k reached
    
    let pool_state = PoolState {
        pool_contract_address: Addr::unchecked("pool_contract"),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(23_500_000_000), // 23.5k bluechip (25k - 6% fees)
        reserve1: Uint128::new(350_000_000_000), // 350k creator tokens
        total_liquidity: Uint128::new(91_104_335_791), // sqrt(23.5k * 350k) ≈ 91k
        block_time_last: 1_600_000_000,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
}

#[test]
fn test_fee_calculation_after_swap() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(10_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let initial_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(initial_fee_state.fee_growth_global_0, Decimal::zero());
    
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("50").unwrap();
    fee_state.fee_growth_global_1 = Decimal::from_str("75").unwrap();
    fee_state.total_fees_collected_0 = Uint128::new(500_000);
    fee_state.total_fees_collected_1 = Uint128::new(750_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Collect fees
    let res = execute_collect_fees(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
    ).unwrap();
    
    // Verify fees were calculated correctly
    // fees_owed = liquidity * fee_growth_delta * multiplier
    // fees_owed = 10_000_000 * (50 - 0) * 1.0 = 500_000_000
    let fees_0_attr = res.attributes.iter().find(|a| a.key == "fees_0").unwrap();
    let fees_collected_0 = Uint128::from_str(&fees_0_attr.value).unwrap();
    
    println!("Fees collected: {}", fees_collected_0);
    assert_eq!(fees_collected_0, Uint128::new(500_000_000));
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.fee_growth_inside_0_last, Decimal::from_str("50").unwrap());
    assert_eq!(position.fee_growth_inside_1_last, Decimal::from_str("75").unwrap());
}

#[test]
fn test_multiple_positions_independent_fee_tracking() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "user1", Uint128::new(5_000_000));
    
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("100").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    create_test_position(&mut deps, 2, "user2", Uint128::new(5_000_000));
    
    // Manually set user2's position to have current fee growth as baseline
    let mut pos2 = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap();
    pos2.fee_growth_inside_0_last = Decimal::from_str("100").unwrap();
    LIQUIDITY_POSITIONS.save(&mut deps.storage, "2", &pos2).unwrap();
    
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("200").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "user1".to_string(), // Simplified - should check token_id
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    
    // User1 collects fees - should get fees from 0 to 200
    let info1 = mock_info("user1", &[]);
    let res1 = execute_collect_fees(deps.as_mut(), env.clone(), info1, "1".to_string()).unwrap();
    let fees_user1 = Uint128::from_str(
        &res1.attributes.iter().find(|a| a.key == "fees_0").unwrap().value
    ).unwrap();
    
    // User2 collects fees - should get fees from 100 to 200 only
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "user2".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let info2 = mock_info("user2", &[]);
    let res2 = execute_collect_fees(deps.as_mut(), env, info2, "2".to_string()).unwrap();
    let fees_user2 = Uint128::from_str(
        &res2.attributes.iter().find(|a| a.key == "fees_0").unwrap().value
    ).unwrap();
    
    println!("User1 fees (0→200): {}", fees_user1);
    println!("User2 fees (100→200): {}", fees_user2);
    
    // User1: 5M * (200 - 0) * 1.0 = 1,000,000,000
    assert_eq!(fees_user1, Uint128::new(1_000_000_000));
    
    // User2: 5M * (200 - 100) * 1.0 = 500,000,000
    assert_eq!(fees_user2, Uint128::new(500_000_000));
}

#[test]
fn test_remove_more_than_position_has() {
    // Ensures you can't remove more liquidity than exists
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Try to remove more than exists
    let msg = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::new(2_000_000), // More than 1M
        transaction_deadline: None,
        min_amount0: None,
        min_amount1: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::InsufficientLiquidity {} => (),
        _ => panic!("Expected InsufficientLiquidity error, got {:?}", err),
    }
}

#[test]
fn test_remove_zero_liquidity() {
    // Ensures zero removal is rejected
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let msg = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::zero(),
        transaction_deadline: None,
        min_amount0: None,
        min_amount1: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::InvalidAmount {} => (),
        _ => panic!("Expected InvalidAmount error, got {:?}", err),
    }
}

#[test]
fn test_position_ownership_transfer() {
    // Ensures position operations respect NFT ownership changes
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "original_owner", Uint128::new(1_000_000));
    
    // Initially owned by original_owner
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "original_owner".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    
    // Original owner can collect fees
    let info = mock_info("original_owner", &[]);
    let res = execute_collect_fees(deps.as_mut(), env.clone(), info, "1".to_string());
    assert!(res.is_ok());
    
    // Simulate NFT transfer to new_owner
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "new_owner".to_string(), // Ownership transferred
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let info_orig = mock_info("original_owner", &[]);
    let err = execute_collect_fees(deps.as_mut(), env.clone(), info_orig, "1".to_string());
    assert!(err.is_err());
    
    // New owner can now collect fees
    let info_new = mock_info("new_owner", &[]);
    let res = execute_collect_fees(deps.as_mut(), env, info_new, "1".to_string());
    assert!(res.is_ok());
}

#[test]
fn test_add_to_position_collects_fees_first() {
    // Verifies that add_to_position collects accumulated fees before adding liquidity
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("100").unwrap();
    fee_state.fee_growth_global_1 = Decimal::from_str("100").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: Uint128::new(500_000_000),
    }]);
    
    // Add more liquidity
    let res = execute_add_to_position(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        user,
        Uint128::new(500_000_000),
        Uint128::new(7_500_000_000),
        None,
        None,
        None,
    ).unwrap();
    
    // Verify fees were collected
    let fees_collected = res.attributes.iter()
        .find(|a| a.key == "fees_collected_0")
        .unwrap()
        .value
        .clone();
    
    assert_ne!(fees_collected, "0", "Fees should have been collected during add_to_position");
    
    // Verify fee tracking was reset
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.fee_growth_inside_0_last, Decimal::from_str("100").unwrap());
}

#[test]
fn test_transaction_deadline_enforcement() {
    // Ensures transaction deadline is properly enforced
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_700_000_000); // Current time
    
    let info = mock_info("liquidity_provider", &[]);
    
    // Set deadline in the past
    let past_deadline = Timestamp::from_seconds(1_600_000_000);
    
    let msg = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::new(500_000),
        transaction_deadline: Some(past_deadline),
        min_amount0: None,
        min_amount1: None,
    };
    
    let err = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap_err();
    match err {
        ContractError::TransactionExpired { .. } => (),
        _ => panic!("Expected TransactionExpired error, got {:?}", err),
    }
    
    // Set deadline in the future - should work
    let future_deadline = Timestamp::from_seconds(1_800_000_000);
    
    let msg2 = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::new(500_000),
        transaction_deadline: Some(future_deadline),
        min_amount0: None,
        min_amount1: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg2);
    assert!(res.is_ok());
}

#[test]
fn test_dust_position_low_fees() {
    // Verifies that dust positions get reduced fee multiplier
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create tiny dust position (100 liquidity, way below optimal)
    create_test_position(&mut deps, 1, "dust_provider", Uint128::new(100));
    
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    
    // Multiplier should be very low (close to MIN_MULTIPLIER)
    let expected_ratio = Decimal::from_ratio(100u128, OPTIMAL_LIQUIDITY);
    let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap();
    let expected_multiplier = min_mult + (Decimal::one() - min_mult) * expected_ratio;
    
    assert_eq!(position.fee_size_multiplier, expected_multiplier);
    assert!(position.fee_size_multiplier < Decimal::percent(20));
    
    // Simulate fee accrual
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("1000").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "dust_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("dust_provider", &[]);
    
    let res = execute_collect_fees(deps.as_mut(), env, info, "1".to_string()).unwrap();
    
    let fees = Uint128::from_str(
        &res.attributes.iter().find(|a| a.key == "fees_0").unwrap().value
    ).unwrap();
    
    // Fees should be heavily penalized
    // Normal: 100 * 1000 * 1.0 = 100,000
    // Dust: 100 * 1000 * ~0.1 = ~10,000
    println!("Dust position fees: {}", fees);
    assert!(fees < Uint128::new(20_000));
}

#[test]
fn test_refund_calculation_accuracy() {
    // Ensures refunds are calculated correctly for imbalanced deposits
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    
    // Send way too much bluechip
    let sent_bluechip = Uint128::new(10_000_000_000); // 10k
    let sent_token = Uint128::new(1_000_000_000); // 1k
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "bluechip".to_string(),
        amount: sent_bluechip,
    }]);
    
    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user,
        sent_bluechip,
        sent_token,
        None,
        None,
        None,
    ).unwrap();
    
    // Extract refund amount
    let refunded = Uint128::from_str(
        &res.attributes.iter().find(|a| a.key == "refunded_amount0").unwrap().value
    ).unwrap();
    
    let actual_used = Uint128::from_str(
        &res.attributes.iter().find(|a| a.key == "actual_amount0").unwrap().value
    ).unwrap();
    
    println!("Sent: {}", sent_bluechip);
    println!("Used: {}", actual_used);
    println!("Refunded: {}", refunded);
    
    // Verify math: sent = used + refunded
    assert_eq!(sent_bluechip, actual_used + refunded);
    assert!(refunded > Uint128::zero());
}

#[test]
fn test_pool_total_liquidity_consistency() {
    // Ensures pool total liquidity stays consistent across operations
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let initial_total = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    
    // Create first position
    create_test_position(&mut deps, 1, "user1", Uint128::new(1_000_000));
    let after_first = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert_eq!(after_first, initial_total + Uint128::new(1_000_000));
    
    // Create second position
    create_test_position(&mut deps, 2, "user2", Uint128::new(2_000_000));
    let after_second = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert_eq!(after_second, initial_total + Uint128::new(3_000_000));
    
    // Remove first position
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "user1".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("user1", &[]);
    execute_remove_all_liquidity(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        None,
        None,
        None,
    ).unwrap();
    
    let after_removal = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert_eq!(after_removal, initial_total + Uint128::new(2_000_000));
}

// Creates a test liquidity position with specified parameters
pub fn create_test_position(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    position_id: u64,
    owner: &str,
    liquidity: Uint128,
) {
    let position = Position {
        liquidity,
        owner: Addr::unchecked(owner),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: 1_600_000_000,
        last_fee_collection: 1_600_000_000,
        fee_size_multiplier: calculate_fee_size_multiplier(liquidity)
    };
    
    LIQUIDITY_POSITIONS.save(&mut deps.storage, &position_id.to_string(), &position).unwrap();
    
    POOL_STATE.update(&mut deps.storage, |mut state| -> Result<_, cosmwasm_std::StdError> {
        state.total_liquidity += liquidity;
        Ok(state)
    }).unwrap();
}

#[test]
    fn test_zero_liquidity_gets_minimum_multiplier() {
        let liquidity = Uint128::zero();
        let multiplier = calculate_fee_size_multiplier(liquidity);
        
        assert_eq!(multiplier, Decimal::from_str(MIN_MULTIPLIER).unwrap());
    }

    #[test]
    fn test_optimal_liquidity_gets_full_multiplier() {
        let liquidity = Uint128::new(OPTIMAL_LIQUIDITY);
        let multiplier = calculate_fee_size_multiplier(liquidity);
        
        assert_eq!(multiplier, Decimal::one());
    }

    #[test]
    fn test_above_optimal_liquidity_still_gets_full_multiplier() {
        // Test various amounts above optimal
        let test_cases = vec![
            OPTIMAL_LIQUIDITY + 1,
            OPTIMAL_LIQUIDITY * 2,
            OPTIMAL_LIQUIDITY * 10,
            OPTIMAL_LIQUIDITY * 1000,
        ];

        for liquidity_amount in test_cases {
            let liquidity = Uint128::new(liquidity_amount);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            assert_eq!(
                multiplier, 
                Decimal::one(),
                "Liquidity {} should get 100% multiplier",
                liquidity_amount
            );
        }
    }

      #[test]
    fn test_linear_scaling_between_min_and_optimal() {
        let liquidity_25_percent = Uint128::new(OPTIMAL_LIQUIDITY / 4);
        let multiplier_25 = calculate_fee_size_multiplier(liquidity_25_percent);
        let expected_25 = Decimal::from_str("0.325").unwrap(); // 0.1 + (0.9 * 0.25)
        assert_eq!(multiplier_25, expected_25);

        let liquidity_50_percent = Uint128::new(OPTIMAL_LIQUIDITY / 2);
        let multiplier_50 = calculate_fee_size_multiplier(liquidity_50_percent);
        let expected_50 = Decimal::from_str("0.55").unwrap(); // 0.1 + (0.9 * 0.5)
        assert_eq!(multiplier_50, expected_50);

        let liquidity_75_percent = Uint128::new(OPTIMAL_LIQUIDITY * 3 / 4);
        let multiplier_75 = calculate_fee_size_multiplier(liquidity_75_percent);
        let expected_75 = Decimal::from_str("0.775").unwrap(); // 0.1 + (0.9 * 0.75)
        assert_eq!(multiplier_75, expected_75);
    }

    #[test]
    fn test_dust_positions_get_heavily_penalized() {
        let dust_positions = vec![
            1u128,
            10u128,
            100u128,
            1000u128,
        ];

        for dust_amount in dust_positions {
            let liquidity = Uint128::new(dust_amount);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            let ratio = Decimal::from_ratio(dust_amount, OPTIMAL_LIQUIDITY);
            let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap();
            let expected = min_mult + (Decimal::one() - min_mult) * ratio;
            
            assert_eq!(
                multiplier, 
                expected,
                "Dust position {} should get correct multiplier",
                dust_amount
            );
            
            assert!(
                multiplier < Decimal::from_str("0.2").unwrap(),
                "Dust position {} should get less than 20% multiplier",
                dust_amount
            );
        }
    }

    #[test]
    fn test_specific_multiplier_values() {
        struct TestCase {
            liquidity: u128,
            expected_multiplier: &'static str,
            description: &'static str,
        }

        let test_cases = vec![
            TestCase {
                liquidity: 0,
                expected_multiplier: "0.1",
                description: "Zero liquidity",
            },
            TestCase {
                liquidity: 100_000,
                expected_multiplier: "0.19",
                description: "10% of optimal",
            },
            TestCase {
                liquidity: 200_000,
                expected_multiplier: "0.28",
                description: "20% of optimal",
            },
            TestCase {
                liquidity: 333_333,
                expected_multiplier: "0.399999",
                description: "~33% of optimal",
            },
            TestCase {
                liquidity: 500_000,
                expected_multiplier: "0.55",
                description: "50% of optimal",
            },
            TestCase {
                liquidity: 900_000,
                expected_multiplier: "0.91",
                description: "90% of optimal",
            },
            TestCase {
                liquidity: 999_999,
                expected_multiplier: "0.999999",
                description: "Just below optimal",
            },
            TestCase {
                liquidity: 1_000_000,
                expected_multiplier: "1",
                description: "Exactly optimal",
            },
            TestCase {
                liquidity: 10_000_000,
                expected_multiplier: "1",
                description: "10x optimal",
            },
        ];

        for test in test_cases {
            let liquidity = Uint128::new(test.liquidity);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            let expected = Decimal::from_str(test.expected_multiplier).unwrap();
            
            let diff = if multiplier > expected {
                multiplier - expected
            } else {
                expected - multiplier
            };
            
            assert!(
                diff < Decimal::from_str("0.000001").unwrap(),
                "{}: Expected multiplier ~{}, got {}",
                test.description,
                test.expected_multiplier,
                multiplier
            );
        }
    }

     #[test]
    fn test_multiplier_monotonically_increases() {
        let mut prev_multiplier = calculate_fee_size_multiplier(Uint128::zero());
        
        for i in 1..=100 {
            let liquidity = Uint128::new(OPTIMAL_LIQUIDITY * i / 100);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            assert!(
                multiplier >= prev_multiplier,
                "Multiplier should increase: {} -> {} at liquidity {}",
                prev_multiplier,
                multiplier,
                liquidity
            );
            
            prev_multiplier = multiplier;
        }
    }

       #[test]
    fn test_multiplier_bounds() {
        // Test many random values to ensure bounds are respected
        let test_values = vec![
            0, 1, 42, 137, 1337, 9999, 
            50_000, 123_456, 654_321, 999_999,
            1_000_000, 1_000_001, 2_000_000, 10_000_000,
            100_000_000, 1_000_000_000,
        ];

        let min_bound = Decimal::from_str(MIN_MULTIPLIER).unwrap();
        let max_bound = Decimal::one();

        for value in test_values {
            let liquidity = Uint128::new(value);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            assert!(
                multiplier >= min_bound,
                "Multiplier {} should be >= {} for liquidity {}",
                multiplier,
                min_bound,
                value
            );
            
            assert!(
                multiplier <= max_bound,
                "Multiplier {} should be <= {} for liquidity {}",
                multiplier,
                max_bound,
                value
            );
        }
    }

    #[test]
    fn test_edge_cases_near_optimal() {
        // Test values right around the optimal threshold
        let edge_cases = vec![
            (OPTIMAL_LIQUIDITY - 2, false),
            (OPTIMAL_LIQUIDITY - 1, false),
            (OPTIMAL_LIQUIDITY, true),
            (OPTIMAL_LIQUIDITY + 1, true),
            (OPTIMAL_LIQUIDITY + 2, true),
        ];

        for (liquidity_amount, should_be_full) in edge_cases {
            let liquidity = Uint128::new(liquidity_amount);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            if should_be_full {
                assert_eq!(
                    multiplier,
                    Decimal::one(),
                    "Liquidity {} should get full multiplier",
                    liquidity_amount
                );
            } else {
                assert!(
                    multiplier < Decimal::one(),
                    "Liquidity {} should get less than full multiplier",
                    liquidity_amount
                );
            }
        }
    }

    