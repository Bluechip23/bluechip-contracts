use crate::asset::{PoolPairType, TokenInfo, TokenType};
use crate::swap_helper::execute_simple_swap;
use crate::error::ContractError;
use crate::generic_helpers::calculate_effective_batch_size;
use crate::liquidity::execute_deposit_liquidity;
use crate::msg::ExecuteMsg;
use crate::state::{
    CommitLimitInfo, OracleInfo, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, PoolState,
    ThresholdPayoutAmounts, COMMIT_INFO, COMMIT_LEDGER, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
    DEFAULT_MAX_GAS_PER_TX, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID,
    ORACLE_INFO, POOL_FEE_STATE, POOL_PAUSED, POOL_SPECS, POOL_STATE, REENTRANCY_LOCK,
    USD_RAISED_FROM_COMMIT,
};
use crate::{
    contract::{execute, instantiate},
    swap_helper::execute_swap_cw20,
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
        message_info, mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage,
        MOCK_CONTRACT_ADDR,
    },
    to_json_binary, Addr, BankMsg, Binary, Coin, ContractResult, CosmosMsg, Decimal, Order,
    OwnedDeps, SystemError, SystemResult, Timestamp, Uint128, WasmQuery,
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
        .bank
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    let env = mock_env();
    let commit_amount = Uint128::new(1_000_000_000); // 1k bluechip
    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let info = message_info(
        &Addr::unchecked("user1"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
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

    assert!(!IS_THRESHOLD_HIT.load(&deps.storage).unwrap());

    let committing = COMMIT_INFO.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(committing.total_paid_bluechip, commit_amount);
    assert_eq!(committing.total_paid_usd, Uint128::new(1_000_000_000));
}

#[test]
fn test_race_condition_commits_crossing_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
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

    let info1 = message_info(
        &Addr::unchecked("alice"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );
    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
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
    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();
    println!(
        "Simulated race -> USD_RAISED_FROM_COMMIT: {}, IS_THRESHOLD_HIT: {}, THRESHOLD_PROCESSING: {}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap()
    );
    let info2 = message_info(
        &Addr::unchecked("bob"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );
    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: Some(Decimal::percent(10)),
    };
    // After the H-cooldown change, a follower commit landing in the same
    // block as the threshold-crossing tx is rejected outright with
    // `PostThresholdCooldownActive`. This is the same-block-sandwich
    // defense: Bob's commit cannot atomically swap against Alice's
    // freshly-seeded pool. Pre-cooldown, this test asserted that Bob's
    // tx fell through to `process_post_threshold_commit` (silently
    // succeeding); now we assert that Bob's tx errors with the cooldown
    // and produces zero state side-effects.
    let err2 = execute(deps.as_mut(), env.clone(), info2, msg2).unwrap_err();
    match err2 {
        ContractError::PostThresholdCooldownActive { until_block } => {
            assert!(
                until_block > env.block.height,
                "cooldown until_block {} must be > current block {}",
                until_block,
                env.block.height
            );
        }
        other => panic!(
            "Expected PostThresholdCooldownActive on same-block follower commit, got {:?}",
            other
        ),
    }

    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();
}

#[test]
fn test_commit_crosses_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
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
    let info = message_info(
        &Addr::unchecked("whale"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());

    assert!(!THRESHOLD_PROCESSING.load(&deps.storage).unwrap());
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
    assert!(
        !pool_state.total_liquidity.is_zero(),
        "Seed liquidity should be non-zero after threshold crossing"
    );
    assert!(
        DISTRIBUTION_STATE
            .may_load(&deps.storage)
            .unwrap()
            .is_some(),
        "Distribution state should be initialized for batched payout"
    );
}

#[test]
fn test_commit_post_threshold_swap() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let commit_amount = Uint128::new(100_000_000); // 100 bluechip

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let info = message_info(
        &Addr::unchecked("commiter"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
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
fn test_continue_distribution_is_permissionless() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

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
        total_to_distribute: Uint128::new(1_000_000_000),
        total_committed_usd: Uint128::new(300),
        last_processed_key: None,
        distributions_remaining: 3,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: env.block.time,
        last_updated: env.block.time,
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();
    let msg = ExecuteMsg::ContinueDistribution {};
    // Any external user can call ContinueDistribution — it's permissionless
    let info = message_info(&Addr::unchecked("random_user"), &[]);

    let res = execute(deps.as_mut(), mock_env(), info, msg);

    assert!(
        res.is_ok(),
        "ContinueDistribution should be permissionless, got: {:?}",
        res.unwrap_err()
    );
}

#[test]
fn test_continue_distribution_processes_batch() {
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
        started_at: env.block.time,
        last_updated: env.block.time,
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    // Permissionless — any user can trigger
    let info = message_info(&Addr::unchecked("anyone"), &[]);

    let msg = ExecuteMsg::ContinueDistribution {};
    let res = execute(deps.as_mut(), env, info, msg).expect("permissionless call should succeed");

    assert!(
        res.attributes
            .iter()
            .any(|a| a.value == "continue_distribution"),
        "Response should include continue_distribution attribute"
    );

    assert!(
        res.messages.len() >= 5,
        "All 5 committers should be processed in one batch with gas-based batch size"
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = message_info(&Addr::unchecked("anyone"), &[]);
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

            // Messages: `processed` mint messages plus one WasmMsg::Execute
            // forwarding the bounty payment to the factory. No self-call
            // ContinueDistribution — external callers trigger subsequent
            // batches in separate transactions.
            assert_eq!(
                res.messages.len(),
                processed + 1,
                "Expected `processed` mints + 1 factory bounty msg, got: {:?}",
                res.messages
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let total_before = COMMIT_LEDGER
        .range(&deps.storage, None, None, Order::Ascending)
        .count();

    let env = mock_env();
    let info = message_info(&Addr::unchecked("anyone"), &[]);
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
    let actually_processed = total_before - total_after;

    // Mints + 1 factory bounty WasmMsg + 1 dust-settlement mint to the
    // creator. The test inputs have `total_committed_usd = 1_000_000`
    // but ledger sums to 2_000, so per-user floor(100 * 1_000_000 /
    // 1_000_000) = 100; 20 * 100 = 2_000 vs total_to_distribute =
    // 1_000_000, leaving a 998_000-base-unit residual that the final
    // batch settles to the creator wallet (audit fix H2).
    assert_eq!(
        res.messages.len(),
        actually_processed + 2,
        "Expected `actually_processed` mints + 1 factory bounty msg + 1 dust-settlement mint"
    );

    let expected = 20;
    assert_eq!(
        actually_processed, expected,
        "Should process exactly {} committers based on gas-based batch size",
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
        distributed_so_far: Uint128::zero(),
    };

    let batch_size = calculate_effective_batch_size(&dist_state);

    assert_eq!(
        batch_size, 20,
        "Should use gas-based estimate, ignoring last_successful_batch_size"
    );

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
        distributed_so_far: Uint128::zero(),
    };

    let batch_size = calculate_effective_batch_size(&dist_state_no_history);

    assert_eq!(
        batch_size, 20,
        "Should use gas-based estimate regardless of history"
    );
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // Up to 5 mints (gas estimate cap) + 1 factory bounty msg.
    assert!(
        res.messages.len() <= 6,
        "Should process at most 5 committers + 1 factory bounty msg, got {}",
        res.messages.len()
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let info = message_info(&Addr::unchecked("anyone"), &[]);
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

    // 3 committer mints + 1 dust-settlement mint to creator + 1
    // factory bounty WasmMsg. With 3 committers each paying 100 USD
    // and total_to_distribute = 1_000_000, per-user reward floors to
    // 333_333; 3 * 333_333 = 999_999, leaving 1 base unit of dust the
    // final batch settles to the creator wallet (audit fix H2).
    assert_eq!(
        res.messages.len(),
        5,
        "Expected 3 mint messages for committers + 1 dust mint + 1 factory bounty msg"
    );
}

#[test]
fn test_commit_reentrancy_protection() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    REENTRANCY_LOCK.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);

    let mut env = mock_env();
    let user = Addr::unchecked("user");

    // $5 = MIN_COMMIT_USD_PRE_THRESHOLD; the test is about rate-limiting,
    // not commit sizing. 5 bluechip atoms @ $1/bluechip = $5 USD.
    let info = message_info(
        &user,
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000),
        }],
    );

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip with 6 decimals

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(5_000_000),
        },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_000_000);

    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000),
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // 1k bluechip

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
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

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: Some(Decimal::permille(1)), // 0.1%
        allow_high_max_spread: None,
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
                let balance_response = cw20::BalanceResponse {
                    balance: Uint128::new(360_000_000_000),
                };
                SystemResult::Ok(ContractResult::Ok(
                    to_json_binary(&balance_response).unwrap(),
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
    });

    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k tokens

    let info = message_info(&Addr::unchecked("token_contract"), &[]);

    let cw20_msg = Cw20ReceiveMsg {
        sender: MockApi::default().addr_make("trader").to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(10)),
            allow_high_max_spread: Some(true),
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

/// M-7 audit: a hostile CW20 cannot dispatch a Receive hook with a
/// fabricated `amount` and drain the opposite reserve. The pool
/// queries the CW20's balance, compares to `reserve + fee_reserve +
/// creator_pot + claimed_amount`, and rejects on shortfall.
#[test]
fn test_cw20_receive_rejects_balance_shortfall() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Hostile CW20: dispatches Receive claiming 10B tokens but its own
    // balance for the pool is still the pre-attack reserve (350B) — no
    // actual transfer happened.
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, .. } if contract_addr == "token_contract" => {
            SystemResult::Ok(ContractResult::Ok(
                to_json_binary(&cw20::BalanceResponse {
                    balance: Uint128::new(350_000_000_000),
                })
                .unwrap(),
            ))
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    let info = message_info(&Addr::unchecked("token_contract"), &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: MockApi::default().addr_make("attacker").to_string(),
        amount: Uint128::new(10_000_000_000), // claim 10B with no actual transfer
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(5)),
            allow_high_max_spread: None,
            to: None,
            transaction_deadline: None,
        })
        .unwrap(),
    };

    let err = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap_err();
    match err {
        crate::error::ContractError::Cw20SwapBalanceMismatch {
            expected_min,
            actual,
            claimed_amount,
            ..
        } => {
            assert_eq!(claimed_amount, Uint128::new(10_000_000_000));
            assert_eq!(actual, Uint128::new(350_000_000_000));
            assert_eq!(expected_min, Uint128::new(360_000_000_000));
        }
        other => panic!("expected Cw20SwapBalanceMismatch, got {:?}", other),
    }

    // Pool state must be untouched after the rejection.
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::new(23_500_000_000));
    assert_eq!(pool_state.reserve1, Uint128::new(350_000_000_000));
}

#[test]
fn test_swap_wrong_asset() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "wrong_token".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "wrong_token".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
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

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
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
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: MockApi::default().addr_make("WILL_BE_CREATED_BY_FACTORY"),
            },
        ],
        cw20_token_contract_id: 2u64,
        threshold_payout: None,
        used_factory_addr: Addr::unchecked("factory_contract"),
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("ubluechip"),
            creator_wallet_address: Addr::unchecked("addr0000"),
            commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
            commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        },
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        commit_threshold_limit_usd: Uint128::new(350_000_000_000),
        position_nft_address: Addr::unchecked("NFT_contract"),
        token_address: Addr::unchecked("token_contract"),
    };
    let info = message_info(&Addr::unchecked("fake_factory"), &[]); // Wrong sender!
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();

    match err {
        ContractError::Unauthorized {} => (),
        _ => panic!("Expected Unauthorized error"),
    }
}

#[test]
fn test_commit_with_changing_oracle_prices() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let env = mock_env();
    let info1 = message_info(
        &Addr::unchecked("user1"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000),
        }],
    );

    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(5_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info1, msg1).unwrap();

    let first_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(first_usd, Uint128::new(5_000_000)); // $5

    with_factory_oracle(&mut deps, Uint128::new(2_000_000));

    let info2 = message_info(
        &Addr::unchecked("user2"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000),
        }],
    );

    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(5_000_000),
        },
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
        denom: "ubluechip".to_string(),
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
    let info1 = message_info(
        &Addr::unchecked("whale"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100_000_000), // 100 tokens
        }],
    );

    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps1.as_mut(), env.clone(), info1, msg1).unwrap();
    assert!(IS_THRESHOLD_HIT.load(&deps1.storage).unwrap());
    let mut deps2 = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
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

    let info2 = message_info(
        &Addr::unchecked("whale"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100_000_000),
        }],
    );

    let msg2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps2.as_mut(), env, info2, msg2).unwrap();
    assert!(!IS_THRESHOLD_HIT.load(&deps2.storage).unwrap());

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

    // Every case targets $5 USD — MIN_COMMIT_USD_PRE_THRESHOLD is $5,
    // so $1 test cases from pre-audit wouldn't reach the validator any
    // more. Scaling token_amounts by 5x preserves the cross-price
    // equivalence the test is really checking.
    let test_cases = vec![
        TestCase {
            oracle_price: Uint128::new(1_000_000), // $1
            token_amount: Uint128::new(5_000_000), // 5 tokens
            expected_usd: Uint128::new(5_000_000), // $5
            description: "$1 per token, 5 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(500_000),    // $0.50
            token_amount: Uint128::new(10_000_000), // 10 tokens
            expected_usd: Uint128::new(5_000_000),  // $5
            description: "$0.50 per token, 10 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(10_000_000), // $10
            token_amount: Uint128::new(500_000),    // 0.5 tokens
            expected_usd: Uint128::new(5_000_000),  // $5
            description: "$10 per token, 0.5 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(100_000),    // $0.10
            token_amount: Uint128::new(50_000_000), // 50 tokens
            expected_usd: Uint128::new(5_000_000),  // $5
            description: "$0.10 per token, 50 tokens",
        },
        TestCase {
            oracle_price: Uint128::new(3_333_333), // $3.33...
            token_amount: Uint128::new(3_000_000), // 3 tokens
            expected_usd: Uint128::new(9_999_999), // ~$10 (already over $5)
            description: "$3.33 per token, 3 tokens",
        },
    ];

    for test in test_cases {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "ubluechip".to_string(),
            amount: test.token_amount,
        }]);
        setup_pool_storage(&mut deps);

        with_factory_oracle(&mut deps, test.oracle_price);

        let env = mock_env();
        let info = message_info(
            &Addr::unchecked("user"),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: test.token_amount,
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                amount: test.token_amount,
            },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000_000), // 1M tokens
    }]);
    setup_pool_storage(&mut deps_low);

    with_factory_oracle(&mut deps_low, Uint128::new(1_000)); // $0.001

    let env = mock_env();
    // 5B bluechip atoms @ $0.001/bluechip = $5 USD (atomics 5_000_000)
    // — exactly MIN_COMMIT_USD_PRE_THRESHOLD. Below the threshold the
    // $5 min commit guard would reject; this test is about the math at
    // a very low rate, not the guard.
    let info_low = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000_000),
        }],
    );

    let msg_low = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(5_000_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res_low = execute(deps_low.as_mut(), env.clone(), info_low, msg_low);
    assert!(res_low.is_ok(), "Should handle very low prices");

    let usd_low = USD_RAISED_FROM_COMMIT.load(&deps_low.storage).unwrap();
    assert_eq!(usd_low, Uint128::new(5_000_000));

    let mut deps_high = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    setup_pool_storage(&mut deps_high);

    with_factory_oracle(&mut deps_high, Uint128::new(1_000_000_000)); // $1000

    let info_high = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000), // 1 token
        }],
    );

    let msg_high = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
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
        denom: "ubluechip".to_string(),
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
        let info = message_info(
            &Addr::unchecked(user),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(amount),
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                amount: Uint128::new(amount),
            },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::zero()); // ZERO PRICE

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(u128::MAX / 1000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::new(1_000_000_000_000)); // $1M per token

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("whale"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(u128::MAX / 1000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(u128::MAX / 1000),
        },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    with_factory_oracle(&mut deps, Uint128::new(333_333)); // $0.333333...

    let env = mock_env();

    let mut manual_sum = Uint128::zero();

    for i in 0..1000 {
        let user = format!("user{}", i);
        // 16M bluechip atoms @ $0.333333/bluechip ≈ $5.33 — above
        // MIN_COMMIT_USD_PRE_THRESHOLD ($5). 1000 commits at ~$5.33
        // accumulate to ~$5,333, well under the $25k threshold so
        // every commit stays pre-threshold (which is what this test
        // exercises: rounding drift in the ledger USD accumulator).
        let amount = Uint128::new(16_000_000);

        // Manual calculation
        let expected_usd = amount * Uint128::new(333_333) / Uint128::new(1_000_000);
        manual_sum += expected_usd;

        let info = message_info(
            &Addr::unchecked(&user),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount,
            }],
        );

        let msg = ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                amount,
            },
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000),
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // 100 bluechip

    let belief_price = Some(Decimal::from_ratio(140u128, 100u128)); // 1.4

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price,
        max_spread: None,
        allow_high_max_spread: None,
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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000_000),
    }]);
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k bluechip

    let belief_price = Some(Decimal::from_ratio(5u128, 100u128)); // 0.05

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price,
        max_spread: Some(Decimal::percent(1)), // Tight spread to ensure failure
        allow_high_max_spread: None,
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
    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1_000_000),
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000),
        },
        belief_price: Some(Decimal::zero()),
        max_spread: None,
        allow_high_max_spread: None,
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
                let balance_response = cw20::BalanceResponse {
                    balance: Uint128::new(360_000_000_000),
                };
                return SystemResult::Ok(ContractResult::Ok(
                    to_json_binary(&balance_response).unwrap(),
                ));
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

    let info = message_info(&Addr::unchecked("token_contract"), &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: MockApi::default().addr_make("trader").to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(5)), // Allow 5% slippage for this large swap
            allow_high_max_spread: None,
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
                let balance_response = cw20::BalanceResponse {
                    balance: Uint128::new(350_100_000_000),
                };
                return SystemResult::Ok(ContractResult::Ok(
                    to_json_binary(&balance_response).unwrap(),
                ));
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
    let recipient = MockApi::default().addr_make("beneficiary").to_string();

    let info = message_info(&Addr::unchecked("token_contract"), &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: MockApi::default().addr_make("trader").to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(2)), // Allow 2% slippage
            allow_high_max_spread: None,
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
                let balance_response = cw20::BalanceResponse {
                    balance: Uint128::new(450_000_000_000),
                };
                return SystemResult::Ok(ContractResult::Ok(
                    to_json_binary(&balance_response).unwrap(),
                ));
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

    let info = message_info(&Addr::unchecked("token_contract"), &[]);
    let cw20_msg = Cw20ReceiveMsg {
        sender: MockApi::default().addr_make("trader").to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price,
            max_spread: Some(Decimal::percent(10)),
            allow_high_max_spread: Some(true),
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
        denom: "ubluechip".to_string(),
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

    let alice_info = message_info(
        &Addr::unchecked("alice"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(200_000_000),
        }],
    );

    let alice_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let alice_res = execute(deps.as_mut(), env.clone(), alice_info, alice_msg).unwrap();

    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());
    assert!(alice_res
        .attributes
        .iter()
        .any(|a| a.value == "threshold_crossing"));

    assert!(
        !THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        "THRESHOLD_PROCESSING should be cleared after successful threshold crossing"
    );

    let bob_info = message_info(
        &Addr::unchecked("bob"),
        &[Coin {
            denom: "ubluechip".to_string(),
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
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: Some(Decimal::percent(10)),
    };

    // Same-block follower commit is now blocked by the post-threshold
    // cooldown (eliminates the atomic same-block sandwich on the
    // freshly-seeded pool). Pool reserves must remain at the seeded
    // values from Alice's crossing.
    let err = execute(deps.as_mut(), env.clone(), bob_info.clone(), bob_msg).unwrap_err();
    match err {
        ContractError::PostThresholdCooldownActive { until_block } => {
            assert!(
                until_block > env.block.height,
                "cooldown until_block {} must be > current block {}",
                until_block,
                env.block.height
            );
        }
        other => panic!(
            "Expected PostThresholdCooldownActive, got {:?}",
            other
        ),
    }

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        pool_state.reserve0, before.reserve0,
        "Pool reserve0 must NOT change while cooldown blocks Bob's commit"
    );
    assert_eq!(
        pool_state.reserve1, before.reserve1,
        "Pool reserve1 must NOT change while cooldown blocks Bob's commit"
    );
}

#[test]
fn test_concurrent_commits_both_recorded() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
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

    let alice_info = message_info(
        &Addr::unchecked("alice"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(200_000_000),
        }],
    );

    let alice_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(200_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), alice_info.clone(), alice_msg).unwrap();

    assert!(
        COMMIT_LEDGER
            .load(&deps.storage, &alice_info.sender)
            .is_ok(),
        "Alice should remain in commit ledger pending batched distribution"
    );
    assert!(
        DISTRIBUTION_STATE
            .may_load(&deps.storage)
            .unwrap()
            .is_some(),
        "Distribution state should be active for batched payout"
    );

    // Use a smaller amount relative to the pool reserves. With the 20% excess
    // swap cap, Alice's threshold crossing leaves the pool thinner, so Bob's
    // post-threshold commit (swap) must be reasonably sized to stay within spread.
    let bob_amount = Uint128::new(5_000_000);
    let bob_info = message_info(
        &Addr::unchecked("bob"),
        &[Coin {
            denom: "ubluechip".to_string(),
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
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: bob_amount,
        },
        transaction_deadline: None,
        belief_price: None,
        // `Commit` doesn't expose `allow_high_max_spread`; the
        // post-threshold AMM swap path passes None to assert_max_spread,
        // so the hard cap on Bob's max_spread is 5% (the default-cap
        // ceiling). 10% (pre-audit) is now rejected.
        max_spread: Some(Decimal::percent(5)),
    };

    // Same-block follower: rejected by post-threshold cooldown.
    let same_block_err =
        execute(deps.as_mut(), env.clone(), bob_info.clone(), bob_msg.clone()).unwrap_err();
    match same_block_err {
        ContractError::PostThresholdCooldownActive { .. } => {}
        other => panic!(
            "Expected PostThresholdCooldownActive on same-block commit, got {:?}",
            other
        ),
    }
    let pool_state_blocked = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        pool_state_blocked.reserve0, before.reserve0,
        "Reserves must be unchanged while cooldown blocks Bob"
    );

    // After advancing past the cooldown, the same commit succeeds and
    // routes through the post-threshold AMM swap path, recording a
    // reserve increase for Bob's bluechip — confirms the cooldown is
    // a temporary gate, not a permanent block.
    let mut env_after_cooldown = env.clone();
    env_after_cooldown.block.height += pool_core::state::POST_THRESHOLD_COOLDOWN_BLOCKS + 1;
    // Advance time too so the per-user `min_commit_interval` rate-limit
    // (13s) doesn't reject the retry under the same-block timestamp.
    env_after_cooldown.block.time = env_after_cooldown.block.time.plus_seconds(60);
    let bob_res =
        execute(deps.as_mut(), env_after_cooldown, bob_info.clone(), bob_msg).unwrap();

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
                TokenType::Native {
                    denom: "ubluechip".to_string(),
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
        reserve0, // No reserves pre-threshold
        reserve1,
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
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        min_commit_usd_pre_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_PRE_THRESHOLD,
        min_commit_usd_post_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_POST_THRESHOLD,
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

    // Mirrors production instantiate semantics: `oracle_addr` is set to
    // the factory address by default (factory hosts the internal oracle).
    // Tests that exercise the operator-rotatable oracle endpoint should
    // overwrite ORACLE_INFO after this fixture runs.
    let oracle_info = OracleInfo {
        oracle_addr: Addr::unchecked("factory_contract"),
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
    // execute_simple_swap now gates on IS_THRESHOLD_HIT as defense-in-depth
    // (M-5.1 audit fix). The setup helper seeds the flag as false (pre-
    // threshold default); these direct-handler-call tests exercise
    // post-threshold AMM mechanics, so flip it on explicitly.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let offer = TokenInfo {
        info: TokenType::Native {
            denom: "ubluechip".to_string(),
        },
        amount: Uint128::new(100),
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user"), &[]),
        Addr::unchecked("user"),
        offer,
        None,
        None,
        None,
        None);

    // Swap must be rejected when a side is below MINIMUM_LIQUIDITY. The drain
    // guard no longer tries to persist POOL_PAUSED on this path — a Wasm Err
    // return would revert the save — so the pool is "soft-paused" solely by
    // the reserve pre-check firing on every subsequent swap attempt.
    assert!(matches!(
        result,
        Err(ContractError::InsufficientReserves {})
    ));
    assert!(
        POOL_PAUSED.may_load(&deps.storage).unwrap().unwrap_or(false) == false,
        "POOL_PAUSED should not be set by the drain guard (save would be rolled back on chain)"
    );
}

#[test]
fn test_swap_fails_when_pool_already_paused() {
    let mut deps = mock_dependencies();
    setup_pool_with_reserves(&mut deps, Uint128::new(50_000), Uint128::new(50_000));
    // M-5.1 audit fix: execute_simple_swap gates on IS_THRESHOLD_HIT.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    // Manually pause the pool
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let offer = TokenInfo {
        info: TokenType::Native {
            denom: "ubluechip".to_string(),
        },

        amount: Uint128::new(100),
    };

    let result = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user"), &[]),
        Addr::unchecked("user"),
        offer,
        None,
        None,
        None,
        None);

    assert!(matches!(
        result,
        Err(ContractError::PoolPausedLowLiquidity {})
    ));
}
#[test]
fn test_swap_prevented_if_would_deplete_below_minimum() {
    let mut deps = mock_dependencies();

    setup_pool_with_reserves(
        &mut deps,
        Uint128::new(10000), // Well above SWAP_PAUSE_THRESHOLD
        Uint128::new(1100),  // Just above MINIMUM_LIQUIDITY
    );
    // M-5.1 audit fix: execute_simple_swap gates on IS_THRESHOLD_HIT.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let swap_amount = Uint128::new(2000);
    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let offer = TokenInfo {
        info: TokenType::Native {
            denom: "ubluechip".to_string(),
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
        Some(Decimal::percent(10)),
        Some(true),
        None,
    );

    // Pre-audit this test asserted `InsufficientReserves` on a swap that
    // would deplete reserves below `MINIMUM_LIQUIDITY`. The audit's
    // 10%-with-override hard cap on realised slippage now fires first
    // for any swap large enough to deplete reserves to that floor — the
    // pre-cap arithmetic that surfaced the InsufficientReserves error
    // is structurally unreachable. Re-purpose this regression test to
    // confirm the audit's slippage gate IS the now-binding guard for
    // depletion-bordering swaps. The MIN-reserve guard is still
    // exercised from `liquidity_tests` via direct AMM math.
    assert!(
        matches!(result, Err(ContractError::MaxSpreadAssertion {})),
        "Expected MaxSpreadAssertion (audit's pre-MIN-floor slippage cap), got: {:?}",
        result
    );
}

#[test]
fn test_swap_triggers_pause_at_threshold() {
    let mut deps = mock_dependencies();

    // Set one reserve below MINIMUM_LIQUIDITY
    setup_pool_with_reserves(
        &mut deps,
        Uint128::new(99), // Below MINIMUM_LIQUIDITY
        Uint128::new(10000),
    );
    // M-5.1 audit fix: execute_simple_swap gates on IS_THRESHOLD_HIT.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let swap_amount = Uint128::new(10);
    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let offer = TokenInfo {
        info: TokenType::Native {
            denom: "ubluechip".to_string(),
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
        None);

    // The drain guard rejects the swap. On chain, any attempt to persist
    // POOL_PAUSED here would be rolled back with the Err, so the guard
    // deliberately does not touch POOL_PAUSED. The soft-pause is enforced
    // by the pre-check running on every swap attempt.
    assert!(
        matches!(result, Err(ContractError::InsufficientReserves {})),
        "Expected InsufficientReserves at pre-swap check, got: {:?}",
        result
    );
    assert!(
        POOL_PAUSED.may_load(&deps.storage).unwrap().unwrap_or(false) == false,
        "POOL_PAUSED flag must stay unset on the drain-guard path (rollback semantics)"
    );
}

#[test]
fn test_add_liquidity_unpauses_pool() {
    use crate::state::POOL_PAUSED_AUTO;
    let mut deps = mock_dependencies();

    // Setup pool with low reserves and simulate an auto-pause:
    // POOL_PAUSED + POOL_PAUSED_AUTO both true means "paused because
    // a swap or remove dropped reserves below MIN, recoverable via
    // deposit". Without POOL_PAUSED_AUTO, the deposit treats this as
    // a hard pause and refuses to clear it.
    setup_pool_with_reserves(&mut deps, Uint128::new(5000), Uint128::new(5000));
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();
    POOL_PAUSED_AUTO.save(&mut deps.storage, &true).unwrap();

    // Native side only — token1 is a CW20 and flows via TransferFrom,
    // not native attached funds. The reject-extras gate rejects any
    // attached coin whose denom isn't one of the pool's native sides.
    let result = execute_deposit_liquidity(
        deps.as_mut(),
        mock_env(),
        message_info(
            &Addr::unchecked("provider"),
            &[Coin::new(50_000u128, "ubluechip")],
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

    // Native side only — the reject-extras gate rejects any attached
    // coin whose denom isn't one of the pool's native sides; token1 is CW20.
    let result = execute_deposit_liquidity(
        deps.as_mut(),
        mock_env(),
        message_info(
            &Addr::unchecked("provider"),
            &[Coin::new(500u128, "ubluechip")],
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
    // M-5.1 audit fix: execute_simple_swap gates on IS_THRESHOLD_HIT.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let result1 = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user"), &[]),
        Addr::unchecked("user"),
        TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
        None);

    assert!(matches!(
        result1,
        Err(ContractError::InsufficientReserves {})
    ));

    // Test with low reserve1. Use a different sender than the first call —
    // execute_simple_swap now runs the rate-limit check (hoisted from
    // simple_swap so it can share the PoolCtx POOL_SPECS load), which would
    // otherwise reject the same-sender second call with TooFrequentCommits
    // before reaching the reserve guard this test exercises.
    setup_pool_with_reserves(&mut deps, Uint128::new(10), Uint128::new(9999));
    // M-5.1 audit fix: setup_pool_with_reserves resets IS_THRESHOLD_HIT
    // to false, so re-flip it for this second post-threshold scenario.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    POOL_PAUSED.remove(&mut deps.storage); // Reset pause state

    let result2 = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user2"), &[]),
        Addr::unchecked("user2"),
        TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },

            amount: Uint128::new(100),
        },
        None,
        None,
        None,
        None);

    assert!(matches!(
        result2,
        Err(ContractError::InsufficientReserves {})
    ));
}

#[test]
fn test_pause_state_persistence() {
    // A drained pool does not flip POOL_PAUSED via the swap path (that save
    // would be reverted by the Err return on chain). Repeat calls to the
    // swap entry point should each be rejected by the reserve pre-check
    // with InsufficientReserves, NOT the PoolPausedLowLiquidity branch.
    let mut deps = mock_dependencies();
    setup_pool_with_reserves(&mut deps, Uint128::new(15), Uint128::new(15));
    // M-5.1 audit fix: execute_simple_swap gates on IS_THRESHOLD_HIT.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let first = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user1"), &[]),
        Addr::unchecked("user1"),
        TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100),
        },
        None,
        None,
        None,
        None);
    assert!(matches!(first, Err(ContractError::InsufficientReserves {})));

    let second = execute_simple_swap(
        &mut deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("user2"), &[]),
        Addr::unchecked("user2"),
        TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100),
        },
        None,
        None,
        None,
        None);
    assert!(
        matches!(second, Err(ContractError::InsufficientReserves {})),
        "Second swap should still hit the reserve pre-check, not the pause branch. Got: {:?}",
        second
    );
    assert!(
        POOL_PAUSED.may_load(&deps.storage).unwrap().unwrap_or(false) == false,
        "POOL_PAUSED must stay unset — the swap path never persists it"
    );
}

#[test]
fn test_swap_lopsided_pool_after_threshold() {
    // Post-audit: a swap whose realised spread exceeds 10% is rejected
    // even with `allow_high_max_spread = Some(true)`. This test
    // previously validated that a 50%-of-reserve swap (extreme spread)
    // *succeeded*; under the new hard cap it must instead be rejected
    // with `MaxSpreadAssertion`. Re-purposed as a regression test for
    // the audit's slippage cap, preserving the lopsided-pool setup so
    // the cap's behaviour is exercised in the same scenario.
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(1_000_000_000); // 1k bluechip
    pool_state.reserve1 = Uint128::new(100_000_000_000); // 100k tokens
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let env = mock_env();
    let swap_amount = Uint128::new(500_000_000); // 50% of reserve (extreme)

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );

    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: Some(Decimal::percent(10)),
        allow_high_max_spread: Some(true),
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    assert!(
        matches!(err, ContractError::MaxSpreadAssertion {}),
        "lopsided 50%-of-reserve swap must be rejected by the post-audit \
         10% hard cap on realised slippage; got {:?}",
        err
    );
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

    let info = message_info(
        &Addr::unchecked("trader"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: swap_amount,
        }],
    );
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        belief_price: Some(Decimal::percent(1)), // 0.01
        max_spread: Some(Decimal::percent(1)),   // 1% tolerance
        allow_high_max_spread: None,
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion { .. } => {}
        _ => panic!("Expected MaxSpreadAssertion error due to high slippage in lopsided pool"),
    }
}

fn update_oracle_price(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    new_price: Uint128,
) {
    with_factory_oracle(deps, new_price);
}

#[test]
fn test_commit_and_swap_with_price_change() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000_000),
    }]);
    setup_pool_storage(&mut deps);

    let env = mock_env();

    // Set initial price: $1.00 per bluechip (1_000_000 = $1 with 6 decimals)
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    // User1 commits 1000 bluechip at $1.00 = $1000 USD
    let commit_msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(
            &Addr::unchecked("user1"),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(1_000_000_000),
            }],
        ),
        commit_msg,
    )
    .unwrap();

    // Verify commit at $1
    let user_commit = COMMIT_LEDGER
        .load(&deps.storage, &Addr::unchecked("user1"))
        .unwrap();
    assert_eq!(user_commit, Uint128::new(1_000_000_000)); // $1000 USD

    update_oracle_price(&mut deps, Uint128::new(1_500_000)); // $1.50

    // User2 commits 1000 bluechip at $1.50 = $1500 USD
    let commit_msg_2 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_000_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(
            &Addr::unchecked("user2"),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(1_000_000_000),
            }],
        ),
        commit_msg_2,
    )
    .unwrap();

    // Verify user2's commit at $1.50
    let user2_commit = COMMIT_LEDGER
        .load(&deps.storage, &Addr::unchecked("user2"))
        .unwrap();
    assert_eq!(user2_commit, Uint128::new(1_500_000_000)); // $1500 USD

    // Total raised should be $2500
    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(2_500_000_000));

    update_oracle_price(&mut deps, Uint128::new(800_000)); // $0.80

    // User3 commits at crashed price - verify they need more bluechip for same USD value
    let commit_msg_3 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1_250_000_000), // 1250 bluechip
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(
            &Addr::unchecked("user3"),
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(1_250_000_000),
            }],
        ),
        commit_msg_3,
    )
    .unwrap();

    // 1250 bluechip * $0.80 = $1000 USD
    let user3_commit = COMMIT_LEDGER
        .load(&deps.storage, &Addr::unchecked("user3"))
        .unwrap();
    assert_eq!(user3_commit, Uint128::new(1_000_000_000)); // $1000 USD
}
