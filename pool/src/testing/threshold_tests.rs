use crate::asset::{TokenInfo, TokenType};
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg};
use crate::state::{
    CommitLimitInfo, CreatorExcessLiquidity, DistributionState, ExpectedFactory, RecoveryType,
    TokenMetadata, COMMITFEEINFO, COMMIT_INFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO,
    CREATOR_EXCESS_POSITION, DISTRIBUTION_STATE, EXPECTED_FACTORY, IS_THRESHOLD_HIT,
    LAST_THRESHOLD_ATTEMPT, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, POOL_STATE,
    THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
};
use crate::testing::swap_tests::with_factory_oracle;
use crate::{
    contract::execute,
    testing::liquidity_tests::{setup_pool_post_threshold, setup_pool_storage},
};
use cosmwasm_std::testing::{mock_dependencies_with_balance, MockApi, MockQuerier, MockStorage};
use cosmwasm_std::{
    coin, BankMsg, Binary, Coin, Decimal, OwnedDeps, SystemError, Timestamp, WasmQuery,
};
use cosmwasm_std::{
    from_json,
    testing::{mock_dependencies, mock_env, mock_info},
    to_json_binary, Addr, ContractResult, CosmosMsg, SystemResult, Uint128, WasmMsg,
};
use pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg;
use pool_factory_interfaces::ConversionResponse;

pub fn setup_pool_with_excess_config(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    setup_pool_storage(deps);

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(25_000_000_000),
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
        max_bluechip_lock_per_pool: Uint128::new(100_000),
        creator_excess_liquidity_lock_days: 14,
    };

    COMMIT_LIMIT_INFO
        .save(&mut deps.storage, &commit_config)
        .unwrap();

    let fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("ubluechip"),
        creator_wallet_address: Addr::unchecked("creator"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };
    COMMITFEEINFO.save(&mut deps.storage, &fee_info).unwrap();
}

#[test]
fn test_threshold_with_excess_creates_position() {
    let mut deps = mock_dependencies();

    setup_pool_with_excess_config(&mut deps);

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("user1"),
            &Uint128::new(24_900_000_000),
        )
        .unwrap();

    deps.querier.update_wasm(|query| match query {
        WasmQuery::Smart { .. } => {
            let response = ConversionResponse {
                amount: Uint128::new(1_000_000_000),
                rate_used: Uint128::new(1_000_000_000),
                timestamp: 1571797419u64, // matches mock_env block time
            };
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&response).unwrap()))
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query".to_string(),
            request: Binary::default(),
        }),
    });
    let env = mock_env();
    let info = mock_info("final_committer", &[coin(100_000_000_000_000, "ubluechip")]);

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100_000_000_000_000),
        },
        amount: Uint128::new(100_000_000_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    println!(
        "USD raised after commit: {}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap()
    );
    println!(
        "Bluechip reserve: {}",
        POOL_STATE.load(&deps.storage).unwrap().reserve0
    );

    match CREATOR_EXCESS_POSITION.load(&deps.storage) {
        Ok(excess_position) => {
            assert!(excess_position.bluechip_amount > Uint128::zero());

            let fee_info = COMMITFEEINFO.load(&deps.storage).unwrap();
            assert_eq!(excess_position.creator, fee_info.creator_wallet_address);
            assert_eq!(
                excess_position.unlock_time,
                env.block.time.plus_seconds(14 * 86400)
            );
        }
        Err(_) => panic!("Creator excess position should exist"),
    }

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();

    assert!(pool_state.reserve0 > Uint128::new(100_000_000_000));
}

#[test]
fn test_claim_excess_before_unlock_fails() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time: env.block.time.plus_seconds(14 * 86400), // 14 days from now
                excess_nft_id: None,
            },
        )
        .unwrap();

    let info = mock_info("creator", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();

    match err {
        ContractError::PositionLocked { unlock_time } => {
            assert_eq!(
                unlock_time.seconds(),
                mock_env().block.time.seconds() + 14 * 86400
            );
        }
        _ => panic!("Expected PositionLocked error"),
    }

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_ok());
}

#[test]
fn test_claim_excess_after_unlock_succeeds() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    let unlock_time = env.block.time.minus_seconds(100);

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time,
                excess_nft_id: None,
            },
        )
        .unwrap();

    NEXT_POSITION_ID.save(&mut deps.storage, &0u64).unwrap();

    let info = mock_info("creator", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    assert_eq!(res.messages.len(), 1);
    match &res.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
            let mint_msg: Cw721ExecuteMsg<TokenMetadata> = from_json(msg).unwrap();
            match mint_msg {
                Cw721ExecuteMsg::Mint { owner, .. } => {
                    assert_eq!(owner, "creator");
                }
                _ => panic!("Expected Mint message"),
            }
        }
        _ => panic!("Expected Wasm Execute message"),
    }

    // L-3 FIX: Position IDs now use plain numeric format (consistent with execute_deposit_liquidity)
    let position = LIQUIDITY_POSITIONS
        .load(&deps.storage, "1")
        .unwrap();
    assert_eq!(position.owner, Addr::unchecked("creator"));
    assert!(position.liquidity > Uint128::zero());

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_err());
}

#[test]
fn test_claim_excess_wrong_user_fails() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time: env.block.time.minus_seconds(100), // Already unlocked
                excess_nft_id: None,
            },
        )
        .unwrap();

    let info = mock_info("hacker", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_ok());
}

#[test]
fn test_no_excess_when_under_cap() {
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    let mut commit_config = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
    commit_config.max_bluechip_lock_per_pool = Uint128::new(10_000_000_000_000); // 10M bluechip
    COMMIT_LIMIT_INFO
        .save(&mut deps.storage, &commit_config)
        .unwrap();

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("user1"),
            &Uint128::new(24_900_000_000),
        )
        .unwrap();

    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { msg: _, .. } => {
            let response = ConversionResponse {
                amount: Uint128::new(1_000_000),
                rate_used: Uint128::new(1_000_000),
                timestamp: 1571797419u64, // matches mock_env block time
            };
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&response).unwrap()))
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    let info = mock_info("final_committer", &[coin(100_000_000, "ubluechip")]);

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        amount: Uint128::new(100_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env, info, msg).unwrap();

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_err());

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(10_000_000_000_000));
}

fn check_correct_factory(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_address"),
            },
        )
        .unwrap();
}
#[test]
fn test_commit_threshold_overshoot_split() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap(); // $24,999

    let env = mock_env();

    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    let commit_amount = Uint128::new(5_000_000);

    let info = mock_info(
        "whale",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    let last_attempt = LAST_THRESHOLD_ATTEMPT.load(&deps.storage).unwrap();
    assert_eq!(
        last_attempt, env.block.time,
        "LAST_THRESHOLD_ATTEMPT should be set to current time"
    );
    assert_eq!(
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        false,
        "THRESHOLD_PROCESSING should be cleared after successful threshold crossing"
    );

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

    // H-1 FIX: Distribution is now always batched. The threshold-crosser's entry is
    // retained in COMMIT_LEDGER until ContinueDistribution pays them out.
    assert!(
        COMMIT_LEDGER.load(&deps.storage, &info.sender).is_ok(),
        "Threshold-crosser's entry should remain in ledger pending batched distribution"
    );
    assert!(
        DISTRIBUTION_STATE.may_load(&deps.storage).unwrap().is_some(),
        "Distribution state should be initialized for batched payout"
    );

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
    assert_eq!(sub.total_paid_bluechip, commit_amount);
    assert_eq!(sub.total_paid_usd, Uint128::new(5_000_000));

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
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    THRESHOLD_PROCESSING
        .save(&mut deps.storage, &false)
        .unwrap();
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap();

    let previous_user = Addr::unchecked("previous_user");
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &previous_user,
            &Uint128::new(24_999_000_000),
        )
        .unwrap();

    let env = mock_env();

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip

    let commit_amount = Uint128::new(1_000_000);

    let info = mock_info(
        "user",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: commit_amount,
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: commit_amount,
        },
        amount: commit_amount,
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    let last_attempt = LAST_THRESHOLD_ATTEMPT.load(&deps.storage).unwrap();
    assert_eq!(
        last_attempt, env.block.time,
        "LAST_THRESHOLD_ATTEMPT should be set to current time"
    );

    assert_eq!(
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        false,
        "THRESHOLD_PROCESSING should be cleared after threshold hit"
    );

    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "phase")
            .unwrap()
            .value,
        "threshold_hit_exact"
    );

    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    let total_usd = USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(25_000_000_000)); // Should be exactly at $25k threshold
}

#[test]
fn test_recover_stuck_threshold() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);
    let mut env = mock_env();
    LAST_THRESHOLD_ATTEMPT
        .save(&mut deps.storage, &env.block.time)
        .unwrap();
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();
    env.block.time = env.block.time.plus_seconds(1800); // 30 minutes later

    let info = mock_info("factory_address", &[]);
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckThreshold,
    };
    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
    assert!(res.is_err());
    // Try recovery after timeout - should succeed
    env.block.time = env.block.time.plus_seconds(1801);

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    assert_eq!(
        THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
        false,
        "THRESHOLD_PROCESSING should be cleared"
    );

    assert!(res.attributes.iter().any(|a| a.value.contains("threshold")));
}

#[test]
fn test_concurrent_threshold_crossing_attempts() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap();

    let env = mock_env();
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    // First user triggers threshold crossing
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();
    LAST_THRESHOLD_ATTEMPT
        .save(&mut deps.storage, &env.block.time)
        .unwrap();

    // Second user tries to commit while first is processing
    let info2 = mock_info(
        "user2",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(2_000_000),
        }],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(2_000_000),
        },
        amount: Uint128::new(2_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env.clone(), info2, msg);

    // Should succeed but process as pre-threshold commit
    assert!(res.is_ok());
    let res = res.unwrap();

    // Should NOT have threshold_crossing phase since someone else is processing
    assert!(res
        .attributes
        .iter()
        .find(|a| a.key == "phase")
        .map_or(true, |a| a.value != "threshold_crossing"));
}

#[test]
fn test_distribution_timeout_triggers_error() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);
    // Add committers
    for i in 0..5 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }

    let old_time = Timestamp::from_seconds(1000);

    // Create old distribution state
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(1_000_000),
        last_processed_key: None,
        distributions_remaining: 5,
        max_gas_per_tx: 1000,
        estimated_gas_per_distribution: 50,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: old_time,
        last_updated: old_time, // Very old
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let mut env = mock_env();
    env.block.time = old_time.plus_seconds(7201); // Over 2 hours later

    // Permissionless — anyone can call ContinueDistribution
    let info = mock_info("anyone", &[]);
    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::ContinueDistribution {},
    );

    // Should fail with timeout
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("timeout"));

    // State should be marked as failed
    let updated = DISTRIBUTION_STATE.load(&deps.storage).unwrap();
    assert_eq!(updated.consecutive_failures, 99);
}

#[test]
fn test_unauthorized_recovery_attempt() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);
    let env = mock_env();

    // Set stuck state
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();

    // Non-admin tries to recover
    let info = mock_info("random_user", &[]);
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckThreshold,
    };

    let res = execute(deps.as_mut(), env, info, msg);

    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ContractError::Unauthorized {}));
}
#[test]
fn test_accumulated_bluechips_respected() {
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    // Set initial state: 4,000 raised, but with LOW price (/bin/bash.50)
    // So we should have collected 48,000 bluechips already
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_000_000_000))
        .unwrap();

    // CRITICAL: We must manually set the NATIVE_RAISED_FROM_COMMIT to reflect the low price history
    // 48,000 bluechips for 4,000 USD
    crate::state::NATIVE_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(48_000_000_000))
        .unwrap();

    // Mock oracle price at /bin/bash.50 (500,000 micros)
    deps.querier.update_wasm(|query| match query {
        WasmQuery::Smart { .. } => {
            let response = ConversionResponse {
                amount: Uint128::new(2_000_000_000), // 000 = 2000 bluechips
                rate_used: Uint128::new(500_000),    // /bin/bash.50
                timestamp: 1571797419u64, // matches mock_env block time
            };
            SystemResult::Ok(ContractResult::Ok(to_json_binary(&response).unwrap()))
        }
        _ => SystemResult::Err(SystemError::InvalidRequest {
            error: "Unknown query".to_string(),
            request: Binary::default(),
        }),
    });

    let env = mock_env();
    // Commit remaining ,000 (requires 2,000 bluechips at /bin/bash.50)
    let info = mock_info("final_committer", &[coin(2_000_000_000, "ubluechip")]);

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(2_000_000_000),
        },
        amount: Uint128::new(2_000_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Total bluechips = 48,000 + 2,000 = 50,000, after 6% fees = ~47,000
    // Max bluechip lock is 100,000 (from setup_pool_with_excess_config)
    // Since 47,000,000,000 > 100,000 cap, excess path triggers
    // Reserves should only contain the CAPPED amount, not the full amount
    // Excess is held separately in CREATOR_EXCESS_POSITION until creator claims

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();

    println!("Reserve0: {}", pool_state.reserve0);
    // Reserves should be at the cap, NOT the full accumulated amount
    assert_eq!(
        pool_state.reserve0,
        Uint128::new(100_000),
        "Pool reserves should be capped at max_bluechip_lock_per_pool"
    );

    // The excess should be stored in the creator excess position
    let excess = crate::state::CREATOR_EXCESS_POSITION
        .load(&deps.storage)
        .unwrap();
    assert!(
        excess.bluechip_amount > Uint128::zero(),
        "Excess bluechip should be stored for creator to claim later"
    );
    // Total (capped reserves + excess) should account for all accumulated bluechips
    assert!(
        pool_state.reserve0 + excess.bluechip_amount > Uint128::new(40_000_000_000),
        "Total bluechips (reserves + excess) should reflect accumulated amount"
    );
}

#[test]
fn test_concurrent_threshold_crossing_race_condition() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);

    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);

    // Setup pool just below threshold
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_999_000_000))
        .unwrap();

    let env = mock_env();
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    // User 1 commits enough to cross
    let info1 = mock_info(
        "user1",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(2_000_000),
        }],
    );
    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(2_000_000),
        },
        amount: Uint128::new(2_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    // User 2 commits enough to cross (simulating same block execution)
    let info2 = mock_info(
        "user2",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(2_000_000),
        }],
    );
    let msg2 = msg1.clone();

    // Execute User 1 - Should trigger threshold
    let res1 = execute(deps.as_mut(), env.clone(), info1, msg1).unwrap();

    // Verify User 1 triggered threshold
    assert!(res1
        .attributes
        .iter()
        .any(|a| a.key == "phase" && a.value == "threshold_crossing"));
    assert_eq!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap(), true);

    let res2 = execute(deps.as_mut(), env.clone(), info2, msg2);

    match res2 {
        Ok(res) => {
            // If it succeeds, it implies it might have been processed.
            // But `Commit` message is usually strictly for pre-threshold.
            // If it succeeds, we want to ensure it didn't trigger threshold AGAIN.
            assert!(!res
                .attributes
                .iter()
                .any(|a| a.key == "phase" && a.value == "threshold_crossing"));
        }
        Err(e) => {
            // Other errors might be acceptable depending on implementation
            println!("User 2 failed with: {:?}", e);
        }
    }
}
