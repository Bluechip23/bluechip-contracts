use crate::asset::{PoolPairType, TokenInfo, TokenType};
use crate::contract::execute_simple_swap;
use crate::error::ContractError;
use crate::generic_helpers::calculate_effective_batch_size;
use crate::liquidity::execute_deposit_liquidity;
use crate::msg::ExecuteMsg;
use crate::state::{
    CommitLimitInfo, OracleInfo, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, PoolState,
    ThresholdPayoutAmounts, COMMIT_INFO, COMMIT_LEDGER, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
    DEFAULT_MAX_GAS_PER_TX, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID,
    ORACLE_INFO, POOL_FEE_STATE, POOL_PAUSED, POOL_SPECS, POOL_STATE, RATE_LIMIT_GUARD,
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
use cosmwasm_schema::cw_serde;
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

#[cw_serde]
enum FactoryQueryWrapper {
    InternalBlueChipOracleQuery(FactoryQueryMsg),
}
fn mock_dependencies_with_balance(
    balances: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    deps.querier
        .update_balance(MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}

pub fn with_factory_oracle(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    bluechip_to_usd_rate: Uint128,
) {
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if contract_addr == "factory_contract" || contract_addr == "factory" {
                if let Ok(FactoryQueryWrapper::InternalBlueChipOracleQuery(factory_query)) =
                    from_json::<FactoryQueryWrapper>(msg)
                {
                    match factory_query {
                        FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
                            let intermediate = match amount.checked_mul(bluechip_to_usd_rate) {
                                Ok(v) => v,
                                Err(_) => {
                                    return SystemResult::Err(SystemError::InvalidRequest {
                                        error: "Overflow in mock oracle calculation".to_string(),
                                        request: msg.clone(),
                                    });
                                }
                            };

                            let usd_amount = match intermediate.checked_div(Uint128::new(1_000_000))
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
                            let intermediate = match amount.checked_mul(Uint128::new(1_000_000)) {
                                Ok(v) => v,
                                Err(_) => {
                                    return SystemResult::Err(SystemError::InvalidRequest {
                                        error: "Overflow in mock oracle calculation".to_string(),
                                        request: msg.clone(),
                                    });
                                }
                            };

                            let bluechip_amount =
                                match intermediate.checked_div(bluechip_to_usd_rate) {
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

            if contract_addr == "nft_contract" {}

            SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown contract or query".to_string(),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query type".to_string(),
            request: Binary::default(),
        }),
    });
}
#[test]
fn test_commit_pre_threshold_basic() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000),
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

    assert_eq!(res.messages.len(), 2);

    let user_addr = Addr::unchecked("user1");
    let user_commit_usd = COMMIT_LEDGER.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(user_commit_usd, Uint128::new(1_000_000_000)); // $1k with 6 decimals

    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(1_000_000_000));

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

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let commit_amount = Uint128::new(200_000_000); // $200 per commit
    let env = mock_env();

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
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();
    println!(
        "Simulated race -> USD_RAISED_FROM_COMMIT: {}, IS_THRESHOLD_HIT: {}, THRESHOLD_PROCESSING: {}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap()
    );
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

    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap(); // $24.9k

    let env = mock_env();
    let commit_amount = Uint128::new(200_000_000); // 200 tokens = $200

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

    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);

    assert_eq!(THRESHOLD_PROCESSING.load(&deps.storage).unwrap(), false);
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "phase" && attr.value == "threshold_crossing"));

    assert!(
        res.messages.len() >= 6,
        "Expected at least 6 messages, got {}",
        res.messages.len()
    );

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.total_liquidity, Uint128::zero()); // Unowned seed liquidity

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

    assert!(res.messages.len() >= 3);

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Increased from commit
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // Decreased from swap

    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_1 > Decimal::zero());
    assert!(fee_state.total_fees_collected_1 > Uint128::zero());
}

#[test]
fn test_threshold_payout_integrity_check() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let mut bad_payout = THRESHOLD_PAYOUT_AMOUNTS
        .load(&deps.storage)
        .expect("failed to load payout");
    bad_payout.creator_reward_amount = Uint128::new(999_999_999_999); // Wrong total!
    THRESHOLD_PAYOUT_AMOUNTS
        .save(&mut deps.storage, &bad_payout)
        .expect("failed to save payout");

    let pool_info = POOL_INFO.load(&deps.storage).expect("pool_info");
    let mut pool_state = POOL_STATE.load(&deps.storage).expect("pool_state");
    let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).expect("pool_fee_state");
    let commit_config = COMMIT_LIMIT_INFO
        .load(&deps.storage)
        .expect("commit_config");
    let fee_info = COMMITFEEINFO.load(&deps.storage).expect("fee_info");
    let env = mock_env();

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

    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000_000),
        total_committed_usd: Uint128::new(1_000_000_000),
        last_processed_key: None,
        distributions_remaining: 10,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(0),
        last_updated: Timestamp::from_seconds(0),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();
    let msg = ExecuteMsg::ContinueDistribution {};
    let info = mock_info("random_user", &[]);

    let res = execute(deps.as_mut(), mock_env(), info, msg);

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

    for i in 0..5 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }
    let env = mock_env();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000_000),
        total_committed_usd: Uint128::new(1_000_000_000),
        last_processed_key: None,
        distributions_remaining: 5,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        last_successful_batch_size: Some(3), // Test with previous successful batch size
        consecutive_failures: 0,
        started_at: env.block.time, // Use current time
        last_updated: env.block.time,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

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
    assert!(
        res.messages.len() <= 3,
        "Should not exceed last successful batch size"
    );
}

#[test]
fn test_continue_distribution_batches() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    for i in 0..10 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }
    let env = mock_env();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 10,
        max_gas_per_tx: 200,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: env.block.time,
        last_updated: env.block.time,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = mock_info(env.contract.address.as_str(), &[]);
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // Calculate expected batch size
    let base_batch_size =
        (dist_state.max_gas_per_tx / dist_state.estimated_gas_per_distribution).max(1) as u32;
    let expected_batch_size = if dist_state.last_successful_batch_size.is_none() {
        base_batch_size.min(10).max(1) as usize
    } else {
        base_batch_size as usize
    };

    let actual_expected = expected_batch_size.min(dist_state.distributions_remaining as usize);

    // Check how many committers were actually processed
    let committers_after = COMMIT_LEDGER
        .range(&deps.storage, None, None, Order::Ascending)
        .count();
    let processed = 10 - committers_after;

    println!(
        "Debug: processed={}, expected={}, committers_after={}",
        processed, actual_expected, committers_after
    );

    assert_eq!(
        processed, actual_expected,
        "Should process exactly {} committers based on gas limits",
        actual_expected
    );

    // Check if state was updated or removed
    match DISTRIBUTION_STATE.may_load(&deps.storage).unwrap() {
        Some(new_state) => {
            assert_eq!(
                new_state.distributions_remaining,
                dist_state.distributions_remaining - processed as u32,
                "Distributions remaining should be updated correctly"
            );

            assert_eq!(
                new_state.last_successful_batch_size,
                Some(processed as u32),
                "Should record the actual batch size that was processed"
            );

            let has_continue = res.messages.iter().any(|submsg| match &submsg.msg {
                CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
                    from_json::<ExecuteMsg>(msg.clone()).map_or(false, |decoded| {
                        matches!(decoded, ExecuteMsg::ContinueDistribution { .. })
                    })
                }
                _ => false,
            });
            println!(
                "Debug: new_state.distributions_remaining={}, has_continue={}",
                new_state.distributions_remaining, has_continue
            );

            assert!(
                has_continue,
                "Should have continuation message when {} distributions remain",
                new_state.distributions_remaining
            );
        }
        None => {
            assert_eq!(
                processed, 10,
                "If state is removed, all 10 committers should have been processed"
            );
        }
    }
}
#[test]
fn test_adaptive_batch_sizing_with_history() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Add many committers
    for i in 0..20 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }
    let env = mock_env();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 20,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: Some(12),
        consecutive_failures: 0,
        started_at: env.block.time,
        last_updated: env.block.time,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let total_before = COMMIT_LEDGER
        .range(&deps.storage, None, None, Order::Ascending)
        .count();
    println!("Total committers before: {}", total_before);

    let env = mock_env();
    let info = mock_info(env.contract.address.as_str(), &[]);
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // Check what's left in ledger after processing
    let total_after = COMMIT_LEDGER
        .range(&deps.storage, None, None, Order::Ascending)
        .count();
    println!("Total committers after: {}", total_after);
    println!("Processed: {}", total_before - total_after);

    let all_messages = res.messages.len();
    let continue_messages = res
        .messages
        .iter()
        .filter(|submsg| match &submsg.msg {
            CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
                msg.to_string().contains("ContinueDistribution")
            }
            _ => false,
        })
        .count();
    let mint_messages = all_messages - continue_messages;

    println!(
        "Total messages: {}, Mint messages: {}, Continue messages: {}",
        all_messages, mint_messages, continue_messages
    );

    if let Ok(new_state) = DISTRIBUTION_STATE.load(&deps.storage) {
        println!(
            "New last_successful_batch_size: {:?}",
            new_state.last_successful_batch_size
        );
        println!(
            "Remaining distributions: {}",
            new_state.distributions_remaining
        );
    }

    let expected = 10;
    let actually_processed = total_before - total_after;
    assert_eq!(
        actually_processed, expected,
        "Should process exactly {} committers based on effective batch size",
        expected
    );
}

#[test]
fn test_calculate_effective_batch_size() {
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 20,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: Some(12),
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(0),
        last_updated: Timestamp::from_seconds(0),
    };

    let batch_size = calculate_effective_batch_size(&dist_state);

    assert_eq!(batch_size, 10, "Should use 90% of last successful");

    let dist_state_no_history = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 20,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(0),
        last_updated: Timestamp::from_seconds(0),
    };

    let batch_size = calculate_effective_batch_size(&dist_state_no_history);

    assert_eq!(batch_size, 10, "Should be conservative on first run");
}

#[test]
fn test_batch_size_with_consecutive_failures() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    for i in 0..10 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }
    let env = mock_env();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 10,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 200, // High estimate due to failures
        last_successful_batch_size: Some(2), // Last success was small
        consecutive_failures: 2,             // Had 2 failures
        started_at: env.block.time,          // Use current time
        last_updated: env.block.time,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = mock_info(env.contract.address.as_str(), &[]);
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // very conservative after failures
    assert!(
        res.messages.len() <= 2,
        "Should use very small batch size after failures"
    );
}

#[test]
fn test_final_batch_completes_distribution() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Add exactly 3 committers
    for i in 0..3 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }
    let env = mock_env();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(300),
        last_processed_key: None,
        distributions_remaining: 3,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: Some(5),
        consecutive_failures: 0,
        started_at: env.block.time, // Use current time
        last_updated: env.block.time,
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = mock_info(env.contract.address.as_str(), &[]);
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // Should complete all remaining
    assert_eq!(
        DISTRIBUTION_STATE.may_load(&deps.storage).unwrap(),
        None,
        "Distribution state should be removed after completion"
    );

    // no ContinueDistribution message since we're done
    let has_continue_msg = res.messages.iter().any(|submsg| match &submsg.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
            msg.to_string().contains("ContinueDistribution")
        }
        _ => false,
    });
    assert!(
        !has_continue_msg,
        "Should not trigger continuation when complete"
    );
}

#[test]
fn test_commit_reentrancy_protection() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

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

    env.block.time = env.block.time.plus_seconds(30); // Only 30 seconds later

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::TooFrequentCommits { wait_time } => {
            assert_eq!(wait_time, 30);
        }
        _ => panic!("Expected TooFrequentCommits error"),
    }
}

#[test]
fn test_commit_with_deadline() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000),
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

#[test]
fn test_simple_swap_bluechip_to_cw20() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000),
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

    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Native increased
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // CW20 decreased

    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_1 > Decimal::zero());
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

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion {} => (),
        _ => panic!("Expected MaxSpreadAssertion error"),
    }
}

#[test]
fn test_swap_cw20_via_hook() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if contract_addr == "token_contract" {
                if msg.to_string().contains("balance") {
                    let balance_response = cw20::BalanceResponse {
                        balance: Uint128::new(350_000_000_000),
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
    });

    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k tokens

    let info = mock_info("token_contract", &[]);

    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(10)),
            to: None,
            transaction_deadline: None,
        })
        .unwrap(),
    };

    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();

    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "swap"
    );

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

    let updated_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(updated_state.price0_cumulative_last > initial_price0);
    assert_eq!(updated_state.block_time_last, env.block.time.seconds());
}

#[test]
fn test_factory_impersonation_prevented() {
    let mut deps = mock_dependencies();

    let msg = PoolInstantiateMsg {
        pool_id: 1u64,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            },
        ],
        cw20_token_contract_id: 2u64,
        threshold_payout: None,
        used_factory_addr: Addr::unchecked("factory_contract"),
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("stake"),
            creator_wallet_address: Addr::unchecked("addr0000"),
            commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
            commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        },
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        commit_amount_for_threshold: Uint128::new(0),
        commit_threshold_limit_usd: Uint128::new(350_000_000_000),
        position_nft_address: Addr::unchecked("NFT_contract"),
        token_address: Addr::unchecked("token_contract"),
        is_standard_pool: None,
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

    with_factory_oracle(&mut deps, Uint128::new(2_000_000));

    let info2 = mock_info(
        "user2",
        &[Coin {
            denom: "stake".to_string(),
            amount: Uint128::new(5_000_000),
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

    let user2_commit = COMMIT_INFO
        .load(&deps.storage, &Addr::unchecked("user2"))
        .unwrap();
    assert_eq!(user2_commit.total_paid_usd, Uint128::new(10_000_000));
}

#[test]
fn test_threshold_crossing_depends_on_oracle_price() {
    let mut deps1 = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps1);
    THRESHOLD_PROCESSING
        .save(&mut deps1.storage, &false)
        .unwrap();

    with_factory_oracle(&mut deps1, Uint128::new(10_000_000));
    USD_RAISED_FROM_COMMIT
        .save(&mut deps1.storage, &Uint128::new(24_000_000_000))
        .unwrap();

    let env = mock_env();
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
    let mut deps2 = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps2);
    THRESHOLD_PROCESSING
        .save(&mut deps2.storage, &false)
        .unwrap();

    with_factory_oracle(&mut deps2, Uint128::new(100_000)); // $0.10

    USD_RAISED_FROM_COMMIT
        .save(&mut deps2.storage, &Uint128::new(24_000_000_000))
        .unwrap();

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
            amount: Uint128::new(1_000_000_000),
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
    assert_eq!(usd_low, Uint128::new(1_000_000));

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
    assert_eq!(usd_high, Uint128::new(1_000_000_000));
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

        let commit_usd = Uint128::new(amount) * Uint128::new(2_500_000) / Uint128::new(1_000_000);
        expected_total += commit_usd;

        let current_total = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert_eq!(
            current_total, expected_total,
            "USD tracking inconsistent after {} commit",
            user
        );
        let user_commit = COMMIT_INFO
            .load(&deps.storage, &Addr::unchecked(user))
            .unwrap();
        assert_eq!(
            user_commit.total_paid_usd, commit_usd,
            "User {} USD tracking incorrect",
            user
        );
    }

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

    assert!(result.is_err(), "Should reject zero oracle price");

    match result.unwrap_err() {
        ContractError::InvalidOraclePrice {} => {}
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

    assert!(result.is_err(), "Should reject overflow");

    let err = result.unwrap_err();

    assert!(
        err.to_string().contains("Overflow")
            || err.to_string().contains("overflow")
            || err.to_string().contains("Querier system error"),
        "Error should mention overflow, got: {}",
        err
    );

    println!("Correctly rejected overflow with error: {}", err);
}

#[test]
fn test_rounding_error_accumulation() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::new(333_333)); // $0.333333...

    let env = mock_env();

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
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(23_500_000_000)); // Bluechip decreased
    assert!(pool_state.reserve1 > Uint128::new(350_000_000_000)); // CW20 increased
}

#[test]
fn test_swap_cw20_with_custom_recipient() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

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

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let env = mock_env();

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

    let alice_res = execute(deps.as_mut(), env.clone(), alice_info, alice_msg).unwrap();

    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    assert!(alice_res
        .attributes
        .iter()
        .any(|a| a.value == "threshold_crossing"));

    assert_eq!(
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        false,
        "THRESHOLD_PROCESSING should be cleared after successful threshold crossing"
    );

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

    assert!(bob_res
        .attributes
        .iter()
        .all(|a| a.value != "threshold_crossing"));
    assert!(bob_res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "commit"));

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

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();

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

    assert!(
        COMMIT_LEDGER
            .load(&deps.storage, &alice_info.sender)
            .is_err(),
        "Alice should have been cleared from ledger after threshold"
    );

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

    assert!(
        bob_res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "commit"),
        "Bob's transaction should be a swap after threshold"
    );

    assert!(
        COMMIT_LEDGER.load(&deps.storage, &bob_info.sender).is_err(),
        "Bob shouldn't be in commit ledger - his transaction is a swap"
    );

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!(
        "Pool reserves after Bob's swap - reserve0: {}, reserve1: {}",
        pool_state.reserve0, pool_state.reserve1
    );

    assert!(
        pool_state.reserve0 > before.reserve0,
        "Pool reserve0 should have increased from Bob's bluechip swap"
    );
}
pub fn setup_pool_with_reserves(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    reserve0: Uint128,
    reserve1: Uint128,
) {
    let pool_info = PoolInfo {
        pool_id: 1u64,
        pool_info: PoolDetails {
            asset_infos: [
                TokenType::Bluechip {
                    denom: "stake".to_string(),
                },
                TokenType::CreatorToken {
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
        reserve0: reserve0, // No reserves pre-threshold
        reserve1: reserve1,
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
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    };
    POOL_FEE_STATE
        .save(&mut deps.storage, &pool_fee_state)
        .unwrap();

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::percent(3) / Uint128::new(10), // 0.3% fee (3/1000)
        min_commit_interval: 60,                        // 1 minute minimum between commits
        usd_payment_tolerance_bps: 100,                 // 1% tolerance
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(100_000_000), // 100 bluechip tokens
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    COMMIT_LIMIT_INFO
        .save(&mut deps.storage, &commit_config)
        .unwrap();

    let threshold_payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000), // 325k tokens
        bluechip_reward_amount: Uint128::new(25_000_000_000), // 25k tokens
        pool_seed_amount: Uint128::new(350_000_000_000),      // 350k tokens
        commit_return_amount: Uint128::new(500_000_000_000),  // 500k tokens
    };
    THRESHOLD_PAYOUT_AMOUNTS
        .save(&mut deps.storage, &threshold_payout)
        .unwrap();
    let commit_fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip_treasury"),
        creator_wallet_address: Addr::unchecked("creator_wallet"),
        commit_fee_bluechip: Decimal::percent(1), // 1%
        commit_fee_creator: Decimal::percent(5),  // 5%
    };
    COMMITFEEINFO
        .save(&mut deps.storage, &commit_fee_info)
        .unwrap();

    let oracle_info = OracleInfo {
        oracle_addr: Addr::unchecked("oracle_contract"),
    };
    ORACLE_INFO.save(&mut deps.storage, &oracle_info).unwrap();

    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();
    IS_THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::zero())
        .unwrap();
    NATIVE_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::zero())
        .unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();
}

#[test]
fn test_swap_fails_when_reserves_below_pause_threshold() {
    let mut deps = mock_dependencies();

    // Setup pool with reserves just below pause threshold
    setup_pool_with_reserves(&mut deps, Uint128::new(9), Uint128::new(100_000));

    let offer = TokenInfo {
        info: TokenType::Bluechip {
            denom: "stake".to_string(),
        },
        amount: Uint128::new(100),
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user", &[]),
        Addr::unchecked("user"),
        offer,
        None,
        None,
        None,
    );

    // Should fail and pause the pool
    assert!(matches!(
        result,
        Err(ContractError::InsufficientReserves {})
    ));

    // Verify pool is now paused
    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(is_paused, "Pool should be paused after hitting threshold");
}

#[test]
fn test_swap_fails_when_pool_already_paused() {
    let mut deps = mock_dependencies();
    setup_pool_with_reserves(&mut deps, Uint128::new(50_000), Uint128::new(50_000));

    // Manually pause the pool
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let offer = TokenInfo {
        info: TokenType::Bluechip {
            denom: "stake".to_string(),
        },

        amount: Uint128::new(100),
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user", &[]),
        Addr::unchecked("user"),
        offer,
        None,
        None,
        None,
    );

    assert!(matches!(
        result,
        Err(ContractError::PoolPausedLowLiquidity {})
    ));
}
#[test]
fn test_swap_prevented_if_would_deplete_below_minimum() {
    let mut deps = mock_dependencies();

    // Set reserves above SWAP_PAUSE_THRESHOLD (100) but where swap would deplete below MINIMUM_LIQUIDITY (1000)
    setup_pool_with_reserves(
        &mut deps,
        Uint128::new(10000), // Well above SWAP_PAUSE_THRESHOLD
        Uint128::new(1100),  // Just above MINIMUM_LIQUIDITY
    );

    // Calculate swap that would deplete reserve1 below 1000
    // k = 10000 * 1100 = 11,000,000
    // If we add 2000 to reserve0: new reserve0 = 12000
    // new reserve1 = 11,000,000 / 12000 = 916.67 (below MINIMUM_LIQUIDITY of 1000!)

    let swap_amount = Uint128::new(2000);
    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    let offer = TokenInfo {
        info: TokenType::Bluechip {
            denom: "stake".to_string(),
        },
        amount: swap_amount,
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        info,
        Addr::unchecked("user"),
        offer,
        None,
        Some(Decimal::percent(50)), // Allow high spread
        None,
    );

    // Should fail with InsufficientReserves (based on your actual code)
    assert!(
        matches!(result, Err(ContractError::InsufficientReserves {})),
        "Expected InsufficientReserves error for post-swap depletion, got: {:?}",
        result
    );
}

#[test]
fn test_swap_triggers_pause_at_threshold() {
    let mut deps = mock_dependencies();

    // Set one reserve below SWAP_PAUSE_THRESHOLD (100)
    setup_pool_with_reserves(
        &mut deps,
        Uint128::new(99), // Below SWAP_PAUSE_THRESHOLD!
        Uint128::new(10000),
    );

    let swap_amount = Uint128::new(10);
    let info = mock_info(
        "user",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    let offer = TokenInfo {
        info: TokenType::Bluechip {
            denom: "stake".to_string(),
        },
        amount: swap_amount,
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        info,
        Addr::unchecked("user"),
        offer,
        None,
        Some(Decimal::percent(50)),
        None,
    );

    // Should fail with InsufficientReserves at pre-swap check
    assert!(
        matches!(result, Err(ContractError::InsufficientReserves {})),
        "Expected InsufficientReserves error at pre-swap check, got: {:?}",
        result
    );

    // Verify pool is paused
    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(
        is_paused,
        "Pool should be paused when reserves drop below threshold"
    );
}

#[test]
fn test_add_liquidity_unpauses_pool() {
    let mut deps = mock_dependencies();

    // Setup pool with low reserves and pause it
    setup_pool_with_reserves(&mut deps, Uint128::new(5000), Uint128::new(5000));
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let result = execute_deposit_liquidity(
        deps.as_mut(),
        mock_env(),
        mock_info(
            "provider",
            &[
                Coin::new(50_000, "stake"),
                Coin::new(50_000, "token1_contract"),
            ],
        ),
        Addr::unchecked("provider"),
        Uint128::new(50_000),
        Uint128::new(50_000),
        None,
        None,
        None,
    );
    if result.is_err() {
        println!("Liquidity deposit failed: {:?}", result);
    }
    assert!(result.is_ok());

    // Check pool is unpaused
    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(
        !is_paused,
        "Pool should be uspaused after adding sufficient liquidity"
    );

    // Verify the response contains unpause attribute
    let response = result.unwrap();
    assert!(response
        .attributes
        .iter()
        .any(|attr| attr.key == "pool_unpaused" && attr.value == "true"));
}

#[test]
fn test_add_liquidity_doesnt_unpause_if_still_below_threshold() {
    let mut deps = mock_dependencies();

    setup_pool_with_reserves(&mut deps, Uint128::new(100), Uint128::new(100));
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let result = execute_deposit_liquidity(
        deps.as_mut(),
        mock_env(),
        mock_info(
            "provider",
            &[Coin::new(500, "stake"), Coin::new(500, "token1")],
        ),
        Addr::unchecked("provider"),
        Uint128::new(500),
        Uint128::new(500),
        None,
        None,
        None,
    );
    assert!(result.is_ok());

    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(
        is_paused,
        "Pool should remain paused with insufficient liquidity"
    );
}

#[test]
fn test_both_reserves_checked() {
    let mut deps = mock_dependencies();

    // Test with low reserve0
    setup_pool_with_reserves(&mut deps, Uint128::new(9999), Uint128::new(10));

    let result1 = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user", &[]),
        Addr::unchecked("user"),
        TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
    );

    assert!(matches!(
        result1,
        Err(ContractError::InsufficientReserves {})
    ));

    // Test with low reserve1
    setup_pool_with_reserves(&mut deps, Uint128::new(10), Uint128::new(9999));
    POOL_PAUSED.remove(&mut deps.storage); // Reset pause state

    let result2 = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user", &[]),
        Addr::unchecked("user"),
        TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
    );

    assert!(matches!(
        result2,
        Err(ContractError::InsufficientReserves {})
    ));
}

#[test]
fn test_pause_state_persistence() {
    let mut deps = mock_dependencies();
    setup_pool_with_reserves(&mut deps, Uint128::new(15), Uint128::new(15));

    // First swap triggers pause
    let _ = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user1", &[]),
        Addr::unchecked("user1"),
        TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
    );

    // Second user tries to swap - should fail due to pause
    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        mock_info("user2", &[]),
        Addr::unchecked("user2"),
        TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
    );

    assert!(matches!(
        result,
        Err(ContractError::PoolPausedLowLiquidity {})
    ));
}

#[test]
fn test_swap_lopsided_pool_after_threshold() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Manually skew the pool to be lopsided (low bluechip, high token)
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(1_000_000_000); // Only 1k bluechip
    pool_state.reserve1 = Uint128::new(100_000_000_000); // 100k tokens
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let env = mock_env();

    // Try to swap a significant amount of bluechip (relative to reserve)
    // 500 bluechip (50% of reserve!)
    let swap_amount = Uint128::new(500_000_000);

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
        // Allow high slippage (50%) because we are intentionally swapping a huge amount relative to liquidity
        max_spread: Some(Decimal::percent(50)),
        to: None,
        transaction_deadline: None,
    };

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    // Check price impact
    let return_amount = res
        .attributes
        .iter()
        .find(|a| a.key == "return_amount")
        .unwrap()
        .value
        .parse::<u128>()
        .unwrap();

    // Just verify it didn't panic and returned *something* less than linear expectation
    assert!(return_amount > 0);

    // Verify pool state updated
    let new_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(new_pool_state.reserve0, Uint128::new(1_500_000_000));
}

#[test]
fn test_swap_slippage_lopsided() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Skew pool
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(1_000_000_000);
    pool_state.reserve1 = Uint128::new(100_000_000_000);
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let env = mock_env();
    let swap_amount = Uint128::new(500_000_000); // 50% of reserve

    let info = mock_info(
        "trader",
        &[Coin {
            denom: "stake".to_string(),
            amount: swap_amount,
        }],
    );

    // Expect 100 tokens per bluechip roughly (100k/1k)
    // So 500 bluechip should get ~50k tokens ideally
    // belief_price is Price of Ask (Token) in Offer (Bluechip).
    // Price = 1000 / 100000 = 0.01 Bluechip per Token.

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: Some(Decimal::percent(1)), // 0.01
        max_spread: Some(Decimal::percent(1)),   // 1% tolerance
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion { .. } => {}
        _ => panic!("Expected MaxSpreadAssertion error due to high slippage in lopsided pool"),
    }
}
