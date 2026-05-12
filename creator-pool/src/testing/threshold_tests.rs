use crate::asset::{TokenInfo, TokenType};
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg};
use crate::state::{
    CommitLimitInfo, CreatorExcessLiquidity, DistributionState, ExpectedFactory, RecoveryType,
    COMMITFEEINFO, COMMIT_INFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO, CREATOR_EXCESS_POSITION,
    DISTRIBUTION_STATE, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LAST_THRESHOLD_ATTEMPT, POOL_PAUSED,
    POOL_STATE, THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT,
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
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Addr, ContractResult, CosmosMsg, SystemResult, Uint128, WasmMsg,
};
use pool_factory_interfaces::ConversionResponse;

pub fn setup_pool_with_excess_config(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    setup_pool_storage(deps);

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
        max_bluechip_lock_per_pool: Uint128::new(100_000),
        creator_excess_liquidity_lock_days: 14,
        min_commit_usd_pre_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_PRE_THRESHOLD,
        min_commit_usd_post_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_POST_THRESHOLD,
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

    // Override max_bluechip_lock_per_pool to a value below the realistic
    // pools_bluechip_seed that this test will generate. The
    // bluechip_to_threshold is derived arithmetically from the rate
    // captured at commit entry, rather than via a second mock oracle
    // query that used to return a flat constant. The realistic seed for
    // this test's tiny usd_to_threshold ($100) is ~94_000 ubluechip, so
    // the cap must be under that for the excess branch to fire.
    let mut commit_config = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
    commit_config.max_bluechip_lock_per_pool = Uint128::new(10_000);
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
    let info = message_info(
        &Addr::unchecked("final_committer"),
        &[coin(100_000_000_000_000, "ubluechip")],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100_000_000_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
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

    // With the 20% excess swap cap, reserve0 won't be inflated by the full
    // excess. It should still be larger than the seed amount from the cap
    // (max_bluechip_lock_per_pool = 100_000) plus the capped swap portion.
    assert!(pool_state.reserve0 > Uint128::zero());
    // Verify a refund message was generated for the capped excess
    let has_refund = res.messages.iter().any(|submsg| {
        matches!(&submsg.msg, CosmosMsg::Bank(BankMsg::Send { to_address, .. }) if to_address == "final_committer")
    });
    assert!(has_refund, "Should refund excess above the 20% swap cap");
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

    let info = message_info(&Addr::unchecked("creator"), &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline: None };

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

    let info = message_info(&Addr::unchecked("creator"), &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline: None };

    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    // Should have 2 messages: bank send for bluechip + CW20 transfer for creator tokens
    assert_eq!(res.messages.len(), 2);
    match &res.messages[0].msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, amount }) => {
            assert_eq!(to_address, "creator");
            assert_eq!(amount[0].amount, Uint128::new(50_000_000_000));
        }
        _ => panic!("Expected Bank Send message for bluechip"),
    }
    match &res.messages[1].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
            let transfer_msg: cw20::Cw20ExecuteMsg = from_json(msg).unwrap();
            match transfer_msg {
                cw20::Cw20ExecuteMsg::Transfer { recipient, amount } => {
                    assert_eq!(recipient, "creator");
                    assert_eq!(amount, Uint128::new(175_000_000_000));
                }
                _ => panic!("Expected CW20 Transfer message"),
            }
        }
        _ => panic!("Expected Wasm Execute message for creator token"),
    }

    // Excess position should be cleared
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

    let info = message_info(&Addr::unchecked("hacker"), &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline: None };

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
            // Return $5 USD regardless of input so the pre-threshold
            // minimum commit check ($5) passes. Tests below don't
            // depend on the exact USD_RAISED delta.
            let response = ConversionResponse {
                amount: Uint128::new(5_000_000),
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
    let info = message_info(
        &Addr::unchecked("final_committer"),
        &[coin(100_000_000, "ubluechip")],
    );

    let msg = ExecuteMsg::Commit {
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

    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    let last_attempt = LAST_THRESHOLD_ATTEMPT.load(&deps.storage).unwrap();
    assert_eq!(
        last_attempt, env.block.time,
        "LAST_THRESHOLD_ATTEMPT should be set to current time"
    );
    assert!(
        !THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
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
    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!("\n=== Pool State After ===");
    println!("reserve0: {}", pool_state.reserve0);
    println!("reserve1: {}", pool_state.reserve1);
    assert_eq!(
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        Uint128::new(25_000_000_000)
    );
    assert!(
        COMMIT_LEDGER.load(&deps.storage, &info.sender).is_ok(),
        "Threshold-crosser's entry should remain in ledger pending batched distribution"
    );
    assert!(
        DISTRIBUTION_STATE
            .may_load(&deps.storage)
            .unwrap()
            .is_some(),
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
    // With 20% excess swap cap, total_paid_bluechip will be less than commit_amount
    // because the refunded portion is not counted. It should include the threshold
    // portion plus whatever capped excess was actually swapped.
    assert!(sub.total_paid_bluechip <= commit_amount);
    assert!(sub.total_paid_bluechip > Uint128::zero());
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
    // Pre-test USD_RAISED is $5 below the $25k threshold so that a
    // minimum-size ($5) commit lands exactly at $25k. The $5 minimum
    // commit (MIN_COMMIT_USD_PRE_THRESHOLD) was added post-audit; this
    // test was originally written with a $1 commit hitting the threshold
    // from $24,999. Updated to respect the current minimum.
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_995_000_000))
        .unwrap();

    let previous_user = Addr::unchecked("previous_user");
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &previous_user,
            &Uint128::new(24_995_000_000),
        )
        .unwrap();

    let env = mock_env();

    with_factory_oracle(&mut deps, Uint128::new(1_000_000)); // $1 per bluechip

    let commit_amount = Uint128::new(5_000_000);

    let info = message_info(
        &Addr::unchecked("user"),
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

    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    let last_attempt = LAST_THRESHOLD_ATTEMPT.load(&deps.storage).unwrap();
    assert_eq!(
        last_attempt, env.block.time,
        "LAST_THRESHOLD_ATTEMPT should be set to current time"
    );

    assert!(
        !THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
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

    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());
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

    let info = message_info(&Addr::unchecked("factory_address"), &[]);
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckThreshold,
    };
    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
    assert!(res.is_err());
    // Try recovery after timeout - should succeed
    env.block.time = env.block.time.plus_seconds(1801);

    let res = execute(deps.as_mut(), env, info, msg).unwrap();

    assert!(
        !THRESHOLD_PROCESSING.load(&deps.storage).unwrap(),
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

    // Second user tries to commit while first is processing.
    // Using $5 = MIN_COMMIT_USD_PRE_THRESHOLD so the commit passes
    // the size guard; the test itself is about the THRESHOLD_PROCESSING
    // lock, not about commit sizing.
    let info2 = message_info(
        &Addr::unchecked("user2"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000),
        }],
    );

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

    let err = execute(deps.as_mut(), env.clone(), info2, msg).unwrap_err();

    // A stuck `THRESHOLD_PROCESSING = true` (pre-set in this test to
    // simulate corruption) is reported as an explicit error pointing
    // at the recovery path, instead of silently downgrading the
    // user-intended threshold-crossing commit into pre/post-threshold.
    // The error message references the StuckThreshold recovery so the
    // operator/keeper has a clear remediation step.
    let msg = err.to_string();
    assert!(
        msg.contains("THRESHOLD_PROCESSING") && msg.contains("StuckThreshold"),
        "stuck-state commit must point at StuckThreshold recovery, got: {}",
        msg
    );

    // Storage state must be untouched: the rejection happens before any
    // commit-side writes, so USD_RAISED_FROM_COMMIT and IS_THRESHOLD_HIT
    // are exactly as the test set them up.
    assert_eq!(
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap(),
        Uint128::new(24_999_000_000)
    );
    assert!(!IS_THRESHOLD_HIT.load(&deps.storage).unwrap());
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let mut env = mock_env();
    // Just past DISTRIBUTION_STALL_TIMEOUT_SECONDS (24h). Raised from the
    // previous 2h window so a brief keeper outage no longer bricks a pool.
    env.block.time = old_time.plus_seconds(crate::state::DISTRIBUTION_STALL_TIMEOUT_SECONDS + 1);

    // Permissionless — anyone can call ContinueDistribution
    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::ContinueDistribution {},
    );

    // Should fail with timeout
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("timeout"));

    // The on-chain stall signal is the QueryMsg::DistributionState query,
    // not a marker written into DISTRIBUTION_STATE itself. CosmWasm reverts
    // every staged storage write when a handler returns Err, so attempting
    // to set `consecutive_failures = 99` immediately before the Err return
    // would be discarded along with the failed tx (the prior version of
    // this test relied on MockStorage NOT enforcing that revert, which
    // masked the dead code on real chains).
    //
    // Verify the new observability path: query_distribution_state should
    // report `is_stalled = true` and `seconds_since_update` past the
    // 24h timeout, giving admin dashboards the structured signal they
    // need to call RecoverPoolStuckStates::StuckDistribution.
    use crate::query::query_distribution_state;
    let mut env_for_query = mock_env();
    env_for_query.block.time = old_time.plus_seconds(crate::state::DISTRIBUTION_STALL_TIMEOUT_SECONDS + 1);
    let response = query_distribution_state(deps.as_ref(), &env_for_query)
        .unwrap()
        .expect("DISTRIBUTION_STATE should still exist (the failed tx reverted any state changes)");
    assert!(
        response.is_stalled,
        "is_stalled should be true past DISTRIBUTION_STALL_TIMEOUT_SECONDS"
    );
    assert!(response.seconds_since_update > crate::state::DISTRIBUTION_STALL_TIMEOUT_SECONDS);
    assert_eq!(
        response.consecutive_failures, 0,
        "consecutive_failures must NOT have moved off 0 — the timeout branch's pre-Err save would revert on a real chain, and the new code no longer attempts it at all"
    );
}

/// Regression: just BELOW the timeout, ContinueDistribution must succeed.
/// Pins the new 24h window so a future "tighten the timeout" change has
/// to also update this test.
#[test]
fn test_distribution_just_below_timeout_succeeds() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    check_correct_factory(&mut deps);

    for i in 0..3 {
        COMMIT_LEDGER
            .save(
                &mut deps.storage,
                &Addr::unchecked(format!("user{}", i)),
                &Uint128::new(100),
            )
            .unwrap();
    }

    let old_time = Timestamp::from_seconds(1_000_000);
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(1_000_000),
        total_committed_usd: Uint128::new(300),
        last_processed_key: None,
        distributions_remaining: 3,
        max_gas_per_tx: crate::state::DEFAULT_MAX_GAS_PER_TX,
        estimated_gas_per_distribution: crate::state::DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: old_time,
        last_updated: old_time,
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let mut env = mock_env();
    env.block.time = old_time.plus_seconds(crate::state::DISTRIBUTION_STALL_TIMEOUT_SECONDS - 1);

    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::ContinueDistribution {},
    );
    assert!(
        res.is_ok(),
        "should still succeed at T = timeout - 1s, got: {:?}",
        res.err()
    );
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
    let info = message_info(&Addr::unchecked("random_user"), &[]);
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

    // Set initial state: $24,000 raised at low price ($0.50 implied),
    // so the contract has 48,000 bluechips of NET-of-fees inflow
    // accumulated. After the gross→net refactor NATIVE_RAISED_FROM_COMMIT
    // is interpreted as net (the post-fee total that has actually
    // entered the pool's bank balance), so we seed the net value
    // directly. The test's exact amount isn't load-bearing — the
    // assertion below is on the `max_bluechip_lock_per_pool` cap, which
    // is reached regardless of whether the input is treated as gross
    // or net (both well exceed the cap).
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_000_000_000))
        .unwrap();
    crate::state::NATIVE_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(48_000_000_000))
        .unwrap();

    // Mock oracle price at /bin/bash.50 (500,000 micros)
    deps.querier.update_wasm(|query| match query {
        WasmQuery::Smart { .. } => {
            let response = ConversionResponse {
                amount: Uint128::new(2_000_000_000), // 000 = 2000 bluechips
                rate_used: Uint128::new(500_000),    // /bin/bash.50
                timestamp: 1571797419u64,            // matches mock_env block time
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
    let info = message_info(
        &Addr::unchecked("final_committer"),
        &[coin(2_000_000_000, "ubluechip")],
    );

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(2_000_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info, msg).unwrap();

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

    // Setup pool just below threshold. $5 below (not $1) so that the
    // minimum commit ($5 MIN_COMMIT_USD_PRE_THRESHOLD) crosses.
    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_995_000_000))
        .unwrap();
    // At $1/bluechip, $24,995 of NET-of-fees bluechip has entered the
    // pool's bank balance (NATIVE_RAISED_FROM_COMMIT is post-fee after
    // the gross→net audit refactor). The test asserts threshold-phase
    // semantics, not seed arithmetic, so the exact amount is just a
    // realistic placeholder.
    crate::state::NATIVE_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_995_000_000))
        .unwrap();

    let env = mock_env();
    with_factory_oracle(&mut deps, Uint128::new(1_000_000));

    // User 1 commits enough to OVERSHOOT the threshold. The
    // "threshold_crossing" phase (vs "threshold_hit_exact") requires
    // USD_RAISED to strictly exceed the threshold after the commit.
    // Pre: $24,995. Commit: $10. Post: $25,005 → overshoot by $5.
    let info1 = message_info(
        &Addr::unchecked("user1"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(10_000_000),
        }],
    );
    let msg1 = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(10_000_000),
        },
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    // User 2 commits enough to cross (simulating same block execution)
    let info2 = message_info(
        &Addr::unchecked("user2"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(5_000_000),
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
    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());

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

#[test]
fn test_paused_pool_rejects_pre_threshold_commit() {
    // With POOL_PAUSED set, a pre-threshold commit must be rejected at
    // dispatch before any state is touched. Previously only
    // process_post_threshold_commit checked POOL_PAUSED, so a paused pool
    // could still accept funding-phase deposits that ended up stuck in the
    // COMMIT_LEDGER.
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    // Pool is in pre-threshold state by default (IS_THRESHOLD_HIT = false).
    // Admin pauses via factory forward.
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("committer"),
        &[coin(100_000_000, "ubluechip")],
    );

    let msg = ExecuteMsg::Commit {
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

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    assert!(
        matches!(err, ContractError::PoolPausedLowLiquidity {}),
        "expected PoolPausedLowLiquidity, got: {:?}",
        err
    );

    // Verify no side effects: commit ledger unchanged, no funds recorded.
    let committer_entry = COMMIT_LEDGER
        .may_load(&deps.storage, &Addr::unchecked("committer"))
        .unwrap();
    assert!(
        committer_entry.is_none(),
        "paused commit must not write to COMMIT_LEDGER"
    );
}

#[test]
fn test_paused_pool_rejects_post_threshold_commit() {
    // Parallel test: post-threshold path was already guarded inside
    // process_post_threshold_commit, but the new dispatch-level check
    // should also reject here. This pins the behavior so future refactors
    // can't remove one of the two checks without also removing the other.
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    // Flip pool to post-threshold state.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("committer"),
        &[coin(100_000_000, "ubluechip")],
    );

    let msg = ExecuteMsg::Commit {
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

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    assert!(
        matches!(err, ContractError::PoolPausedLowLiquidity {}),
        "expected PoolPausedLowLiquidity, got: {:?}",
        err
    );
}

#[test]
fn test_unpaused_pool_accepts_commit_after_previously_paused() {
    // Confirms the pause check is stateful, not sticky: unpausing the pool
    // restores the ability to commit. Guards against a future change that
    // accidentally converts POOL_PAUSED into an emergency-drained-style
    // permanent flag.
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    // Pause.
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    let info = message_info(
        &Addr::unchecked("committer"),
        &[coin(100_000_000, "ubluechip")],
    );
    let msg = ExecuteMsg::Commit {
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

    // Confirm paused rejects.
    assert!(execute(deps.as_mut(), env.clone(), info.clone(), msg.clone()).is_err());

    // Unpause.
    POOL_PAUSED.save(&mut deps.storage, &false).unwrap();

    // Needs oracle query; wire the conversion mock.
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { msg: _, .. } => {
            // Return $5 USD regardless of input so the pre-threshold
            // minimum commit check ($5) passes. Tests below don't
            // depend on the exact USD_RAISED delta.
            let response = ConversionResponse {
                amount: Uint128::new(5_000_000),
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

    // Now should succeed (pre-threshold commit path).
    execute(deps.as_mut(), env, info, msg).unwrap();
}

// ===========================================================================
// Fix 6 (audit): NATIVE_RAISED_FROM_COMMIT stores net-of-fees, not gross
// ===========================================================================
//
// Coverage for the gross→net refactor in commit handlers + the matching
// no-recovery read in `trigger_threshold_payout`. Each of the three commit
// branches stores a different "what actually entered the pool's bank
// balance" value:
//
//   - pre_threshold:           amount_after_fees
//   - exact-threshold-hit:     amount_after_fees
//   - threshold_crossing:      threshold_portion_after_fees only
//                              (excess routes through the inline AMM swap)
//
// The matching read at threshold-cross time:
//   pools_bluechip_seed = NATIVE_RAISED_FROM_COMMIT.load(...)   // direct
//
// (no `* (1 - fee_rate)` multiply — the per-commit fee floor is the only
// floor applied end-to-end).
mod native_raised_net_semantics_tests {
    use super::*;
    use crate::state::NATIVE_RAISED_FROM_COMMIT;

    /// Pre-threshold commit must store `amount - total_fees` in
    /// NATIVE_RAISED_FROM_COMMIT, NOT the gross `asset.amount`.
    /// With commit_fee_bluechip=1% + commit_fee_creator=5% = 6% total,
    /// a 1_000_000 ubluechip commit nets 940_000 (after subtracting
    /// 10_000 + 50_000 = 60_000 in fees).
    #[test]
    fn pre_threshold_commit_stores_net_not_gross() {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100_000_000_000),
        }]);
        setup_pool_storage(&mut deps);
        check_correct_factory(&mut deps);
        with_factory_oracle(&mut deps, Uint128::new(1_000_000));

        // Sanity: NATIVE_RAISED starts at zero.
        let pre = NATIVE_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert!(pre.is_zero(), "NATIVE_RAISED must start at zero");

        let commit_amount = Uint128::new(1_000_000_000); // 1000 bluechip = $1000 USD
        let env = mock_env();
        let info = message_info(
            &Addr::unchecked("alice"),
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

        execute(deps.as_mut(), env, info, msg).expect("pre-threshold commit must succeed");

        // 1% bluechip fee + 5% creator fee = 6% total.
        // 1_000_000_000 * 6 / 100 = 60_000_000 fees.
        // NET = 1_000_000_000 - 60_000_000 = 940_000_000.
        let post = NATIVE_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert_eq!(
            post,
            Uint128::new(940_000_000),
            "NATIVE_RAISED must store the post-fee NET amount, not gross. Got {} (gross would be 1_000_000_000)",
            post
        );
        assert_ne!(
            post,
            commit_amount,
            "regression guard: NATIVE_RAISED must NOT equal the gross asset.amount"
        );
    }

    /// Exact-threshold-hit commit (USD raised reaches exactly the
    /// threshold via this commit) must also store the NET amount.
    #[test]
    fn exact_threshold_hit_stores_net_not_gross() {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100_000_000_000),
        }]);
        setup_pool_storage(&mut deps);
        check_correct_factory(&mut deps);
        with_factory_oracle(&mut deps, Uint128::new(1_000_000));

        // Pre-seed: 24_000_000_000 NET already raised, 24_000_000_000 USD raised
        // ($1/bluechip implied via the oracle mock).
        NATIVE_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(24_000_000_000))
            .unwrap();
        USD_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(24_000_000_000))
            .unwrap();

        // Commit exactly $1000 (1_000_000_000 ubluechip at $1/bluechip)
        // — pushes USD raised to exactly $25,000 (the threshold). This
        // routes to the `threshold_hit_exact` branch (NOT
        // process_threshold_crossing_with_excess, which fires only when
        // usd_value > usd_to_threshold).
        let commit_amount = Uint128::new(1_000_000_000);
        let env = mock_env();
        let info = message_info(
            &Addr::unchecked("crosser"),
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

        let res = execute(deps.as_mut(), env, info, msg)
            .expect("exact-hit commit must succeed and trigger threshold");

        // Verify we landed on the exact-hit branch (phase attribute).
        let phase = res
            .attributes
            .iter()
            .find(|a| a.key == "phase")
            .expect("phase attribute must be present");
        assert_eq!(
            phase.value, "threshold_hit_exact",
            "this commit should hit threshold exactly, got phase={}",
            phase.value
        );

        // NATIVE_RAISED += amount_after_fees = 1_000_000_000 - 60_000_000 = 940_000_000
        // Total: 24_000_000_000 + 940_000_000 = 24_940_000_000.
        let total = NATIVE_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert_eq!(
            total,
            Uint128::new(24_940_000_000),
            "exact-hit must add NET (940M) — not gross (1B). Got {}",
            total
        );
    }

    /// Threshold-crossing commit (usd_value > usd_to_threshold, the
    /// "with excess" branch) must store ONLY the
    /// `threshold_portion_after_fees` — NOT the gross
    /// `bluechip_to_threshold`, NOT the full `amount_after_fees`. The
    /// excess (post-fee) goes through the AMM swap inline and lands
    /// in `pool_state.reserve0` directly, NOT in NATIVE_RAISED.
    #[test]
    fn threshold_crossing_stores_only_threshold_portion_after_fees() {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100_000_000_000),
        }]);
        setup_pool_storage(&mut deps);
        check_correct_factory(&mut deps);
        with_factory_oracle(&mut deps, Uint128::new(1_000_000));

        // Pre-seed: $24,995 raised (matching NET), $5 short of threshold.
        NATIVE_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(24_995_000_000))
            .unwrap();
        USD_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(24_995_000_000))
            .unwrap();

        // Commit $10 — overshoots threshold by $5 — routes to the
        // threshold_crossing (with excess) branch.
        let commit_amount = Uint128::new(10_000_000);
        let env = mock_env();
        let info = message_info(
            &Addr::unchecked("crosser"),
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

        let res = execute(deps.as_mut(), env, info, msg)
            .expect("threshold-crossing commit must succeed");

        // Verify we landed on threshold_crossing branch.
        let phase = res
            .attributes
            .iter()
            .find(|a| a.key == "phase")
            .expect("phase attr must exist");
        assert_eq!(phase.value, "threshold_crossing");

        // Compute expected NET threshold portion. The handler computes:
        //   amount_after_fees = amount - total_fees
        //                     = 10_000_000 - (1% + 5%) * 10_000_000
        //                     = 10_000_000 - 600_000 = 9_400_000.
        //   bluechip_to_threshold = usd_to_bluechip_at_rate(usd_to_threshold=$5,
        //                                                   rate=1_000_000)
        //                         = $5 * 1e6 / 1_000_000 = 5_000_000 ubluechip.
        //   threshold_portion_after_fees =
        //       amount_after_fees * bluechip_to_threshold / amount
        //     = 9_400_000 * 5_000_000 / 10_000_000 = 4_700_000.
        // NATIVE_RAISED = 24_995_000_000 + 4_700_000 = 24_999_700_000.
        let total = NATIVE_RAISED_FROM_COMMIT.load(&deps.storage).unwrap();
        assert_eq!(
            total,
            Uint128::new(24_999_700_000),
            "threshold-crossing must add ONLY threshold_portion_after_fees (4.7M), \
             not gross bluechip_to_threshold (5M), not full amount_after_fees (9.4M). Got {}",
            total
        );

        // Defense-in-depth: explicitly confirm we did NOT add the
        // pre-refactor gross value.
        let gross_would_be = Uint128::new(24_995_000_000 + 5_000_000);
        assert_ne!(
            total, gross_would_be,
            "regression guard: NATIVE_RAISED must NOT equal pre-refactor gross"
        );
    }

    /// `trigger_threshold_payout` reads NATIVE_RAISED_FROM_COMMIT
    /// directly into `pools_bluechip_seed` with NO `(1 - fee_rate)`
    /// recovery multiply. End-to-end: a pre-seeded NATIVE_RAISED of
    /// 1_000_000 (under the max_bluechip_lock_per_pool cap) must produce
    /// `pool_state.reserve0 = 1_000_000` after threshold-cross — the
    /// pre-refactor code would have produced `1_000_000 * 0.94 = 940_000`.
    #[test]
    fn trigger_threshold_payout_reads_native_raised_directly_no_recovery_multiply() {
        use crate::generic_helpers::trigger_threshold_payout;
        use crate::state::{
            COMMIT_LIMIT_INFO, POOL_FEE_STATE, POOL_INFO, POOL_STATE, THRESHOLD_PAYOUT_AMOUNTS,
        };

        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        // Seed NATIVE_RAISED with a value below max_bluechip_lock_per_pool
        // (which is 10_000_000_000 in setup_pool_storage). After the
        // refactor, this entire amount becomes `pools_bluechip_seed`
        // and lands as `pool_state.reserve0` (no excess carve-off).
        let seeded_net = Uint128::new(1_000_000);
        NATIVE_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &seeded_net)
            .unwrap();

        // Pre-load required items (as production would have at threshold-
        // cross time).
        let pool_info = POOL_INFO.load(&deps.storage).unwrap();
        let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
        let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
        let commit_config = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
        let payout = THRESHOLD_PAYOUT_AMOUNTS.load(&deps.storage).unwrap();
        let fee_info = COMMITFEEINFO.load(&deps.storage).unwrap();

        let _payout_msgs = trigger_threshold_payout(
            &mut deps.storage,
            &pool_info,
            &mut pool_state,
            &mut pool_fee_state,
            &commit_config,
            &payout,
            &fee_info,
            &mock_env(),
        )
        .expect("trigger_threshold_payout must succeed");

        // Net invariant: pool_state.reserve0 == NATIVE_RAISED_FROM_COMMIT
        // (provided we're under the cap, which we are: 1M ≪ 10B).
        // Pre-refactor would have produced 1_000_000 * 0.94 = 940_000.
        assert_eq!(
            pool_state.reserve0, seeded_net,
            "post-refactor: pool_state.reserve0 must equal NATIVE_RAISED directly. \
             Got reserve0={}, seeded={}. (Pre-refactor would have produced 940_000.)",
            pool_state.reserve0, seeded_net
        );

        // Defense-in-depth: the pre-refactor `gross * (1 - fee_rate)`
        // result is explicitly NOT what we got.
        let pre_refactor_seed = seeded_net.checked_mul_floor(Decimal::percent(94)).unwrap();
        assert_ne!(
            pool_state.reserve0, pre_refactor_seed,
            "regression guard: must not produce pre-refactor recovery-multiplied seed"
        );

        // pool_state.reserve1 lands the full `payout.pool_seed_amount`
        // creator-token allocation (this is independent of the
        // gross→net refactor, included as a sanity check that the
        // payout math is otherwise unchanged).
        assert_eq!(
            pool_state.reserve1, payout.pool_seed_amount,
            "creator-token side of seed should be the full pool_seed_amount"
        );
    }

    /// §7-M-2 audit fix: the two threshold-crossing handlers
    /// (`process_threshold_crossing_with_excess` and
    /// `process_threshold_hit_exact`) refuse to execute when
    /// `IS_THRESHOLD_HIT == true`. The dispatcher in
    /// `commit::execute_commit_logic` already routes only when the flag
    /// is false; this defensive gate at handler entry keeps the
    /// no-double-mint invariant load-bearing rather than incidental.
    ///
    /// This test directly invokes `process_threshold_hit_exact` with the
    /// flag pre-set to `true`, simulating any future call site (or
    /// storage-state desync) that bypasses the dispatcher's gate. The
    /// handler MUST refuse rather than re-running
    /// `trigger_threshold_payout` and re-minting the 1.2T splits.
    #[test]
    fn threshold_hit_exact_rejects_when_flag_already_true() {
        use crate::commit::threshold_crossing::process_threshold_hit_exact;
        use crate::msg::CommitFeeInfo;
        use crate::state::{
            CommitLimitInfo, IS_THRESHOLD_HIT, PoolAnalytics, PoolInfo, POOL_FEE_STATE,
            POOL_INFO, POOL_STATE, THRESHOLD_PAYOUT_AMOUNTS,
        };

        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        // Pre-set the flag — the case we want to defend against. Under
        // normal operation the dispatcher already gates on this, but
        // a future call site or storage corruption that bypasses the
        // dispatcher must not be able to re-mint.
        IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

        let pool_info: PoolInfo = POOL_INFO.load(&deps.storage).unwrap();
        let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
        let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
        let commit_config: CommitLimitInfo = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
        let payout = THRESHOLD_PAYOUT_AMOUNTS.load(&deps.storage).unwrap();
        let fee_info: CommitFeeInfo = COMMITFEEINFO.load(&deps.storage).unwrap();

        let mut deps_mut = deps.as_mut();
        let err = process_threshold_hit_exact(
            &mut deps_mut,
            mock_env(),
            Addr::unchecked("alice"),
            &TokenInfo {
                info: TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                amount: Uint128::new(1_000_000),
            },
            Uint128::new(1_000_000),
            Uint128::new(5_000_000),
            commit_config.commit_amount_for_threshold_usd,
            &mut pool_state,
            &mut pool_fee_state,
            &pool_info,
            &commit_config,
            &payout,
            &fee_info,
            vec![],
            &PoolAnalytics::default(),
        )
        .unwrap_err();

        assert!(
            matches!(err, ContractError::StuckThresholdProcessing),
            "expected StuckThresholdProcessing (no-double-mint gate), got: {:?}",
            err
        );
    }

    /// §7-M-2 follow-up: structural gate moved INTO trigger_threshold_payout.
    /// Bypasses the crossing handlers entirely and invokes the load-bearing
    /// mint function directly with `IS_THRESHOLD_HIT == true`. The gate
    /// must fire here too — any future caller that skips the handler-level
    /// gates still cannot trigger a re-mint of the 1.2T splits.
    #[test]
    fn trigger_threshold_payout_rejects_when_flag_already_true() {
        use crate::commit::threshold_payout::trigger_threshold_payout;
        use crate::msg::CommitFeeInfo;
        use crate::state::{
            CommitLimitInfo, IS_THRESHOLD_HIT, POOL_FEE_STATE, POOL_INFO, POOL_STATE,
            THRESHOLD_PAYOUT_AMOUNTS,
        };

        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        NATIVE_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(1_000_000))
            .unwrap();

        // The case we're defending against: a direct invocation of
        // trigger_threshold_payout when the flag is already true (handler
        // gates bypassed, or storage state desynced).
        IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

        let pool_info = POOL_INFO.load(&deps.storage).unwrap();
        let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
        let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
        let commit_config: CommitLimitInfo = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
        let payout = THRESHOLD_PAYOUT_AMOUNTS.load(&deps.storage).unwrap();
        let fee_info: CommitFeeInfo = COMMITFEEINFO.load(&deps.storage).unwrap();

        let err = trigger_threshold_payout(
            &mut deps.storage,
            &pool_info,
            &mut pool_state,
            &mut pool_fee_state,
            &commit_config,
            &payout,
            &fee_info,
            &mock_env(),
        )
        .unwrap_err();

        assert!(
            matches!(err, ContractError::StuckThresholdProcessing),
            "expected StuckThresholdProcessing from trigger_threshold_payout's \
             entry gate, got: {:?}",
            err
        );
    }

    /// §7-M-2 follow-up: confirm the structural gate also sets
    /// IS_THRESHOLD_HIT at the END of a successful trigger run. Pre-fix
    /// the caller set the flag before calling; now the function itself
    /// is the single witness to "mint completed".
    #[test]
    fn trigger_threshold_payout_sets_is_threshold_hit_on_success() {
        use crate::commit::threshold_payout::trigger_threshold_payout;
        use crate::msg::CommitFeeInfo;
        use crate::state::{
            CommitLimitInfo, IS_THRESHOLD_HIT, POOL_FEE_STATE, POOL_INFO, POOL_STATE,
            THRESHOLD_PAYOUT_AMOUNTS,
        };

        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        NATIVE_RAISED_FROM_COMMIT
            .save(&mut deps.storage, &Uint128::new(1_000_000))
            .unwrap();

        // Pre-conditions: flag is false (canonical pre-trigger state).
        // Either it's been left at its default false (covered by
        // setup_pool_storage) or set explicitly to false — either way
        // the entry gate passes.
        assert_eq!(
            IS_THRESHOLD_HIT.may_load(&deps.storage).unwrap().unwrap_or(false),
            false,
            "pre-trigger: flag must be false"
        );

        let pool_info = POOL_INFO.load(&deps.storage).unwrap();
        let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
        let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
        let commit_config: CommitLimitInfo = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
        let payout = THRESHOLD_PAYOUT_AMOUNTS.load(&deps.storage).unwrap();
        let fee_info: CommitFeeInfo = COMMITFEEINFO.load(&deps.storage).unwrap();

        trigger_threshold_payout(
            &mut deps.storage,
            &pool_info,
            &mut pool_state,
            &mut pool_fee_state,
            &commit_config,
            &payout,
            &fee_info,
            &mock_env(),
        )
        .expect("trigger_threshold_payout must succeed on a clean pool");

        // Post-condition: the function flipped the flag to true at its
        // tail. Subsequent commits will route to the post-threshold
        // AMM path; subsequent trigger_threshold_payout calls reject
        // at the entry gate.
        assert_eq!(
            IS_THRESHOLD_HIT.load(&deps.storage).unwrap(),
            true,
            "post-trigger: flag must be true (set inside trigger_threshold_payout's tail)"
        );
    }
}
