
use std::str::FromStr;

use cosmwasm_std::{
    from_json, testing::{mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR}, to_json_binary, Addr, BankMsg, Binary, Coin, ContractResult, CosmosMsg, Decimal, Order, OwnedDeps, SystemError, SystemResult, Timestamp, Uint128, WasmMsg, WasmQuery
};
use cw20::Cw20ReceiveMsg;
use cw721::OwnerOfResponse;
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};
use crate::{asset::PoolPairType, contract::{execute, execute_swap_cw20, instantiate}, generic_helpers::trigger_threshold_payout, 

liquidity::{execute_add_to_position, execute_collect_fees, execute_deposit_liquidity, execute_remove_all_liquidity}, liquidity_helpers::calculate_fee_size_multiplier, msg::{Cw20HookMsg, CommitFeeInfo, PoolInstantiateMsg}, state::{CommitLimitInfo, OracleInfo, PoolFeeState, PoolInfo, PoolSpecs, PoolState, ThresholdPayoutAmounts, COMMITSTATUS, 
        COMMIT_LIMIT_INFO, COMMITFEEINFO, LIQUIDITY_POSITIONS, NATIVE_RAISED_FROM_COMMIT, ORACLE_INFO, POOL_INFO, POOL_SPECS, THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING
    }};
use crate::msg::ExecuteMsg;
use crate::state::{
    IS_THRESHOLD_HIT, USD_RAISED_FROM_COMMIT, COMMIT_LEDGER, RATE_LIMIT_GUARD,
    COMMIT_INFO, POOL_STATE, POOL_FEE_STATE, Position, NEXT_POSITION_ID, PoolDetails
};
use crate::error::ContractError;
use crate::asset::{TokenInfo, TokenType};
const OPTIMAL_LIQUIDITY: u128 = 1_000_000;
const MIN_MULTIPLIER: &str = "0.1";
fn mock_dependencies_with_balance(balances: &[Coin]) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    // Give the contract some balance
    deps.querier.update_balance(MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}
fn with_factory_oracle(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    bluechip_to_usd_rate: Uint128, // e.g., Uint128::new(1_000_000) for $1 per bluechip
) {
    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                // Check if it's a factory query
                if contract_addr == "factory_contract" {
                    // Try to parse as FactoryQueryMsg
                    if let Ok(factory_query) = from_json::<FactoryQueryMsg>(msg) {
                        match factory_query {
                            FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
                                let usd_amount = amount * bluechip_to_usd_rate / Uint128::new(1_000_000);
                                let response = ConversionResponse {
                                    amount: usd_amount,
                                    rate_used: bluechip_to_usd_rate,
                                    timestamp: 1_600_000_000,
                                };
                                return SystemResult::Ok(ContractResult::Ok(
                                    to_json_binary(&response).unwrap()
                                ));
                            }
                            FactoryQueryMsg::ConvertUsdToBluechip { amount } => {
                                let bluechip_amount = amount * Uint128::new(1_000_000) / bluechip_to_usd_rate;
                                let response = ConversionResponse {
                                    amount: bluechip_amount,
                                    rate_used: bluechip_to_usd_rate,
                                    timestamp: 1_600_000_000,
                                };
                                return SystemResult::Ok(ContractResult::Ok(
                                    to_json_binary(&response).unwrap()
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                
                // Handle NFT ownership queries
                if contract_addr == "nft_contract" {
                    // Your existing NFT mock logic
                }
                
                SystemResult::Err(SystemError::InvalidRequest {
                    error: "Unknown contract or query".to_string(),
                    request: msg.clone(),
                })
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
}
#[test]
fn test_commit_pre_threshold_basic() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let env = mock_env();
    let commit_amount = Uint128::new(1_000_000_000); // 1k bluechip
with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals
    
    let info = mock_info("user1", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    
    // Verify fees were sent (1% bluechip, 5% creator)
    assert_eq!(res.messages.len(), 2); // Two fee transfers
    
    // Verify commit was recorded in USD
    let user_addr = Addr::unchecked("user1");
    let user_commit_usd = COMMIT_LEDGER.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(user_commit_usd, Uint128::new(1_000_000_000)); // $1k with 6 decimals
    
    // Verify USD raised updated
    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(1_000_000_000));
    
    // Verify threshold not hit
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), false);
    
    let commiting = COMMIT_INFO.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(commiting.total_paid_bluechip, commit_amount);
    assert_eq!(commiting.total_paid_usd, Uint128::new(1_000_000_000));
    
}

#[test]
fn test_race_condition_commits_crossing_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(20_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();

    // Just below threshold
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(24_900_000_000)).unwrap();

    // Mock oracle: $1 per token
   with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let commit_amount = Uint128::new(200_000_000); // $200 per commit
    let env = mock_env();

    // -------- First Commit --------
    let info1 = mock_info("alice", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    // Run first commit
    let res1 = execute(deps.as_mut(), env.clone(), info1, msg1).unwrap();
    println!(
        "[Commit 1] USD_RAISED_FROM_COMMIT: {}, IS_THRESHOLD_HIT: {}, THRESHOLD_PROCESSING: {}, Attributes: {:?}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        res1.attributes
    );

    assert!(res1.attributes.iter().any(|a| a.value == "threshold_crossing"));
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    // --- Simulate race: threshold processing still TRUE ---
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();
    println!(
        "Simulated race -> USD_RAISED_FROM_COMMIT: {}, IS_THRESHOLD_HIT: {}, THRESHOLD_PROCESSING: {}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap()
    );
    // -------- Second Commit (same block) --------
    let info2 = mock_info("bob", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
let msg2 = ExecuteMsg::Commit {
    asset: TokenInfo {
        info: TokenType::Bluechip { denom: "stake".to_string() },
        amount: commit_amount,
    },
    amount: commit_amount,
    transaction_deadline: None,
    belief_price: None,
    max_spread: Some(Decimal::percent(99)),
};
    let res2 = execute(deps.as_mut(), env.clone(), info2, msg2).unwrap();
    println!(
        "[Commit 2] USD_RAISED_FROM_COMMIT: {}, IS_THRESHOLD_HIT: {}, THRESHOLD_PROCESSING: {}, Attributes: {:?}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        res2.attributes
    );

    assert!(
        res2.attributes.iter().all(|a| a.value != "threshold_crossing"),
        "Second commit should not run threshold logic while THRESHOLD_PROCESSING is true"
    );
    // Second commit should NOT trigger threshold crossing
    assert!(
        res2.attributes.iter().all(|a| a.value != "threshold_crossing"),
        "Second commit should not run threshold logic while THRESHOLD_PROCESSING is true"
    );

    // At the end, reset processing flag manually for cleanup
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
}


#[test]
fn test_commit_crosses_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(10_000_000_000), // 10k tokens
    }]);
    
    setup_pool_storage(&mut deps);
    
    // CRITICAL: Initialize the new threshold processing flag
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
    
    // Set USD raised to just below threshold
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(24_900_000_000)).unwrap(); // $24.9k
    
    // Also need to set up COMMIT_LIMIT_INFO if not in setup_pool_storage
    
    
    let env = mock_env();
    let commit_amount = Uint128::new(200_000_000); // 200 tokens = $200
    
    // Mock oracle response for $1 per token
    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals
    let info = mock_info("whale", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify threshold was hit
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    
    // Verify threshold processing flag was cleared
    assert_eq!(THRESHOLD_PROCESSING.load(&deps.storage).unwrap(), false);
    
    // Check for threshold crossing attribute
    assert!(res.attributes.iter().any(|attr| 
        attr.key == "phase" && attr.value == "threshold_crossing"
    ));
    
    // Verify multiple messages were sent
    // Should have: 2 fee transfers + token mints + bluechip seed transfer
    assert!(res.messages.len() >= 6, "Expected at least 6 messages, got {}", res.messages.len());
    
    // Verify pool state was initialized
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.total_liquidity, Uint128::zero()); // Unowned seed liquidity
    
    // Verify commit ledger was cleared
    assert_eq!(COMMIT_LEDGER.keys(&deps.storage, None, None, Order::Ascending).count(), 0);
}

#[test]
fn test_commit_post_threshold_swap() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let commit_amount = Uint128::new(100_000_000); // 100 bluechip
    
    // Mock oracle response
   with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals
    
    let info = mock_info("commiter", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify it performed a swap (fees + CW20 transfer)
    assert!(res.messages.len() >= 3); // 2 fees + 1 CW20 transfer
    
    // Verify pool reserves updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Increased from commit
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // Decreased from swap
    
    // Verify fee growth updated
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_0 > Decimal::zero());
    assert!(fee_state.total_fees_collected_0 > Uint128::zero());
}

#[test]
fn test_threshold_payout_integrity_check() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    
    // Corrupt the threshold payout state
    let mut bad_payout = THRESHOLD_PAYOUT_AMOUNTS.load(&deps.storage).unwrap();
    bad_payout.creator_reward_amount = Uint128::new(999_999_999_999); // Wrong!
    THRESHOLD_PAYOUT_AMOUNTS.save(&mut deps.storage, &bad_payout).unwrap();
    
    // Try to trigger threshold
    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    let commit_config = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
    let fee_info = COMMITFEEINFO.load(&deps.storage).unwrap();
    
    let result = trigger_threshold_payout(
        &mut deps.storage,
        &pool_info,
        &mut pool_state,
        &mut pool_fee_state,
        &commit_config,
        &bad_payout,
        &fee_info,
        &mock_env(),
    );
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("corruption"));
}

#[test]
fn test_commit_reentrancy_protection() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    
    // Set reentrancy guard
    RATE_LIMIT_GUARD.save(&mut deps.storage, &true).unwrap();
    
    let env = mock_env();
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::ReentrancyGuard {} => (),
        _ => panic!("Expected ReentrancyGuard error"),
    }
}

#[test]
fn test_commit_rate_limiting() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let mut env = mock_env();
    let user = Addr::unchecked("user");
    
    // First commit succeeds
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    execute(deps.as_mut(), env.clone(), info.clone(), msg.clone()).unwrap();
    
    // Second commit too soon should fail
    env.block.time = env.block.time.plus_seconds(30); // Only 30 seconds later
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::TooFrequentCommits { wait_time } => {
            assert_eq!(wait_time, 30); // Should wait 30 more seconds (60 total - 30 elapsed)
        },
        _ => panic!("Expected TooFrequentCommits error"),
    }
}

#[test]
fn test_commit_with_deadline() {
     let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_000_000);
    
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    // Set deadline in the past
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        transaction_deadline: Some(Timestamp::from_seconds(999_999)),
        belief_price: None,
        max_spread: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::TransactionExpired {} => (),
        _ => panic!("Expected DeadlineExceeded error"),
    }
}

// ============= SWAP TESTS =============

#[test]
fn test_simple_swap_bluechip_to_cw20() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // 1k bluechip
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: swap_amount,
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string()},
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify swap executed
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "swap");
    
    // Verify reserves updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Native increased
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // CW20 decreased
    
    // Verify fee growth
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_0 > Decimal::zero());
}

#[test]
fn test_swap_with_max_spread() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k bluechip (large swap)
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: swap_amount,
    }]);
    
    // Set very tight max spread (0.1%)
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: Some(Decimal::permille(1)), // 0.1%
        to: None,
        transaction_deadline: None,
    };
    
    // Large swap should exceed max spread
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion {} => (),
        _ => panic!("Expected MaxSpreadAssertion error"),
    }
}

#[test]
fn test_commit_threshold_overshoot_split() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000), // Plenty for all operations
    }]);
    
    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
    
    // Set USD raised to just below threshold
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(24_999_000_000)).unwrap(); // $24,999
    
    let env = mock_env();

// Mock factory oracle at $1 per bluechip
with_factory_oracle(&mut deps, Uint128::new(1_000_000));

let commit_amount = Uint128::new(5_000_000);
    
    let info = mock_info("whale", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();
        println!("\n=== Response Attributes ===");
    for attr in &res.attributes {
        println!("{}: {}", attr.key, attr.value);
    }

       println!("\n=== All Messages ({} total) ===", res.messages.len());
    for (i, submsg) in res.messages.iter().enumerate() {
        match &submsg.msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
                println!("Message {}: Bank Send to {} amount {:?}", i, to_address, amount);
            }
            CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
                println!("Message {}: Wasm Execute to {} with msg: {}", i, contract_addr, 
                    String::from_utf8_lossy(msg.as_slice()));
            }
            _ => println!("Message {}: Other type", i),
        }
    }

     let has_transfer = res.messages.iter().any(|submsg| {
        if let CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) = &submsg.msg {
            let msg_str = String::from_utf8_lossy(msg.as_slice());
            msg_str.contains("transfer")
        } else {
            false
        }
    });
   let binding = "0".to_string();
   let return_amt_str = res.attributes.iter()
        .find(|a| a.key == "bluechip_excess_returned")
        .map(|a| &a.value)
        .unwrap_or(&binding);
    println!("Return amount from attributes: {}", return_amt_str);
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
       let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!("\n=== Pool State After ===");
    println!("reserve0: {}", pool_state.reserve0);
    println!("reserve1: {}", pool_state.reserve1);
    assert_eq!(USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(), Uint128::new(25_000_000_000));
    
    assert!(COMMIT_LEDGER.load(&deps.storage, &info.sender).is_err());
    
    let attrs = &res.attributes;
    assert_eq!(attrs.iter().find(|a| a.key == "phase").unwrap().value, "threshold_crossing");
    assert_eq!(attrs.iter().find(|a| a.key == "threshold_amount_usd").unwrap().value, "1000000");
    assert_eq!(attrs.iter().find(|a| a.key == "swap_amount_usd").unwrap().value, "4000000");
    let bluechip_excess = attrs.iter().find(|a| a.key == "swap_amount_bluechip").unwrap().value.clone();
    let return_amt = attrs.iter().find(|a| a.key == "bluechip_excess_returned").unwrap().value.clone();
    
    println!("\n=== Swap Details ===");
    println!("Native excess to swap: {}", bluechip_excess);
    println!("CW20 returned: {}", return_amt);
    let sub = COMMIT_INFO.load(&deps.storage, &info.sender).unwrap();
    assert_eq!(sub.total_paid_bluechip, commit_amount); // Full 5 tokens
    assert_eq!(sub.total_paid_usd, Uint128::new(5_000_000)); // Full $5
  
        if has_transfer {
        println!("SUCCESS: CW20 transfer found!");
    } else {
        println!("ISSUE: No CW20 transfer found despite return_amt = {}", return_amt_str);
    }
}


#[test]
fn test_commit_exact_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    
    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
    // Set USD raised to need exactly $1 more
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(24_999_000_000)).unwrap();
    
    // add previous commits to simulate the 24,999
    let previous_user = Addr::unchecked("previous_user");
    COMMIT_LEDGER.save(&mut deps.storage, &previous_user, &Uint128::new(24_999_000_000)).unwrap();
    
   let env = mock_env();

// Mock factory oracle responses
with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip

// Commit exactly $1
let commit_amount = Uint128::new(1_000_000);
    
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };
    
    let res = execute(deps.as_mut(), env, info.clone(), msg).unwrap();
    
    // Should be a normal funding phase commit that triggers threshold
    assert_eq!(res.attributes.iter().find(|a| a.key == "phase").unwrap().value, "threshold_hit_exact");
    
    // Verify threshold hit
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    // verify that the total USD raised is at the threshold
    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(25_000_000_000)); // Should be exactly at $25k threshold
}
#[test]
fn test_swap_cw20_via_hook() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Mock CW20 balance query for the pool contract
    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "token_contract" {
                    // Parse the query to check if it's a balance query
                    if msg.to_string().contains("balance") {
                        let balance_response = cw20::BalanceResponse {
                            balance: Uint128::new(350_000_000_000), // Pool has 350k tokens
                        };
                        SystemResult::Ok(ContractResult::Ok(
                            to_json_binary(&balance_response).unwrap()
                        ))
                    } else {
                        SystemResult::Err(SystemError::InvalidRequest {
                            error: "Unknown query".to_string(),
                            request: msg.clone(),
                        })
                    }
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
    let swap_amount = Uint128::new(10_000_000_000); // 10k tokens
    
    // Message from CW20 token contract
    let info = mock_info("token_contract", &[]);
    
    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(10)), // Allow spread
            to: None,
            transaction_deadline: None,
        }).unwrap(),
    };
    
    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();
    
    // Verify swap executed
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "swap");
    
    // Verify reserves updated (opposite direction from bluechip swap)
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(23_500_000_000)); // Native decreased
    assert!(pool_state.reserve1 > Uint128::new(350_000_000_000)); // CW20 increased
}

#[test]
fn test_swap_wrong_asset() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let info = mock_info("trader", &[Coin {
        denom: "wrong_token".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "wrong_token".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::AssetMismatch {} => (),
        _ => panic!("Expected AssetMismatch error"),
    }
}

#[test]
fn test_swap_price_accumulator_update() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_600_001_000); // 1000 seconds later
    
    let initial_state = POOL_STATE.load(&deps.storage).unwrap();
    let initial_price0 = initial_state.price0_cumulative_last;
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    
    execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    
    // Verify price accumulator updated
    let updated_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(updated_state.price0_cumulative_last > initial_price0);
    assert_eq!(updated_state.block_time_last, env.block.time.seconds());
}

#[test]
fn test_deposit_liquidity_first_position() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let bluechip_amount = Uint128::new(1_000_000_000); // 1k bluechip
    let token_amount = Uint128::new(14_893_617_021); // Approximately correct ratio
    
    // User sends bluechip tokens with the message
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: bluechip_amount,
    }]);
    
    // Call execute_deposit_liquidity directly
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
    
    // Verify NFT mint message sent
    assert!(res.messages.iter().any(|msg| {
        matches!(&msg.msg, CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) 
            if contract_addr == "nft_contract")
    }));
    
    // Verify position created
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap(); // ID starts at 1, increments to 2
    assert_eq!(position.owner, user);
    assert!(position.liquidity > Uint128::zero());
    
    // Verify pool state updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.total_liquidity > Uint128::new(91_104_335_791)); // Initial + new
    
    // Verify next position ID incremented
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
        denom: "stake".to_string(),
        amount: bluechip_amount,
    }]);
    
    // Set minimum amounts for slippage protection
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
    
    // Create initial position
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Mock NFT ownership check - the user owns position NFT #1
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
        denom: "stake".to_string(),
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
    
    // Verify position liquidity increased
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(position.liquidity > Uint128::new(1_000_000));
    
    // Verify action
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
        denom: "stake".to_string(),
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
    
    // Mock NFT ownership
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
    
    // Simulate fee accrual
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
    
    // Verify position fee growth updated
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.fee_growth_inside_0_last, fee_state.fee_growth_global_0);
    assert_eq!(position.fee_growth_inside_1_last, fee_state.fee_growth_global_1);
}
#[test]
fn test_remove_all_liquidity() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Store initial liquidity for comparison
    let initial_liquidity = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    
    // Create position (this will increase total liquidity)
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Verify liquidity increased
    let after_add = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert!(after_add > initial_liquidity);
    
    // Mock NFT ownership
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
    
    // Verify assets returned (bluechip + CW20 transfers)
    assert!(res.messages.len() >= 2);
    
    // Verify position removed
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
        denom: "stake".to_string(),
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
    
    // Should have refund message for excess bluechip tokens
    let refund_msg = res.messages.iter().find(|msg| {
        matches!(&msg.msg, CosmosMsg::Bank(BankMsg::Send { .. }))
    });
    assert!(refund_msg.is_some());
    
    // Check refund amount in attributes
    let refund_attr = res.attributes.iter().find(|a| a.key == "refunded_amount0").unwrap();
    assert!(Uint128::new(refund_attr.value.parse::<u128>().unwrap()) > Uint128::zero());
}

#[test]
fn test_remove_liquidity_with_slippage_protection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position
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
    
    // Manipulate pool to cause slippage
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
    
    // Verify partial removal
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.liquidity, Uint128::new(700_000)); // 1M - 300k
    
    // Verify proportional fee collection
    assert!(res.messages.len() >= 2); // Asset returns
}

#[test]
fn test_remove_partial_liquidity_by_percent() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position with 1M liquidity
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Store initial pool state
    let initial_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    
    // Mock NFT ownership
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
    
    // Remove 25% of liquidity
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
    
    // Verify liquidity removed attribute
    assert_eq!(
        res.attributes.iter().find(|a| a.key == "liquidity_removed").unwrap().value, 
        "250000" // 25% of 1M
    );
    
    // Verify pool total liquidity decreased by 25% of position
    let final_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        final_pool_state.total_liquidity,
        initial_pool_state.total_liquidity - Uint128::new(250_000)
    );
    
    // Verify transfer messages were created (bluechip + CW20)
    assert!(res.messages.len() >= 2);
}
// ============= EDGE CASE TESTS =============

#[test]
fn test_zero_liquidity_fee_collection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Set pool to zero liquidity
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.total_liquidity = Uint128::zero();
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    
    // Try to update fee growth (should not panic)
    let env = mock_env();
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    
    // Should execute without updating fee growth
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Fee growth should remain zero
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(fee_state.fee_growth_global_0, Decimal::zero());
}

#[test]
fn test_price_accumulator_zero_reserves() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps); // Pre-threshold, zero reserves
    
    // Mark as post-threshold but keep zero reserves
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(1000);
    
    // This should not panic with zero reserves
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::zero());
    assert_eq!(pool_state.reserve1, Uint128::zero());
    
    // Price accumulator should not update with zero reserves
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
    
    // Should succeed but no transfer messages
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
    // Set up PoolInfo
    let pool_info = PoolInfo {
        pool_id: 1u64,
        pool_info: PoolDetails {
            asset_infos: [
                TokenType::Bluechip {
                    denom: "stake".to_string(), // Using "stake" as bluechip token
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

    // Set up PoolState - Pre-threshold (no liquidity yet)
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

    // Set up PoolFeeState
    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOL_FEE_STATE.save(&mut deps.storage, &pool_fee_state).unwrap();

    // Set up PoolSpecs
    let pool_specs = PoolSpecs {
        lp_fee: Decimal::percent(3) / Uint128::new(10), // 0.3% fee (3/1000)
        min_commit_interval: 60, // 1 minute minimum between commits
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    // Set up CommitInfo
    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(100_000_000), // 100 bluechip tokens
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
    };
    COMMIT_LIMIT_INFO.save(&mut deps.storage, &commit_config).unwrap();

    // Set up ThresholdPayout
    let threshold_payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000), // 325k tokens
        bluechip_reward_amount: Uint128::new(25_000_000_000), // 25k tokens
        pool_seed_amount: Uint128::new(350_000_000_000),     // 350k tokens
        commit_return_amount: Uint128::new(500_000_000_000),   // 500k tokens
    };
    THRESHOLD_PAYOUT_AMOUNTS.save(&mut deps.storage, &threshold_payout).unwrap();

    // Set up FeeInfo
    let commit_fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip_treasury"),
        creator_wallet_address: Addr::unchecked("creator_wallet"),
        commit_fee_bluechip: Decimal::percent(1), // 1%
        commit_fee_creator: Decimal::percent(5),   // 5%
    };
    COMMITFEEINFO.save(&mut deps.storage, &commit_fee_info).unwrap();

    // Set up OracleInfo
    let oracle_info = OracleInfo {
        oracle_addr: Addr::unchecked("oracle_contract"),
    };
    ORACLE_INFO.save(&mut deps.storage, &oracle_info).unwrap();

    // Initialize other state variables
    THRESHOLD_PROCESSING.save(&mut deps.storage, &false).unwrap();
    IS_THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NATIVE_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();
}

// Sets up a pool in post-threshold state with initial liquidity
pub fn setup_pool_post_threshold(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    // First set up basic pool
    setup_pool_storage(deps);
    COMMITSTATUS.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap();
    // Mark threshold as hit
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap(); // $25k reached
    
    // Update pool state with initial liquidity
    // Initial liquidity: 23.5k bluechip (25k - fees) and 350k creator tokens
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
fn test_factory_impersonation_prevented() {
    let mut deps = mock_dependencies();
    
    // Try to instantiate from non-factory address
      let msg = PoolInstantiateMsg {
        pool_id: 1u64,
        pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken{
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
        cw20_token_contract_id: 2u64,
        threshold_payout: None,
        used_factory_addr: Addr::unchecked("factory_contract"),
        commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("bluechip"),
                creator_wallet_address: Addr::unchecked("addr0000"),
                commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
                commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
            },
        commit_amount_for_threshold: Uint128::new(0),
        commit_threshold_limit_usd: Uint128::new(350_000_000_000),
        position_nft_address: Addr::unchecked("NFT_contract"),
        token_address: Addr::unchecked("token_contract"),
    };
    let info = mock_info("fake_factory", &[]); // Wrong sender!
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();
    
    match err {
        ContractError::Unauthorized {} => (),
        _ => panic!("Expected Unauthorized error"),
    }
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
        fee_size_multiplier: Decimal::percent(1)
    };
    
    LIQUIDITY_POSITIONS.save(&mut deps.storage, &position_id.to_string(), &position).unwrap();
    
    // Also update the pool's total liquidity
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
        // Test 25% of optimal liquidity
        let liquidity_25_percent = Uint128::new(OPTIMAL_LIQUIDITY / 4);
        let multiplier_25 = calculate_fee_size_multiplier(liquidity_25_percent);
        let expected_25 = Decimal::from_str("0.325").unwrap(); // 0.1 + (0.9 * 0.25)
        assert_eq!(multiplier_25, expected_25);

        // Test 50% of optimal liquidity
        let liquidity_50_percent = Uint128::new(OPTIMAL_LIQUIDITY / 2);
        let multiplier_50 = calculate_fee_size_multiplier(liquidity_50_percent);
        let expected_50 = Decimal::from_str("0.55").unwrap(); // 0.1 + (0.9 * 0.5)
        assert_eq!(multiplier_50, expected_50);

        // Test 75% of optimal liquidity
        let liquidity_75_percent = Uint128::new(OPTIMAL_LIQUIDITY * 3 / 4);
        let multiplier_75 = calculate_fee_size_multiplier(liquidity_75_percent);
        let expected_75 = Decimal::from_str("0.775").unwrap(); // 0.1 + (0.9 * 0.75)
        assert_eq!(multiplier_75, expected_75);
    }

    #[test]
    fn test_dust_positions_get_heavily_penalized() {
        // Test tiny positions
        let dust_positions = vec![
            1u128,
            10u128,
            100u128,
            1000u128,
        ];

        for dust_amount in dust_positions {
            let liquidity = Uint128::new(dust_amount);
            let multiplier = calculate_fee_size_multiplier(liquidity);
            
            // Calculate expected multiplier
            let ratio = Decimal::from_ratio(dust_amount, OPTIMAL_LIQUIDITY);
            let min_mult = Decimal::from_str(MIN_MULTIPLIER).unwrap();
            let expected = min_mult + (Decimal::one() - min_mult) * ratio;
            
            assert_eq!(
                multiplier, 
                expected,
                "Dust position {} should get correct multiplier",
                dust_amount
            );
            
            // Verify it's significantly less than 100%
            assert!(
                multiplier < Decimal::from_str("0.2").unwrap(),
                "Dust position {} should get less than 20% multiplier",
                dust_amount
            );
        }
    }

    #[test]
    fn test_specific_multiplier_values() {
        // Test specific values for documentation/reference
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
            
            // Use approximate equality for floating point precision
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
        // Ensure multiplier always increases with liquidity up to optimal
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