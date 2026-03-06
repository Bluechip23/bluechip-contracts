use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage},
    to_json_binary, Addr, Binary, Coin, ContractResult, CosmosMsg, Decimal, OwnedDeps,
    StdError, SystemError, SystemResult, Timestamp, Uint128, WasmQuery,
};
use std::str::FromStr;

use crate::asset::{PoolPairType, TokenInfo, TokenType};
use crate::contract::{execute, execute_simple_swap, migrate};
use crate::error::ContractError;
use crate::liquidity::execute_deposit_liquidity;
use crate::msg::{CommitFeeInfo, ExecuteMsg, MigrateMsg};
use crate::state::{
    CommitLimitInfo, DistributionState, ExpectedFactory, OracleInfo, Position,
    PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, PoolState, RecoveryType,
    ThresholdPayoutAmounts, COMMITFEEINFO, COMMIT_LEDGER, COMMIT_LIMIT_INFO,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
    DISTRIBUTION_STATE, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS,
    NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID, ORACLE_INFO, POOL_FEE_STATE,
    POOL_INFO, POOL_SPECS, POOL_STATE, RATE_LIMIT_GUARD, THRESHOLD_PAYOUT_AMOUNTS,
    THRESHOLD_PROCESSING, USD_RAISED_FROM_COMMIT, MINIMUM_LIQUIDITY,
};
use crate::liquidity::{execute_collect_fees};
use crate::liquidity_helpers::sync_position_on_transfer;
use crate::testing::liquidity_tests::{create_test_position, setup_pool_post_threshold, setup_pool_storage};
use crate::testing::swap_tests::with_factory_oracle;
use crate::state::{EMERGENCY_DRAINED, OWNER_POSITIONS, PENDING_EMERGENCY_WITHDRAW, POOL_PAUSED};

fn mock_dependencies_with_balance(
    balances: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    deps.querier
        .update_balance(cosmwasm_std::testing::MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}

#[test]
fn test_c1_swap_reserve_deducts_return_and_commission() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let initial_state = POOL_STATE.load(&deps.storage).unwrap();
    let initial_reserve0 = initial_state.reserve0; // 23.5k bluechip
    let initial_reserve1 = initial_state.reserve1; // 350k creator tokens

    let initial_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(initial_fee_state.fee_reserve_1, Uint128::zero());

    // Perform a swap: bluechip -> creator token (small amount to avoid MaxSpreadAssertion)
    let swap_amount = Uint128::new(100_000_000); // 100 bluechip (small vs 23.5k reserve)
    let env = mock_env();
    let user = Addr::unchecked("swapper");

    let mut deps_mut = deps.as_mut();
    let res = execute_simple_swap(
        &mut deps_mut,
        env.clone(),
        mock_info(user.as_str(), &[Coin { denom: "ubluechip".to_string(), amount: swap_amount }]),
        user.clone(),
        TokenInfo {
            info: TokenType::Bluechip { denom: "ubluechip".to_string() },
            amount: swap_amount,
        },
        None,
        Some(Decimal::percent(50)), // Allow wide spread for test
        None,
    ).unwrap();

    let post_state = POOL_STATE.load(&deps.storage).unwrap();
    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();

    // reserve0 should increase by swap_amount
    assert_eq!(post_state.reserve0, initial_reserve0 + swap_amount);

    // The commission was collected in fee_reserve_1 (ask side)
    let commission_in_reserve = post_fee_state.fee_reserve_1;
    assert!(commission_in_reserve > Uint128::zero(), "Commission should be tracked in fee_reserve");

    let tokens_sent = res.messages.iter()
        .filter_map(|m| {
            if let CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { msg, .. }) = &m.msg {
                // CW20 transfer message
                if let Ok(cw20::Cw20ExecuteMsg::Transfer { amount, .. }) = cosmwasm_std::from_json(msg) {
                    return Some(amount);
                }
            }
            None
        })
        .next()
        .unwrap_or(Uint128::zero());

    // Total accounting: reserve1 + fee_reserve_1 + sent_to_user = original_reserve1
    let total_accounted = post_state.reserve1 + commission_in_reserve + tokens_sent;
    assert_eq!(
        total_accounted, initial_reserve1,
        "C-1 regression: reserve1 ({}) + fee_reserve_1 ({}) + sent ({}) must equal initial_reserve1 ({})",
        post_state.reserve1, commission_in_reserve, tokens_sent, initial_reserve1
    );
}

#[test]
fn test_c3_recover_stuck_reentrancy_guard() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Set up the factory address for authorization
    EXPECTED_FACTORY.save(&mut deps.storage, &ExpectedFactory {
        expected_factory_address: Addr::unchecked("factory_contract"),
    }).unwrap();

    // Simulate stuck reentrancy guard
    RATE_LIMIT_GUARD.save(&mut deps.storage, &true).unwrap();
    assert!(RATE_LIMIT_GUARD.load(&deps.storage).unwrap());

    let env = mock_env();
    let factory_info = mock_info("factory_contract", &[]);

    // Recover via RecoverStuckStates
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let res = execute(deps.as_mut(), env, factory_info, msg).unwrap();

    // Guard should be reset
    assert!(!RATE_LIMIT_GUARD.load(&deps.storage).unwrap());

    // Check response attributes
    let recovered_attr = res.attributes.iter()
        .find(|a| a.key == "recovered")
        .expect("Should have 'recovered' attribute");
    assert!(recovered_attr.value.contains("reentrancy_guard"));
}

#[test]
fn test_c3_recover_stuck_reentrancy_guard_unauthorized() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY.save(&mut deps.storage, &ExpectedFactory {
        expected_factory_address: Addr::unchecked("factory_contract"),
    }).unwrap();

    RATE_LIMIT_GUARD.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    // Not the factory - should fail
    let hacker_info = mock_info("hacker", &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    // Guard still stuck
    assert!(RATE_LIMIT_GUARD.load(&deps.storage).unwrap());
}

#[test]
fn test_c3_recover_not_stuck_returns_error() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY.save(&mut deps.storage, &ExpectedFactory {
        expected_factory_address: Addr::unchecked("factory_contract"),
    }).unwrap();

    // Guard is NOT stuck
    RATE_LIMIT_GUARD.save(&mut deps.storage, &false).unwrap();

    let env = mock_env();
    let factory_info = mock_info("factory_contract", &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let err = execute(deps.as_mut(), env, factory_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::NothingToRecover {}));
}

#[test]
fn test_c3_recover_both_resets_all_stuck_states() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY.save(&mut deps.storage, &ExpectedFactory {
        expected_factory_address: Addr::unchecked("factory_contract"),
    }).unwrap();

    // Simulate both stuck reentrancy guard and stuck threshold
    RATE_LIMIT_GUARD.save(&mut deps.storage, &true).unwrap();
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();

    // Set last threshold attempt to far in the past so it qualifies as stuck
    use crate::state::LAST_THRESHOLD_ATTEMPT;
    LAST_THRESHOLD_ATTEMPT.save(
        &mut deps.storage,
        &Timestamp::from_seconds(0),
    ).unwrap();

    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(7200); // 2 hours later

    let factory_info = mock_info("factory_contract", &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::Both,
    };

    let res = execute(deps.as_mut(), env, factory_info, msg).unwrap();

    // Both should be reset
    assert!(!RATE_LIMIT_GUARD.load(&deps.storage).unwrap());
    assert!(!THRESHOLD_PROCESSING.load(&deps.storage).unwrap());

    let recovered_attr = res.attributes.iter()
        .find(|a| a.key == "recovered")
        .expect("Should have 'recovered' attribute");
    assert!(recovered_attr.value.contains("reentrancy_guard"));
    assert!(recovered_attr.value.contains("threshold"));
}


#[test]
fn test_m4_first_deposit_locks_minimum_liquidity() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Reset pool state to have no liquidity (simulate fresh post-threshold pool)
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.total_liquidity = Uint128::zero();
    pool_state.reserve0 = Uint128::zero();
    pool_state.reserve1 = Uint128::zero();
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();

    let env = mock_env();
    let user = Addr::unchecked("first_depositor");
    let bluechip_amount = Uint128::new(1_000_000_000); // 1k
    let token_amount = Uint128::new(1_000_000_000);    // 1k

    let info = mock_info(user.as_str(), &[Coin {
        denom: "ubluechip".to_string(),
        amount: bluechip_amount,
    }]);

    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user.clone(),
        bluechip_amount,
        token_amount,
        None,
        None,
        None,
    ).unwrap();

    let pool_state_after = POOL_STATE.load(&deps.storage).unwrap();

    // NEXT_POSITION_ID starts at 1, increments to 2 on first deposit, so position ID is "2"
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap();

    // sqrt(1k * 1k) = 1k = 1_000_000_000 units raw
    // Position should get raw - MINIMUM_LIQUIDITY = 1_000_000_000 - 1000 = 999_999_000
    let raw_liquidity = crate::liquidity_helpers::integer_sqrt(
        bluechip_amount.checked_mul(token_amount).unwrap()
    );
    let expected_user_liquidity = raw_liquidity - MINIMUM_LIQUIDITY;

    assert_eq!(
        position.liquidity, expected_user_liquidity,
        "M-4 regression: first depositor should get sqrt(a*b) - MINIMUM_LIQUIDITY ({}) but got {}",
        expected_user_liquidity, position.liquidity
    );

    // total_liquidity tracks only assigned liquidity (not the locked minimum).
    // The locked amount is implicit: reserves hold more value than total_liquidity accounts for.
    assert_eq!(
        pool_state_after.total_liquidity, position.liquidity,
        "total_liquidity should equal position liquidity (locked minimum is not tracked in total_liquidity)"
    );

    // The key M-4 check: position got LESS than the raw sqrt due to the lock
    assert!(
        position.liquidity < raw_liquidity,
        "M-4 regression: position liquidity should be less than raw sqrt due to MINIMUM_LIQUIDITY lock"
    );
}

#[test]
fn test_m5_distribution_bounty_from_fee_reserves() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Set up factory address
    EXPECTED_FACTORY.save(&mut deps.storage, &ExpectedFactory {
        expected_factory_address: Addr::unchecked("factory_contract"),
    }).unwrap();

    // Seed some fee reserves (bluechip side)
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_reserve_0 = Uint128::new(10_000_000); // 10 bluechip in fees
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let initial_reserve0 = POOL_STATE.load(&deps.storage).unwrap().reserve0;

    // Set up distribution state with committers
    let committer = Addr::unchecked("committer1");
    COMMIT_LEDGER.save(&mut deps.storage, &committer, &Uint128::new(5_000_000_000)).unwrap();

    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(500_000_000_000),
        total_committed_usd: Uint128::new(25_000_000_000),
        last_processed_key: None,
        distributions_remaining: 1,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(1_600_000_000),
        last_updated: Timestamp::from_seconds(1_600_000_000),
    };
    DISTRIBUTION_STATE.save(&mut deps.storage, &dist_state).unwrap();

    let env = mock_env();
    let caller_info = mock_info("bounty_hunter", &[]);

    let msg = ExecuteMsg::ContinueDistribution {};
    let res = execute(deps.as_mut(), env, caller_info, msg).unwrap();

    // Bounty is now paid from pool reserves (not fee reserves) to avoid
    // distorting fee_growth_global_0 and LP fee accounting.
    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        post_fee_state.fee_reserve_0, fee_state.fee_reserve_0,
        "M-5 regression: fee_reserve_0 should be untouched (bounty comes from reserves)"
    );

    // Check that pool reserves decreased by the bounty amount
    let post_reserve0 = POOL_STATE.load(&deps.storage).unwrap().reserve0;
    assert!(
        post_reserve0 < initial_reserve0,
        "M-5 regression: reserve0 should decrease (bounty paid from reserves)"
    );

    // The bounty_paid attribute should confirm bounty was paid
    let bounty_attr = res.attributes.iter()
        .find(|a| a.key == "bounty_paid")
        .expect("Should have bounty_paid attribute");
    let bounty_amount: u128 = bounty_attr.value.parse().unwrap();
    assert!(bounty_amount > 0, "Bounty should be non-zero");
}

#[test]
fn test_m6_migrate_rejects_excessive_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Try to set fee to 11% (above 10% cap) - should fail
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::percent(11),
    };

    let err = migrate(deps.as_mut(), env.clone(), msg).unwrap_err();
    assert!(
        err.to_string().contains("must not exceed 10%"),
        "M-6 regression: fees above 10% should be rejected, got: {}",
        err
    );
}

#[test]
fn test_m6_migrate_accepts_valid_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Set fee to exactly 10% (boundary) - should succeed
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::percent(10),
    };

    let res = migrate(deps.as_mut(), env.clone(), msg).unwrap();
    assert!(res.attributes.iter().any(|a| a.key == "action" && a.value == "migrate"));

    // Verify the fee was actually updated
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::percent(10));
}

#[test]
fn test_m6_migrate_accepts_small_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Set fee to 0.3% - typical AMM fee
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::from_str("0.003").unwrap(),
    };

    let res = migrate(deps.as_mut(), env, msg).unwrap();
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::from_str("0.003").unwrap());
}

// ==================== New Audit V2 Regression Tests ====================

/// C-1: Verify that sync_position_on_transfer resets fee checkpoints when
/// position ownership changes, preventing the new owner from claiming fees
/// that accrued before the transfer.
#[test]
fn test_c1_nft_transfer_resets_fee_checkpoints() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Create a position owned by Alice with fee growth snapshots at zero
    create_test_position(&mut deps, 1, "alice", Uint128::new(10_000_000));

    // Simulate fees accruing: advance global fee growth
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("50").unwrap();
    fee_state.fee_growth_global_1 = Decimal::from_str("75").unwrap();
    fee_state.fee_reserve_0 = Uint128::new(1_000_000_000_000);
    fee_state.fee_reserve_1 = Uint128::new(1_000_000_000_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    // Simulate NFT transfer: Bob is now the CW721 owner, but position still
    // has Alice as `position.owner`. Call sync_position_on_transfer as Bob.
    let bob = Addr::unchecked("bob");
    let mut position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.owner, Addr::unchecked("alice"));

    let transferred = sync_position_on_transfer(
        &mut deps.storage,
        &mut position,
        "1",
        &bob,
        &fee_state,
    )
    .unwrap();

    assert!(transferred, "Should detect ownership transfer");
    assert_eq!(position.owner, bob);
    // Fee snapshots should be reset to current globals — Bob gets no pre-transfer fees
    assert_eq!(position.fee_growth_inside_0_last, fee_state.fee_growth_global_0);
    assert_eq!(position.fee_growth_inside_1_last, fee_state.fee_growth_global_1);
    assert_eq!(position.unclaimed_fees_0, Uint128::zero());
    assert_eq!(position.unclaimed_fees_1, Uint128::zero());

    // OWNER_POSITIONS should be updated
    assert!(OWNER_POSITIONS.may_load(&deps.storage, (&Addr::unchecked("alice"), "1")).unwrap().is_none());
    assert!(OWNER_POSITIONS.may_load(&deps.storage, (&bob, "1")).unwrap().is_some());
}

/// C-1: Verify that sync_position_on_transfer is a no-op when owner hasn't changed
#[test]
fn test_c1_no_transfer_no_reset() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    create_test_position(&mut deps, 1, "alice", Uint128::new(10_000_000));

    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("50").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let alice = Addr::unchecked("alice");
    let mut position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    let transferred = sync_position_on_transfer(
        &mut deps.storage,
        &mut position,
        "1",
        &alice,
        &fee_state,
    )
    .unwrap();

    assert!(!transferred, "Should NOT detect transfer when owner is the same");
    // Fee snapshots should remain at zero (original values)
    assert_eq!(position.fee_growth_inside_0_last, Decimal::zero());
}

/// H-1: Verify migrate rejects fees below 0.1% minimum
#[test]
fn test_h1_migrate_rejects_zero_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::zero(),
    };

    let err = migrate(deps.as_mut(), env, msg).unwrap_err();
    assert!(
        err.to_string().contains("at least 0.1%"),
        "H-1 regression: zero fees should be rejected, got: {}",
        err
    );
}

/// H-1: Verify migrate rejects fees just below the 0.1% minimum
#[test]
fn test_h1_migrate_rejects_below_minimum() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::from_str("0.0009").unwrap(), // 0.09% < 0.1%
    };

    let err = migrate(deps.as_mut(), env, msg).unwrap_err();
    assert!(
        err.to_string().contains("at least 0.1%"),
        "H-1 regression: fees below 0.1% should be rejected, got: {}",
        err
    );
}

/// H-1: Verify migrate accepts fees at exactly 0.1% minimum
#[test]
fn test_h1_migrate_accepts_minimum_fee() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::permille(1), // 0.1%
    };

    let res = migrate(deps.as_mut(), env, msg).unwrap();
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::permille(1));
}

/// H-3: Verify ContinueDistribution adjusts fee_growth_global_0 when paying bounty
#[test]
fn test_h3_distribution_bounty_does_not_distort_fee_growth() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Set up fee reserves and some fee growth
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_reserve_0 = Uint128::new(10_000_000); // 10 bluechip
    fee_state.fee_growth_global_0 = Decimal::from_str("100").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let pre_growth = fee_state.fee_growth_global_0;

    // Set up distribution state
    let committer = Addr::unchecked("committer1");
    COMMIT_LEDGER
        .save(&mut deps.storage, &committer, &Uint128::new(5_000_000_000))
        .unwrap();
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(500_000_000_000),
        total_committed_usd: Uint128::new(25_000_000_000),
        last_processed_key: None,
        distributions_remaining: 1,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(1_600_000_000),
        last_updated: Timestamp::from_seconds(1_600_000_000),
    };
    DISTRIBUTION_STATE.save(&mut deps.storage, &dist_state).unwrap();

    let env = mock_env();
    let caller_info = mock_info("bounty_hunter", &[]);
    let msg = ExecuteMsg::ContinueDistribution {};
    execute(deps.as_mut(), env, caller_info, msg).unwrap();

    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    // Bounty is now paid from pool reserves, so fee_growth_global_0 must NOT change.
    // This prevents LP fee accounting distortion (the original M-3 finding).
    assert_eq!(
        post_fee_state.fee_growth_global_0, pre_growth,
        "fee_growth_global_0 must not change when bounty is paid from reserves. Before: {}, After: {}",
        pre_growth,
        post_fee_state.fee_growth_global_0
    );
}

/// M-5: Verify emergency withdrawal clears distribution state
#[test]
fn test_m5_emergency_withdraw_clears_distribution() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Set up an in-progress distribution
    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(500_000_000_000),
        total_committed_usd: Uint128::new(25_000_000_000),
        last_processed_key: None,
        distributions_remaining: 50,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(1_600_000_000),
        last_updated: Timestamp::from_seconds(1_600_000_000),
    };
    DISTRIBUTION_STATE.save(&mut deps.storage, &dist_state).unwrap();

    // Phase 1: initiate emergency withdrawal
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_700_000_000);
    let factory_info = mock_info("factory_contract", &[]);
    execute(deps.as_mut(), env.clone(), factory_info.clone(), ExecuteMsg::EmergencyWithdraw {}).unwrap();

    // Phase 2: execute after timelock (24h + 1s)
    env.block.time = Timestamp::from_seconds(1_700_000_000 + 86_401);
    execute(deps.as_mut(), env, factory_info, ExecuteMsg::EmergencyWithdraw {}).unwrap();

    // Distribution should be cleared
    let post_dist = DISTRIBUTION_STATE.load(&deps.storage).unwrap();
    assert!(
        !post_dist.is_distributing,
        "M-5 regression: distribution should be stopped after emergency withdrawal"
    );
    assert_eq!(
        post_dist.distributions_remaining, 0,
        "M-5 regression: distributions_remaining should be 0 after emergency withdrawal"
    );

    // Pool should be permanently drained
    assert!(EMERGENCY_DRAINED.load(&deps.storage).unwrap());
}
