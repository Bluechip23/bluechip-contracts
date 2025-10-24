use crate::asset::{TokenInfo, TokenType};
use crate::error::ContractError;
use crate::msg::ExecuteMsg;
use crate::state::{
    COMMIT_INFO, COMMIT_LEDGER, IS_THRESHOLD_HIT, POOL_FEE_STATE, POOL_STATE, RATE_LIMIT_GUARD,
    USD_RAISED_FROM_COMMIT,
};
use crate::{
    contract::{execute, execute_swap_cw20, instantiate},
    generic_helpers::trigger_threshold_payout,
    msg::{CommitFeeInfo, Cw20HookMsg, PoolInstantiateMsg},
    state::{
        DistributionState, COMMITFEEINFO, COMMIT_LIMIT_INFO, DISTRIBUTION_STATE, POOL_INFO,
        THRESHOLD_PAYOUT_AMOUNTS, THRESHOLD_PROCESSING,
    },
    testing::liquidity_tests::{setup_pool_post_threshold, setup_pool_storage},
};
use cosmwasm_std::{
    from_json,
    testing::{
        mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
        MOCK_CONTRACT_ADDR,
    },
    to_json_binary, Addr, BankMsg, Binary, Coin, ContractResult, CosmosMsg, Decimal, Order,
    OwnedDeps, SystemError, SystemResult, Timestamp, Uint128, WasmMsg, WasmQuery,
};
use cw20::Cw20ReceiveMsg;
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};
fn mock_dependencies_with_balance(
    balances: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    // Give the contract some balance
    deps.querier
        .update_balance(MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}
fn with_factory_oracle(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    bluechip_to_usd_rate: Uint128,
) {
    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "factory_contract" {
                    if let Ok(factory_query) = from_json::<FactoryQueryMsg>(msg) {
                        match factory_query {
                            FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
                                // Handle multiplication and division separately
                                let intermediate = match amount.checked_mul(bluechip_to_usd_rate) {
                                    Ok(v) => v,
                                    Err(_) => {
                                        return SystemResult::Err(SystemError::InvalidRequest {
                                            error: "Overflow in mock oracle calculation"
                                                .to_string(),
                                            request: msg.clone(),
                                        });
                                    }
                                };

                                let usd_amount = match intermediate
                                    .checked_div(Uint128::new(1_000_000))
                                {
                                    Ok(v) => v,
                                    Err(_) => {
                                        return SystemResult::Err(SystemError::InvalidRequest {
                                            error: "Division error in mock oracle calculation"
                                                .to_string(),
                                            request: msg.clone(),
                                        });
                                    }
                                };

                                let response = ConversionResponse {
                                    amount: usd_amount,
                                    rate_used: bluechip_to_usd_rate,
                                    timestamp: 1_600_000_000,
                                };
                                return SystemResult::Ok(ContractResult::Ok(
                                    to_json_binary(&response).unwrap(),
                                ));
                            }
                            FactoryQueryMsg::ConvertUsdToBluechip { amount } => {
                                let intermediate = match amount.checked_mul(Uint128::new(1_000_000))
                                {
                                    Ok(v) => v,
                                    Err(_) => {
                                        return SystemResult::Err(SystemError::InvalidRequest {
                                            error: "Overflow in mock oracle calculation"
                                                .to_string(),
                                            request: msg.clone(),
                                        });
                                    }
                                };

                                let bluechip_amount = match intermediate
                                    .checked_div(bluechip_to_usd_rate)
                                {
                                    Ok(v) => v,
                                    Err(_) => {
                                        return SystemResult::Err(SystemError::InvalidRequest {
                                            error: "Division error in mock oracle calculation"
                                                .to_string(),
                                            request: msg.clone(),
                                        });
                                    }
                                };

                                let response = ConversionResponse {
                                    amount: bluechip_amount,
                                    rate_used: bluechip_to_usd_rate,
                                    timestamp: 1_600_000_000,
                                };
                                return SystemResult::Ok(ContractResult::Ok(
                                    to_json_binary(&response).unwrap(),
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

    let info = mock_info(
        "user1",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    // Just below threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();

    // Mock oracle: $1 per token
    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let commit_amount = Uint128::new(200_000_000); // $200 per commit
    let env = mock_env();

    // -------- First Commit --------
    let info1 = mock_info(
        "alice",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );
    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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

    assert!(res1
        .attributes
        .iter()
        .any(|a| a.value == "threshold_crossing"));
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
    let info2 = mock_info(
        "bob",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );
    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
        res2.attributes
            .iter()
            .all(|a| a.value != "threshold_crossing"),
        "Second commit should not run threshold logic while THRESHOLD_PROCESSING is true"
    );
    // Second commit should NOT trigger threshold crossing
    assert!(
        res2.attributes
            .iter()
            .all(|a| a.value != "threshold_crossing"),
        "Second commit should not run threshold logic while THRESHOLD_PROCESSING is true"
    );

    // At the end, reset processing flag manually for cleanup
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();
}

#[test]
fn test_commit_crosses_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(10_000_000_000), // 10k tokens
    }]);

    setup_pool_storage(&mut deps);

    // CRITICAL: Initialize the new threshold processing flag
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    // Set USD raised to just below threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap(); // $24.9k

    // Also need to set up COMMIT_LIMIT_INFO if not in setup_pool_storage

    let env = mock_env();
    let commit_amount = Uint128::new(200_000_000); // 200 tokens = $200

    // Mock oracle response for $1 per token
    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals
    let info = mock_info(
        "whale",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "phase" && attr.value == "threshold_crossing"));

    // Verify multiple messages were sent
    // Should have: 2 fee transfers + token mints + bluechip seed transfer
    assert!(
        res.messages.len() >= 6,
        "Expected at least 6 messages, got {}",
        res.messages.len()
    );

    // Verify pool state was initialized
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.total_liquidity, Uint128::zero()); // Unowned seed liquidity

    // Verify commit ledger was cleared
    assert_eq!(
        COMMIT_LEDGER
            .keys(&deps.storage, None, None, Order::Ascending)
            .count(),
        0
    );
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

    let info = mock_info(
        "commiter",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
    let mut bad_payout = THRESHOLD_PAYOUT_AMOUNTS
        .load(&deps.storage)
        .expect("failed to load payout");
    bad_payout.creator_reward_amount = Uint128::new(999_999_999_999); // Wrong total!
    THRESHOLD_PAYOUT_AMOUNTS
        .save(&mut deps.storage, &bad_payout)
        .expect("failed to save payout");

    // Prepare required pool state structs
    let pool_info = POOL_INFO.load(&deps.storage).expect("pool_info");
    let mut pool_state = POOL_STATE.load(&deps.storage).expect("pool_state");
    let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).expect("pool_fee_state");
    let commit_config = COMMIT_LIMIT_INFO
        .load(&deps.storage)
        .expect("commit_config");
    let fee_info = COMMITFEEINFO.load(&deps.storage).expect("fee_info");
    let env = mock_env();

    // Trigger threshold payout — should fail the integrity check
    let result = trigger_threshold_payout(
        &mut deps.storage,
        &pool_info,
        &mut pool_state,
        &mut pool_fee_state,
        &commit_config,
        &bad_payout,
        &fee_info,
        &env,
    );

    // Validate failure
    assert!(result.is_err(), "expected integrity check failure");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("corruption"),
        "unexpected error message: {}",
        err_msg
    );
}

#[test]
fn test_continue_distribution_rejects_external_call() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Simulate an active distribution
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000_000),
        total_committed_usd: Uint128::new(1_000_000_000),
        last_processed_key: None,
        distributions_remaining: 10,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    // Try to call as an external user
    let msg = ExecuteMsg::ContinueDistribution {};
    let info = mock_info("random_user", &[]);

    let res = execute(deps.as_mut(), mock_env(), info, msg);

    // Should be unauthorized
    assert!(res.is_err());
    assert!(
        matches!(res.unwrap_err(), ContractError::Unauthorized {}),
        "Expected Unauthorized error"
    );
}

#[test]
fn test_continue_distribution_internal_self_call_succeeds() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Active distribution
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000_000),
        total_committed_usd: Uint128::new(1_000_000_000),
        last_processed_key: None,
        distributions_remaining: 5,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    // Simulate a call coming from the contract itself
    let env = mock_env();
    let info = mock_info(env.contract.address.as_str(), &[]);

    let msg = ExecuteMsg::ContinueDistribution {};
    let res = execute(deps.as_mut(), env, info, msg).expect("internal self-call should succeed");

    assert!(
        res.attributes
            .iter()
            .any(|a| a.value == "continue_distribution"),
        "Response should include continue_distribution attribute"
    );
}

#[test]
fn test_commit_reentrancy_protection() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Set reentrancy guard
    RATE_LIMIT_GUARD.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
    let info = mock_info(
        user.as_str(),
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
        }
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

    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    // Set deadline in the past
    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    // Verify swap executed
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );

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

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    // Set very tight max spread (0.1%)
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    // Set USD raised to just below threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap(); // $24,999

    let env = mock_env();

    // Mock factory oracle at $1 per bluechip
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let commit_amount = Uint128::new(5_000_000);

    let info = mock_info(
        "whale",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
                println!(
                    "Message {}: Bank Send to {} amount {:?}",
                    i, to_address, amount
                );
            }
            CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr, msg, ..
            }) => {
                println!(
                    "Message {}: Wasm Execute to {} with msg: {}",
                    i,
                    contract_addr,
                    String::from_utf8_lossy(msg.as_slice())
                );
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
    let return_amt_str = res
        .attributes
        .iter()
        .find(|a| a.key == "bluechip_excess_returned")
        .map(|a| &a.value)
        .unwrap_or(&binding);
    println!("Return amount from attributes: {}", return_amt_str);
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!("\n=== Pool State After ===");
    println!("reserve0: {}", pool_state.reserve0);
    println!("reserve1: {}", pool_state.reserve1);
    assert_eq!(
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        Uint128::new(25_000_000_000)
    );

    assert!(COMMIT_LEDGER.load(&deps.storage, &info.sender).is_err());

    let attrs = &res.attributes;
    assert_eq!(
        attrs.iter().find(|a| a.key == "phase").unwrap().value,
        "threshold_crossing"
    );
    assert_eq!(
        attrs
            .iter()
            .find(|a| a.key == "threshold_amount_usd")
            .unwrap()
            .value,
        "1000000"
    );
    assert_eq!(
        attrs
            .iter()
            .find(|a| a.key == "swap_amount_usd")
            .unwrap()
            .value,
        "4000000"
    );
    let bluechip_excess = attrs
        .iter()
        .find(|a| a.key == "swap_amount_bluechip")
        .unwrap()
        .value
        .clone();
    let return_amt = attrs
        .iter()
        .find(|a| a.key == "bluechip_excess_returned")
        .unwrap()
        .value
        .clone();

    println!("\n=== Swap Details ===");
    println!("Native excess to swap: {}", bluechip_excess);
    println!("CW20 returned: {}", return_amt);
    let sub = COMMIT_INFO.load(&deps.storage, &info.sender).unwrap();
    assert_eq!(sub.total_paid_bluechip, commit_amount); // Full 5 tokens
    assert_eq!(sub.total_paid_usd, Uint128::new(5_000_000)); // Full $5

    if has_transfer {
        println!("SUCCESS: CW20 transfer found!");
    } else {
        println!(
            "ISSUE: No CW20 transfer found despite return_amt = {}",
            return_amt_str
        );
    }
}

#[test]
fn test_commit_exact_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();
    // Set USD raised to need exactly $1 more
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap();

    // add previous commits to simulate the 24,999
    let previous_user = Addr::unchecked("previous_user");
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &previous_user,
            &Uint128::new(24_999_000_000),
        )
        .unwrap();

    let env = mock_env();

    // Mock factory oracle responses
    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip

    // Commit exactly $1
    let commit_amount = Uint128::new(1_000_000);

    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env, info.clone(), msg).unwrap();

    // Should be a normal funding phase commit that triggers threshold
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "phase")
            .unwrap()
            .value,
        "threshold_hit_exact"
    );

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
                            to_json_binary(&balance_response).unwrap(),
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
        })
        .unwrap(),
    };

    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();

    // Verify swap executed
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );

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
    let info = mock_info(
        "trader",
        &[Coin {
            denom: "wrong_token".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "wrong_token".to_string(),
            },
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

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
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
fn test_factory_impersonation_prevented() {
    let mut deps = mock_dependencies();

    // Try to instantiate from non-factory address
    let msg = PoolInstantiateMsg {
        pool_id: 1u64,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken {
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

#[test]
fn test_commit_with_changing_oracle_prices() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(10_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    // First commit at $1 per bluechip
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let env = mock_env();
    let info1 = mock_info(
        "user1",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(5_000_000),
        }],
    );

    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(5_000_000),
        },
        amount: Uint128::new(5_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info1, msg1).unwrap();

    let first_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(first_usd, Uint128::new(5_000_000)); // $5

    // Price doubles to $2 per bluechip
    with_factory_oracle(&mut deps, Uint128::new(2_000_000));

    let info2 = mock_info(
        "user2",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(5_000_000), // Same bluechip amount
        }],
    );

    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(5_000_000),
        },
        amount: Uint128::new(5_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env, info2, msg2).unwrap();

    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(15_000_000)); // $5 + $10 = $15

    // Verify second user paid $10
    let user2_commit = COMMIT_INFO
        .load(&deps.storage, &Addr::unchecked("user2"))
        .unwrap();
    assert_eq!(user2_commit.total_paid_usd, Uint128::new(10_000_000));
}

#[test]
fn test_threshold_crossing_depends_on_oracle_price() {
    // Scenario 1: High price means fewer tokens needed
    let mut deps1 = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps1);
    THRESHOLD_PROCESSING
        .save(&mut deps1.storage, &false)
        .unwrap();

    // Oracle at $10 per token
    with_factory_oracle(&mut deps1, Uint128::new(10_000_000));

    // Already at $24k
    USD_RAISED_FROM_COMMIT
        .save(&mut deps1.storage, &Uint128::new(24_000_000_000))
        .unwrap();

    let env = mock_env();

    // Commit 100 tokens at $10 each = $1k, should hit $25k threshold
    let info1 = mock_info(
        "whale",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(100_000_000), // 100 tokens
        }],
    );

    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        amount: Uint128::new(100_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps1.as_mut(), env.clone(), info1, msg1).unwrap();
    assert_eq!(IS_THRESHOLD_HIT.load(&deps1.storage).unwrap(), true);

    // Scenario 2: Low price means more tokens needed
    let mut deps2 = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps2);
    THRESHOLD_PROCESSING
        .save(&mut deps2.storage, &false)
        .unwrap();

    // Oracle at $0.10 per token
    with_factory_oracle(&mut deps2, Uint128::new(100_000)); // $0.10

    USD_RAISED_FROM_COMMIT
        .save(&mut deps2.storage, &Uint128::new(24_000_000_000))
        .unwrap();

    // Same 100 tokens but at $0.10 each = $10, should NOT hit threshold
    let info2 = mock_info(
        "whale",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(100_000_000),
        }],
    );

    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        amount: Uint128::new(100_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps2.as_mut(), env, info2, msg2).unwrap();
    assert_eq!(IS_THRESHOLD_HIT.load(&deps2.storage).unwrap(), false);

    let total = USD_RAISED_FROM_COMMIT.load(&deps2.storage).unwrap();
    assert_eq!(total, Uint128::new(24_010_000_000)); // $24k + $10
}

#[test]
fn test_oracle_conversion_precision_various_prices() {
    struct TestCase {
        oracle_price: Uint128,
        token_amount: Uint128,
        expected_usd: Uint128,
        description: &'static str,
    }

    let test_cases = vec![
        TestCase {
            oracle_price: Uint128::new(1_000_000), // $1
            token_amount: Uint128::new(1_000_000), // 1 token
            expected_usd: Uint128::new(1_000_000), // $1
            description: "$1 per token, 1 token",
        },
        TestCase {
            oracle_price: Uint128::new(500_000),   // $0.50
            token_amount: Uint128::new(2_000_000), // 2 tokens
            expected_usd: Uint128::new(1_000_000), // $1
            description: "$0.50 per token, 2 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(10_000_000), // $10
            token_amount: Uint128::new(100_000),    // 0.1 tokens
            expected_usd: Uint128::new(1_000_000),  // $1
            description: "$10 per token, 0.1 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(100_000),    // $0.10
            token_amount: Uint128::new(10_000_000), // 10 tokens
            expected_usd: Uint128::new(1_000_000),  // $1
            description: "$0.10 per token, 10 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(3_333_333), // $3.33...
            token_amount: Uint128::new(3_000_000), // 3 tokens
            expected_usd: Uint128::new(9_999_999), // ~$10
            description: "$3.33 per token, 3 tokens",
        },
    ];

    for test in test_cases {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "stake".to_string(),
            amount: test.token_amount,
        }]);
        setup_pool_storage(&mut deps);

        with_factory_oracle(&mut deps, test.oracle_price);

        let env = mock_env();
        let info = mock_info(
            "user",
            &[Coin {
                denom: "stake".to_string(),
                amount: test.token_amount,
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Bluechip {
                    denom: "stake".to_string(),
                },
                amount: test.token_amount,
            },
            amount: test.token_amount,
            transaction_deadline: None,
            belief_price: None,
            max_spread: None,
        };

        execute(deps.as_mut(), env, info, msg).unwrap();

        let recorded_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        let tolerance = Uint128::new(10); // Allow small rounding error

        assert!(
            recorded_usd >= test.expected_usd.saturating_sub(tolerance)
                && recorded_usd <= test.expected_usd + tolerance,
            "{}: expected ~{}, got {}",
            test.description,
            test.expected_usd,
            recorded_usd
        );
    }
}

#[test]
fn test_extreme_oracle_prices() {
    // Test very low price
    let mut deps_low = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000_000), // 1M tokens
    }]);
    setup_pool_storage(&mut deps_low);

    with_factory_oracle(&mut deps_low, Uint128::new(1_000)); // $0.001

    let env = mock_env();
    let info_low = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000_000), // 1k tokens
        }],
    );

    let msg_low = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(1_000_000_000),
        },
        amount: Uint128::new(1_000_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res_low = execute(deps_low.as_mut(), env.clone(), info_low, msg_low);
    assert!(res_low.is_ok(), "Should handle very low prices");

    let usd_low = USD_RAISED_FROM_COMMIT.load(&deps_low.storage).unwrap();
    assert_eq!(usd_low, Uint128::new(1_000_000)); // $1 (1k tokens * $0.001)

    // Test very high price
    let mut deps_high = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    setup_pool_storage(&mut deps_high);

    with_factory_oracle(&mut deps_high, Uint128::new(1_000_000_000)); // $1000

    let info_high = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000), // 1 token
        }],
    );

    let msg_high = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res_high = execute(deps_high.as_mut(), env, info_high, msg_high);
    assert!(res_high.is_ok(), "Should handle very high prices");

    let usd_high = USD_RAISED_FROM_COMMIT.load(&deps_high.storage).unwrap();
    assert_eq!(usd_high, Uint128::new(1_000_000_000)); // $1000
}

#[test]
fn test_usd_tracking_consistency_across_commits() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::new(2_500_000)); // $2.50 per token

    let env = mock_env();

    // Multiple commits
    let commits = vec![
        ("user1", 4_000_000u128), // 4 tokens * $2.50 = $10
        ("user2", 8_000_000u128), // 8 tokens * $2.50 = $20
        ("user3", 2_000_000u128), // 2 tokens * $2.50 = $5
    ];

    let mut expected_total = Uint128::zero();

    for (user, amount) in commits {
        let info = mock_info(
            user,
            &[Coin {
                denom: "stake".to_string(),
                amount: Uint128::new(amount),
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Bluechip {
                    denom: "stake".to_string(),
                },
                amount: Uint128::new(amount),
            },
            amount: Uint128::new(amount),
            transaction_deadline: None,
            belief_price: None,
            max_spread: None,
        };

        execute(deps.as_mut(), env.clone(), info, msg).unwrap();

        // Calculate expected USD for this commit
        let commit_usd = Uint128::new(amount) * Uint128::new(2_500_000) / Uint128::new(1_000_000);
        expected_total += commit_usd;

        // Verify global USD tracking
        let current_total = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert_eq!(
            current_total, expected_total,
            "USD tracking inconsistent after {} commit",
            user
        );

        // Verify individual tracking
        let user_commit = COMMIT_INFO
            .load(&deps.storage, &Addr::unchecked(user))
            .unwrap();
        assert_eq!(
            user_commit.total_paid_usd, commit_usd,
            "User {} USD tracking incorrect",
            user
        );
    }

    // Final totals: $10 + $20 + $5 = $35
    assert_eq!(expected_total, Uint128::new(35_000_000));
}

#[test]
fn test_commit_with_zero_oracle_price() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::zero()); // ZERO PRICE

    let env = mock_env();
    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let result = execute(deps.as_mut(), env, info, msg);

    // Should reject with clear error
    assert!(result.is_err(), "Should reject zero oracle price");

    match result.unwrap_err() {
        ContractError::InvalidOraclePrice {} => {
            // Good! This is what we want
        }
        other => panic!("Wrong error type: {:?}", other),
    }
}
#[test]
fn test_usd_calculation_overflow() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(u128::MAX / 1000),
    }]);
    setup_pool_storage(&mut deps);

    // Extremely high price that would cause overflow
    with_factory_oracle(&mut deps, Uint128::new(1_000_000_000_000)); // $1M per token

    let env = mock_env();
    let info = mock_info(
        "whale",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(u128::MAX / 1000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(u128::MAX / 1000),
        },
        amount: Uint128::new(u128::MAX / 1000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let result = execute(deps.as_mut(), env, info, msg);

    // Should return error, not panic
    assert!(result.is_err(), "Should reject overflow");

    let err = result.unwrap_err();

    // Verify it's the overflow error we expect
    assert!(
        err.to_string().contains("Overflow")
            || err.to_string().contains("overflow")
            || err.to_string().contains("Querier system error"),
        "Error should mention overflow, got: {}",
        err
    );

    println!("✅ Correctly rejected overflow with error: {}", err);
}

#[test]
fn test_rounding_error_accumulation() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    // Price that causes rounding issues
    with_factory_oracle(&mut deps, Uint128::new(333_333)); // $0.333333...

    let env = mock_env();

    // Make 1000 tiny commits that should sum to specific amount
    let mut manual_sum = Uint128::zero();

    for i in 0..1000 {
        let user = format!("user{}", i);
        let amount = Uint128::new(1_000); // Tiny amount

        // Manual calculation
        let expected_usd = amount * Uint128::new(333_333) / Uint128::new(1_000_000);
        manual_sum += expected_usd;

        let info = mock_info(
            &user,
            &[Coin {
                denom: "stake".to_string(),
                amount,
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Bluechip {
                    denom: "stake".to_string(),
                },
                amount,
            },
            amount,
            transaction_deadline: None,
            belief_price: None,
            max_spread: None,
        };

        execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    }

    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();

    // Check if rounding errors accumulated significantly
    let diff = if total_usd > manual_sum {
        total_usd - manual_sum
    } else {
        manual_sum - total_usd
    };

    println!("Rounding difference over 1000 commits: {}", diff);

    // Acceptable tolerance?
    let max_acceptable = Uint128::new(1000); // 1000 units = 0.001 USD
    assert!(
        diff <= max_acceptable,
        "Rounding errors accumulated too much: {}",
        diff
    );
}

#[test]
fn test_swap_with_belief_price_protection() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000),
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // 100 bluechip

    // User believes they'll get at least 1.4 tokens per bluechip
    // Pool ratio is ~14.89 (350M/23.5M), so actual is much higher
    let belief_price = Some(Decimal::from_ratio(140u128, 100u128)); // 1.4

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: swap_amount,
        },
        belief_price,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    // Should succeed because actual price is better than belief
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );
}

#[test]
fn test_swap_belief_price_rejects_bad_price_corrected() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(10_000_000_000),
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k bluechip

    // Actual return will be ~104B tokens
    // To fail, we need to expect MORE than that
    // If belief_price = 0.05, expected = 10B / 0.05 = 200B
    // Since actual (104B) < expected (200B), it should fail
    let belief_price = Some(Decimal::from_ratio(5u128, 100u128)); // 0.05

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: swap_amount,
        },
        belief_price,
        max_spread: Some(Decimal::percent(1)), // Tight spread to ensure failure
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion {} => (),
        _ => panic!("Expected MaxSpreadAssertion error, got {:?}", err),
    }
}

#[test]
fn test_belief_price_with_zero_price() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    // Belief price of zero should be rejected
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        belief_price: Some(Decimal::zero()),
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::InvalidBeliefPrice {} => (),
        _ => panic!("Expected InvalidBeliefPrice error"),
    }
}

#[test]
fn test_swap_cw20_to_bluechip_direct() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Mock CW20 balance and transfer
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if contract_addr == "token_contract" {
                if msg.to_string().contains("balance") {
                    let balance_response = cw20::BalanceResponse {
                        balance: Uint128::new(350_000_000_000),
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&balance_response).unwrap(),
                    ));
                }
            }
            SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query".to_string(),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query type".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k CW20 tokens

    // Simulate CW20 receive hook
    let info = mock_info("token_contract", &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(5)), // Allow 5% slippage for this large swap
            to: None,
            transaction_deadline: None,
        })
        .unwrap(),
    };

    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();

    // Verify swap executed
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "offer_asset")
            .unwrap()
            .value,
        "token_contract"
    );

    // Should have bank send message for bluechip
    assert!(res
        .messages
        .iter()
        .any(|msg| { matches!(&msg.msg, CosmosMsg::Bank(BankMsg::Send { .. })) }));

    // Verify reserves updated correctly
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(23_500_000_000)); // Bluechip decreased
    assert!(pool_state.reserve1 > Uint128::new(350_000_000_000)); // CW20 increased
}

#[test]
fn test_swap_cw20_with_custom_recipient() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Mock CW20 queries
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if contract_addr == "token_contract" {
                if msg.to_string().contains("balance") {
                    let balance_response = cw20::BalanceResponse {
                        balance: Uint128::new(350_000_000_000),
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&balance_response).unwrap(),
                    ));
                }
            }
            SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query".to_string(),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query type".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // Reduced to 100M to avoid slippage
    let recipient = "beneficiary".to_string();

    let info = mock_info("token_contract", &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(2)), // Allow 2% slippage
            to: Some(recipient.clone()),
            transaction_deadline: None,
        })
        .unwrap(),
    };

    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();

    // Verify bluechip sent to custom recipient
    let bank_msg = res
        .messages
        .iter()
        .find_map(|msg| {
            if let CosmosMsg::Bank(BankMsg::Send { to_address, .. }) = &msg.msg {
                Some(to_address.clone())
            } else {
                None
            }
        })
        .expect("Should have bank send message");

    assert_eq!(
        bank_msg, recipient,
        "Bluechip should be sent to custom recipient"
    );
}

#[test]
fn test_cw20_swap_with_belief_price() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Mock CW20 balance
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if contract_addr == "token_contract" {
                if msg.to_string().contains("balance") {
                    let balance_response = cw20::BalanceResponse {
                        balance: Uint128::new(350_000_000_000),
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&balance_response).unwrap(),
                    ));
                }
            }
            SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query".to_string(),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query type".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000_000); // Large amount for slippage

    // Expect at least 0.05 bluechip per CW20 token
    let belief_price = Some(Decimal::from_ratio(5u128, 100u128));

    let info = mock_info("token_contract", &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price,
            max_spread: Some(Decimal::percent(10)),
            to: None,
            transaction_deadline: None,
        })
        .unwrap(),
    };

    // Should fail due to slippage from large swap
    let err = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion {} => (),
        _ => panic!(
            "Expected MaxSpreadAssertion due to belief price, got {:?}",
            err
        ),
    }
}

#[test]
fn test_race_condition_not_manually_set() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(20_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    // Just below threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let env = mock_env();

    // Alice's commit that crosses threshold
    let alice_info = mock_info(
        "alice",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(200_000_000),
        }],
    );

    let alice_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        amount: Uint128::new(200_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    // Execute Alice's commit
    let alice_res = execute(deps.as_mut(), env.clone(), alice_info, alice_msg).unwrap();

    // Verify threshold was crossed
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    assert!(alice_res
        .attributes
        .iter()
        .any(|a| a.value == "threshold_crossing"));

    // CRITICAL: Verify THRESHOLD_PROCESSING was properly cleared after successful threshold
    assert_eq!(
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        false,
        "THRESHOLD_PROCESSING should be cleared after successful threshold crossing"
    );

    // Bob tries to commit after threshold - should work as normal swap
    let bob_info = mock_info(
        "bob",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(200_000_000),
        }],
    );
    let before = POOL_STATE.load(&deps.storage).unwrap();
    println!(
        "Before Bob's swap: reserve0: {}, reserve1: {}",
        before.reserve0, before.reserve1
    );

    let bob_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        amount: Uint128::new(200_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: Some(Decimal::percent(99)),
    };

    let bob_res = execute(deps.as_mut(), env.clone(), bob_info.clone(), bob_msg).unwrap();

    // Verify Bob's commit was processed as swap
    assert!(bob_res
        .attributes
        .iter()
        .all(|a| a.value != "threshold_crossing"));
    assert!(bob_res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "commit"));

    // CRITICAL: Verify Bob's swap was recorded in pool state
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(
        pool_state.reserve0 > before.reserve0,
        "Pool reserve0 should have increased from Bob's bluechip swap"
    );
}

#[test]
fn test_concurrent_commits_both_recorded() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(20_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    // Set up near threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();

    // Add previous committers to ledger
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("previous1"),
            &Uint128::new(10_000_000_000),
        )
        .unwrap();
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("previous2"),
            &Uint128::new(14_900_000_000),
        )
        .unwrap();

    with_factory_oracle(&mut deps, Uint128::new(1_000_000));
    let env = mock_env();

    // Alice crosses threshold
    let alice_info = mock_info(
        "alice",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(200_000_000),
        }],
    );

    let alice_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        amount: Uint128::new(200_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), alice_info.clone(), alice_msg).unwrap();

    // Verify Alice was added to ledger before threshold processing
    assert!(
        COMMIT_LEDGER
            .load(&deps.storage, &alice_info.sender)
            .is_err(),
        "Alice should have been cleared from ledger after threshold"
    );

    // Bob commits after threshold with his own amount
    let bob_amount = Uint128::new(100_000_000);
    let bob_info = mock_info(
        "bob",
        &[Coin {
            denom: "stake".to_string(),
            amount: bob_amount,
        }],
    );

    let before = POOL_STATE.load(&deps.storage).unwrap();
    println!(
        "Before Bob's swap: reserve0: {}, reserve1: {}",
        before.reserve0, before.reserve1
    );

    let bob_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: bob_amount,
        },
        amount: bob_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: Some(Decimal::percent(50)),
    };

    let bob_res = execute(deps.as_mut(), env.clone(), bob_info.clone(), bob_msg).unwrap();

    // Verify it was processed as a swap
    assert!(
        bob_res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "commit"),
        "Bob's transaction should be a swap after threshold"
    );

    // Verify Bob's commit is NOT in the ledger (it's a swap now)
    assert!(
        COMMIT_LEDGER.load(&deps.storage, &bob_info.sender).is_err(),
        "Bob shouldn't be in commit ledger - his transaction is a swap"
    );

    // But verify Bob's transaction affected the pool
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!(
        "Pool reserves after Bob's swap - reserve0: {}, reserve1: {}",
        pool_state.reserve0, pool_state.reserve1
    );

    // Pool starts with ~23.5M bluechip, Bob adds more (minus fees)
    assert!(
        pool_state.reserve0 > before.reserve0,
        "Pool reserve0 should have increased from Bob's bluechip swap"
    );
}
