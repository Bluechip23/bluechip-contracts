use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage},
    Addr, Coin, CosmosMsg, Decimal, OwnedDeps, Timestamp, Uint128, WasmMsg,
};
use std::str::FromStr;

use crate::asset::{TokenInfo, TokenType};
use crate::contract::{execute, execute_simple_swap, migrate};
use crate::error::ContractError;
use crate::liquidity::execute_deposit_liquidity;
use crate::liquidity_helpers::sync_position_on_transfer;
use crate::msg::{ExecuteMsg, MigrateMsg};
use crate::state::{
    DistributionState, ExpectedFactory, RecoveryType, COMMIT_LEDGER,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX, DISTRIBUTION_STATE,
    EXPECTED_FACTORY, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, NEXT_POSITION_ID, POOL_FEE_STATE,
    POOL_SPECS, POOL_STATE, REENTRANCY_GUARD, THRESHOLD_PROCESSING,
};
use crate::state::{EMERGENCY_DRAINED, OWNER_POSITIONS};
use crate::testing::liquidity_tests::{
    create_test_position, setup_pool_post_threshold, setup_pool_storage,
};

#[allow(dead_code)]
fn mock_dependencies_with_balance(
    balances: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    deps.querier
        .bank
        .update_balance(cosmwasm_std::testing::MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}

#[test]
fn test_swap_reserve_deducts_return_and_commission() {
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
        message_info(
            &user,
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: swap_amount,
            }],
        ),
        user.clone(),
        TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        None,
        Some(Decimal::percent(50)), // Allow wide spread for test
        None,
    )
    .unwrap();

    let post_state = POOL_STATE.load(&deps.storage).unwrap();
    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();

    // reserve0 should increase by swap_amount
    assert_eq!(post_state.reserve0, initial_reserve0 + swap_amount);

    // The commission was collected in fee_reserve_1 (ask side)
    let commission_in_reserve = post_fee_state.fee_reserve_1;
    assert!(
        commission_in_reserve > Uint128::zero(),
        "Commission should be tracked in fee_reserve"
    );

    let tokens_sent = res
        .messages
        .iter()
        .filter_map(|m| {
            if let CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { msg, .. }) = &m.msg {
                // CW20 transfer message
                if let Ok(cw20::Cw20ExecuteMsg::Transfer { amount, .. }) =
                    cosmwasm_std::from_json(msg)
                {
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
        "reserve1 ({}) + fee_reserve_1 ({}) + sent ({}) must equal initial_reserve1 ({})",
        post_state.reserve1, commission_in_reserve, tokens_sent, initial_reserve1
    );
}

#[test]
fn test_recover_stuck_reentrancy_guard() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Set up the factory address for authorization
    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Simulate stuck reentrancy guard
    REENTRANCY_GUARD.save(&mut deps.storage, &true).unwrap();
    assert!(REENTRANCY_GUARD.load(&deps.storage).unwrap());

    let env = mock_env();
    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);

    // Recover via RecoverStuckStates
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let res = execute(deps.as_mut(), env, factory_info, msg).unwrap();

    // Guard should be reset
    assert!(!REENTRANCY_GUARD.load(&deps.storage).unwrap());

    // Check response attributes
    let recovered_attr = res
        .attributes
        .iter()
        .find(|a| a.key == "recovered")
        .expect("Should have 'recovered' attribute");
    assert!(recovered_attr.value.contains("reentrancy_guard"));
}

#[test]
fn test_recover_stuck_reentrancy_guard_unauthorized() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    REENTRANCY_GUARD.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    // Not the factory - should fail
    let hacker_info = message_info(&Addr::unchecked("hacker"), &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    // Guard still stuck
    assert!(REENTRANCY_GUARD.load(&deps.storage).unwrap());
}

#[test]
fn test_recover_not_stuck_returns_error() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Guard is NOT stuck
    REENTRANCY_GUARD.save(&mut deps.storage, &false).unwrap();

    let env = mock_env();
    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let err = execute(deps.as_mut(), env, factory_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::NothingToRecover {}));
}

#[test]
fn test_recover_both_resets_all_stuck_states() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Simulate both stuck reentrancy guard and stuck threshold
    REENTRANCY_GUARD.save(&mut deps.storage, &true).unwrap();
    THRESHOLD_PROCESSING.save(&mut deps.storage, &true).unwrap();

    // Set last threshold attempt to far in the past so it qualifies as stuck
    use crate::state::LAST_THRESHOLD_ATTEMPT;
    LAST_THRESHOLD_ATTEMPT
        .save(&mut deps.storage, &Timestamp::from_seconds(0))
        .unwrap();

    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(7200); // 2 hours later

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::Both,
    };

    let res = execute(deps.as_mut(), env, factory_info, msg).unwrap();

    // Both should be reset
    assert!(!REENTRANCY_GUARD.load(&deps.storage).unwrap());
    assert!(!THRESHOLD_PROCESSING.load(&deps.storage).unwrap());

    let recovered_attr = res
        .attributes
        .iter()
        .find(|a| a.key == "recovered")
        .expect("Should have 'recovered' attribute");
    assert!(recovered_attr.value.contains("reentrancy_guard"));
    assert!(recovered_attr.value.contains("threshold"));
}

#[test]
fn test_first_deposit_locks_minimum_liquidity() {
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
    let token_amount = Uint128::new(1_000_000_000); // 1k

    let info = message_info(
        &user,
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: bluechip_amount,
        }],
    );

    let _res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user.clone(),
        bluechip_amount,
        token_amount,
        None,
        None,
        None,
    )
    .unwrap();

    let pool_state_after = POOL_STATE.load(&deps.storage).unwrap();

    // NEXT_POSITION_ID starts at 1, increments to 2 on first deposit, so position ID is "2"
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap();

    // sqrt(1k * 1k) = 1k = 1_000_000_000 units raw
    // Position should get raw - MINIMUM_LIQUIDITY = 1_000_000_000 - 1000 = 999_999_000
    let raw_liquidity =
        crate::liquidity_helpers::integer_sqrt(bluechip_amount.checked_mul(token_amount).unwrap());
    let expected_user_liquidity = raw_liquidity - MINIMUM_LIQUIDITY;

    assert_eq!(
        position.liquidity, expected_user_liquidity,
        "first depositor should get sqrt(a*b) - MINIMUM_LIQUIDITY ({}) but got {}",
        expected_user_liquidity, position.liquidity
    );

    // total_liquidity tracks only assigned liquidity (not the locked minimum).
    // The locked amount is implicit: reserves hold more value than total_liquidity accounts for.
    assert_eq!(
        pool_state_after.total_liquidity, position.liquidity,
        "total_liquidity should equal position liquidity (locked minimum is not tracked in total_liquidity)"
    );

    // Position got LESS than the raw sqrt due to the lock
    assert!(
        position.liquidity < raw_liquidity,
        "position liquidity should be less than raw sqrt due to MINIMUM_LIQUIDITY lock"
    );
}

#[test]
fn test_distribution_bounty_does_not_touch_pool_funds() {
    // Pre-refactor name: test_distribution_bounty_from_reserves.
    //
    // The bounty for distribution batches is now paid by the FACTORY, not
    // skimmed from the pool's own reserve. This test pins the invariant
    // that ContinueDistribution leaves reserve0 and fee_reserve_0
    // completely untouched on the pool side, and that the pool emits a
    // WasmMsg to the factory's PayDistributionBounty endpoint instead of
    // a BankMsg out of its own balance.
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

    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_reserve_0 = Uint128::new(10_000_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let initial_reserve0 = POOL_STATE.load(&deps.storage).unwrap().reserve0;

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
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let caller_info = message_info(&Addr::unchecked("bounty_hunter"), &[]);

    let msg = ExecuteMsg::ContinueDistribution {};
    let res = execute(deps.as_mut(), env, caller_info, msg).unwrap();

    // Fee reserves untouched — pool no longer pays the bounty.
    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        post_fee_state.fee_reserve_0, fee_state.fee_reserve_0,
        "fee_reserve_0 must not change — bounty is now paid by the factory"
    );

    // Pool trading reserves untouched.
    let post_reserve0 = POOL_STATE.load(&deps.storage).unwrap().reserve0;
    assert_eq!(
        post_reserve0, initial_reserve0,
        "reserve0 must not decrease — bounty is now paid by the factory"
    );

    // Confirm the pool emitted a WasmMsg::Execute to the factory's
    // PayDistributionBounty endpoint with the keeper as recipient.
    let factory_msg_present = res.messages.iter().any(|sm| match &sm.msg {
        cosmwasm_std::CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            if contract_addr != "factory_contract" {
                return false;
            }
            // Decode the inner message to check the variant.
            let parsed: Result<pool_factory_interfaces::FactoryExecuteMsg, _> = from_json(msg);
            matches!(
                parsed,
                Ok(pool_factory_interfaces::FactoryExecuteMsg::PayDistributionBounty { recipient })
                    if recipient == "bounty_hunter"
            )
        }
        _ => false,
    });
    assert!(
        factory_msg_present,
        "expected WasmMsg::Execute to factory.PayDistributionBounty, got: {:?}",
        res.messages
    );
}

#[test]
fn test_migrate_rejects_excessive_fees() {
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
        "fees above 10% should be rejected, got: {}",
        err
    );
}

#[test]
fn test_migrate_accepts_valid_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Set fee to exactly 10% (boundary) - should succeed
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::percent(10),
    };

    let res = migrate(deps.as_mut(), env.clone(), msg).unwrap();
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "migrate"));

    // Verify the fee was actually updated
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::percent(10));
}

#[test]
fn test_migrate_accepts_small_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Set fee to 0.3% - typical AMM fee
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::from_str("0.003").unwrap(),
    };

    let _res = migrate(deps.as_mut(), env, msg).unwrap();
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::from_str("0.003").unwrap());
}

// ==================== Additional Regression Tests ====================

/// Verify that sync_position_on_transfer resets fee checkpoints when
/// position ownership changes, preventing the new owner from claiming fees
/// that accrued before the transfer.
#[test]
fn test_nft_transfer_resets_fee_checkpoints() {
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

    let transferred =
        sync_position_on_transfer(&mut deps.storage, &mut position, "1", &bob, &fee_state).unwrap();

    assert!(transferred, "Should detect ownership transfer");
    assert_eq!(position.owner, bob);
    // Fee snapshots should be reset to current globals — Bob gets no pre-transfer fees
    assert_eq!(
        position.fee_growth_inside_0_last,
        fee_state.fee_growth_global_0
    );
    assert_eq!(
        position.fee_growth_inside_1_last,
        fee_state.fee_growth_global_1
    );
    assert_eq!(position.unclaimed_fees_0, Uint128::zero());
    assert_eq!(position.unclaimed_fees_1, Uint128::zero());

    // OWNER_POSITIONS should be updated
    assert!(OWNER_POSITIONS
        .may_load(&deps.storage, (&Addr::unchecked("alice"), "1"))
        .unwrap()
        .is_none());
    assert!(OWNER_POSITIONS
        .may_load(&deps.storage, (&bob, "1"))
        .unwrap()
        .is_some());
}

/// Verify that sync_position_on_transfer is a no-op when owner hasn't changed
#[test]
fn test_no_transfer_no_reset() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    create_test_position(&mut deps, 1, "alice", Uint128::new(10_000_000));

    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("50").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let alice = Addr::unchecked("alice");
    let mut position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    let transferred =
        sync_position_on_transfer(&mut deps.storage, &mut position, "1", &alice, &fee_state)
            .unwrap();

    assert!(
        !transferred,
        "Should NOT detect transfer when owner is the same"
    );
    // Fee snapshots should remain at zero (original values)
    assert_eq!(position.fee_growth_inside_0_last, Decimal::zero());
}

/// Verify migrate rejects fees below 0.1% minimum
#[test]
fn test_migrate_rejects_zero_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::zero(),
    };

    let err = migrate(deps.as_mut(), env, msg).unwrap_err();
    assert!(
        err.to_string().contains("at least 0.1%"),
        "zero fees should be rejected, got: {}",
        err
    );
}

/// Verify migrate rejects fees just below the 0.1% minimum
#[test]
fn test_migrate_rejects_below_minimum() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::from_str("0.0009").unwrap(), // 0.09% < 0.1%
    };

    let err = migrate(deps.as_mut(), env, msg).unwrap_err();
    assert!(
        err.to_string().contains("at least 0.1%"),
        "fees below 0.1% should be rejected, got: {}",
        err
    );
}

/// Verify migrate accepts fees at exactly 0.1% minimum
#[test]
fn test_migrate_accepts_minimum_fee() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::permille(1), // 0.1%
    };

    let _res = migrate(deps.as_mut(), env, msg).unwrap();
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::permille(1));
}

/// Verify ContinueDistribution does not distort fee_growth_global_0 when paying bounty
#[test]
fn test_distribution_bounty_does_not_distort_fee_growth() {
    // Pool's fee_growth_global_0 must not move when ContinueDistribution
    // runs — the pool no longer pays the bounty itself, so there's no
    // accounting path that could touch fee growth. This test guards
    // against a future regression where someone reintroduces a pool-side
    // fee deduction for keeper costs.
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

    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_reserve_0 = Uint128::new(10_000_000);
    fee_state.fee_growth_global_0 = Decimal::from_str("100").unwrap();
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let pre_growth = fee_state.fee_growth_global_0;

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
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let env = mock_env();
    let caller_info = message_info(&Addr::unchecked("bounty_hunter"), &[]);
    let msg = ExecuteMsg::ContinueDistribution {};
    execute(deps.as_mut(), env, caller_info, msg).unwrap();

    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        post_fee_state.fee_growth_global_0, pre_growth,
        "fee_growth_global_0 must not change during distribution. \
         Before: {}, After: {}",
        pre_growth, post_fee_state.fee_growth_global_0
    );
}

/// Verify emergency withdrawal clears distribution state
#[test]
fn test_emergency_withdraw_clears_distribution() {
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
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    // Phase 1: initiate emergency withdrawal
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_700_000_000);
    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    execute(
        deps.as_mut(),
        env.clone(),
        factory_info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Phase 2: execute after timelock (24h + 1s)
    env.block.time = Timestamp::from_seconds(1_700_000_000 + 86_401);
    execute(
        deps.as_mut(),
        env,
        factory_info,
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Distribution should be cleared
    let post_dist = DISTRIBUTION_STATE.load(&deps.storage).unwrap();
    assert!(
        !post_dist.is_distributing,
        "distribution should be stopped after emergency withdrawal"
    );
    assert_eq!(
        post_dist.distributions_remaining, 0,
        "distributions_remaining should be 0 after emergency withdrawal"
    );

    // Pool should be permanently drained
    assert!(EMERGENCY_DRAINED.load(&deps.storage).unwrap());
}
