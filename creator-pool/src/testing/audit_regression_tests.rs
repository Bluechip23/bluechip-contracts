use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage},
    Addr, Coin, CosmosMsg, Decimal, OwnedDeps, Timestamp, Uint128, WasmMsg,
};
use std::str::FromStr;

use crate::asset::{TokenInfo, TokenType};
use crate::contract::{execute, migrate};
use crate::swap_helper::execute_simple_swap;
use crate::error::ContractError;
use crate::liquidity::execute_deposit_liquidity;
use crate::liquidity_helpers::sync_position_on_transfer;
use crate::msg::{ExecuteMsg, MigrateMsg};
use crate::state::{
    DistributionState, ExpectedFactory, RecoveryType, COMMIT_LEDGER,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX, DISTRIBUTION_STATE,
    EXPECTED_FACTORY, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, NEXT_POSITION_ID, POOL_FEE_STATE,
    POOL_SPECS, POOL_STATE, REENTRANCY_LOCK, THRESHOLD_PROCESSING,
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
            info: TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            amount: swap_amount,
        },
        None,
        // Wide-as-allowed slippage: post-audit hard cap is 10% with
        // `allow_high_max_spread = Some(true)`. The test swap is small
        // enough relative to the post-threshold pool reserves that the
        // realised spread fits comfortably under that bound.
        Some(Decimal::percent(10)),
        Some(true),
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
    REENTRANCY_LOCK.save(&mut deps.storage, &true).unwrap();
    assert!(REENTRANCY_LOCK.load(&deps.storage).unwrap());

    let env = mock_env();
    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);

    // Recover via RecoverStuckStates
    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let res = execute(deps.as_mut(), env, factory_info, msg).unwrap();

    // Guard should be reset
    assert!(!REENTRANCY_LOCK.load(&deps.storage).unwrap());

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

    REENTRANCY_LOCK.save(&mut deps.storage, &true).unwrap();

    let env = mock_env();
    // Not the factory - should fail
    let hacker_info = message_info(&Addr::unchecked("hacker"), &[]);

    let msg = ExecuteMsg::RecoverStuckStates {
        recovery_type: RecoveryType::StuckReentrancyGuard,
    };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    // Guard still stuck
    assert!(REENTRANCY_LOCK.load(&deps.storage).unwrap());
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
    REENTRANCY_LOCK.save(&mut deps.storage, &false).unwrap();

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
    REENTRANCY_LOCK.save(&mut deps.storage, &true).unwrap();
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
    assert!(!REENTRANCY_LOCK.load(&deps.storage).unwrap());
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
    // Updated for the locked-on-Position model: the first depositor's
    // Position now carries the FULL `raw_liquidity` plus a
    // `locked_liquidity = MINIMUM_LIQUIDITY` field, so fees accrue
    // against the full position. The lock is enforced on the remove
    // paths (covered by separate tests) rather than by subtracting from
    // `position.liquidity`. `pool_state.total_liquidity` matches.
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
    let raw_liquidity =
        crate::liquidity_helpers::integer_sqrt(bluechip_amount.checked_mul(token_amount).unwrap());

    // Position carries the FULL raw_liquidity as `liquidity`, with
    // MINIMUM_LIQUIDITY of it marked as `locked_liquidity` (cannot be
    // withdrawn but still earns fees against the full position).
    assert_eq!(
        position.liquidity, raw_liquidity,
        "first depositor's position.liquidity should equal raw sqrt(a*b) ({}); got {}",
        raw_liquidity, position.liquidity
    );
    assert_eq!(
        position.locked_liquidity, MINIMUM_LIQUIDITY,
        "first depositor's position.locked_liquidity should equal MINIMUM_LIQUIDITY ({}); got {}",
        MINIMUM_LIQUIDITY, position.locked_liquidity
    );

    // total_liquidity now tracks the FULL raw amount (matches position.liquidity).
    assert_eq!(
        pool_state_after.total_liquidity, raw_liquidity,
        "total_liquidity should equal raw_liquidity (full first-depositor position)"
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
        distributed_so_far: Uint128::zero(),
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

/// Regression: ContinueDistribution must NOT push a PayDistributionBounty
/// message when the call processed zero committers.
///
/// Before the fix, a keeper could collect a free bounty by calling
/// ContinueDistribution after the ledger was already empty but before the
/// state had been cleaned up — the pool would emit an unconditional bounty
/// msg regardless of whether work was done. This test sets up exactly that
/// scenario and asserts (a) the response contains zero messages, (b) the
/// `bounty_paid=false` attribute is emitted, and (c) DISTRIBUTION_STATE is
/// removed in the same tx.
#[test]
fn test_continue_distribution_skips_bounty_on_empty_batch() {
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

    // Distribution is "in progress" by state, but the ledger is empty —
    // matches the post-final-batch window in the old (buggy) flow where
    // the cursor had advanced past the last entry but the state had not
    // yet been cleaned up.
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    // No COMMIT_LEDGER entries — the empty-batch case.

    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_600_000_100);

    let caller = message_info(&Addr::unchecked("bounty_hunter"), &[]);
    let res = execute(
        deps.as_mut(),
        env,
        caller,
        ExecuteMsg::ContinueDistribution {},
    )
    .expect("call should still succeed — it's a clean no-op");

    // No bounty msg emitted (and no mint msgs either, since nothing to mint).
    assert!(
        res.messages.is_empty(),
        "no messages should be emitted on an empty batch, got: {:?}",
        res.messages
    );

    // Attributes should explicitly call out the no-op for observability.
    let bounty_paid = res
        .attributes
        .iter()
        .find(|a| a.key == "bounty_paid")
        .map(|a| a.value.as_str())
        .unwrap_or("");
    assert_eq!(
        bounty_paid, "false",
        "bounty_paid attribute must reflect that no bounty was emitted"
    );
    let processed = res
        .attributes
        .iter()
        .find(|a| a.key == "processed_count")
        .map(|a| a.value.as_str())
        .unwrap_or("");
    assert_eq!(processed, "0", "processed_count must reflect zero work");

    // State must be cleaned up in the same tx (ledger-emptiness termination).
    assert_eq!(
        DISTRIBUTION_STATE.may_load(&deps.storage).unwrap(),
        None,
        "DISTRIBUTION_STATE must be removed when the ledger is empty"
    );
}

/// Regression: when the batch processes the FINAL committer, the bounty IS
/// paid AND the state is removed in the same tx — no extra empty cleanup
/// call required. Pins that the natural-completion path doesn't regress.
#[test]
fn test_continue_distribution_completes_in_one_tx_when_final() {
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

    // Single committer — one mint + one bounty msg, then state removed.
    let committer = Addr::unchecked("only_committer");
    COMMIT_LEDGER
        .save(&mut deps.storage, &committer, &Uint128::new(5_000_000_000))
        .unwrap();

    let dist_state = DistributionState {
        is_distributing: true,
        total_to_distribute: Uint128::new(500_000_000_000),
        total_committed_usd: Uint128::new(5_000_000_000),
        last_processed_key: None,
        distributions_remaining: 1,
        estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
        max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
        last_successful_batch_size: None,
        consecutive_failures: 0,
        started_at: Timestamp::from_seconds(1_600_000_000),
        last_updated: Timestamp::from_seconds(1_600_000_000),
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_600_000_100);

    let caller = message_info(&Addr::unchecked("bounty_hunter"), &[]);
    let res = execute(
        deps.as_mut(),
        env,
        caller,
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    assert_eq!(
        res.messages.len(),
        2,
        "expected 1 mint + 1 bounty msg, got: {:?}",
        res.messages
    );
    let bounty_paid = res
        .attributes
        .iter()
        .find(|a| a.key == "bounty_paid")
        .map(|a| a.value.as_str())
        .unwrap_or("");
    assert_eq!(bounty_paid, "true");

    let complete = res
        .attributes
        .iter()
        .find(|a| a.key == "distribution_complete")
        .map(|a| a.value.as_str())
        .unwrap_or("");
    assert_eq!(complete, "true", "should complete in this single tx");

    assert_eq!(
        DISTRIBUTION_STATE.may_load(&deps.storage).unwrap(),
        None,
        "DISTRIBUTION_STATE must be removed when the ledger is fully drained"
    );
    // Ledger is empty.
    assert_eq!(
        COMMIT_LEDGER
            .keys(&deps.storage, None, None, cosmwasm_std::Order::Ascending)
            .count(),
        0
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
        matches!(err, ContractError::LpFeeOutOfRange { .. }),
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

/// Verify that sync_position_on_transfer preserves fee state across an
/// NFT ownership change. Accrued `unclaimed_fees_*` and the
/// `fee_growth_inside_*_last` checkpoint belong to the position; the new
/// owner inherits them and can collect via the standard `CollectFees`
/// path. Confirms the `fee_reserve == sum-owed` invariant is not broken
/// by the transfer (the older "zero-on-transfer" behavior orphaned
/// `unclaimed_fees_*` that `remove_partial_liquidity` saves into the
/// position without debiting `fee_reserve_*`).
#[test]
fn test_nft_transfer_preserves_fee_state() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Create a position owned by Alice with fee growth snapshots at zero
    create_test_position(&mut deps, 1, "alice", Uint128::new(10_000_000));

    // Simulate fees accruing: advance global fee growth and stamp the
    // position with non-zero unclaimed_fees (mirrors what
    // remove_partial_liquidity does — preserves fees into the position
    // without debiting fee_reserve, so they MUST carry over to the new
    // NFT holder).
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("50").unwrap();
    fee_state.fee_growth_global_1 = Decimal::from_str("75").unwrap();
    fee_state.fee_reserve_0 = Uint128::new(1_000_000_000_000);
    fee_state.fee_reserve_1 = Uint128::new(1_000_000_000_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let mut position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    let prior_growth_0 = Decimal::from_str("10").unwrap();
    let prior_growth_1 = Decimal::from_str("12").unwrap();
    let preserved_unclaimed_0 = Uint128::new(123_456);
    let preserved_unclaimed_1 = Uint128::new(789_012);
    position.fee_growth_inside_0_last = prior_growth_0;
    position.fee_growth_inside_1_last = prior_growth_1;
    position.unclaimed_fees_0 = preserved_unclaimed_0;
    position.unclaimed_fees_1 = preserved_unclaimed_1;
    LIQUIDITY_POSITIONS
        .save(&mut deps.storage, "1", &position)
        .unwrap();

    // Simulate NFT transfer: Bob is now the CW721 owner, but position still
    // has Alice as `position.owner`. Call sync_position_on_transfer as Bob.
    let bob = Addr::unchecked("bob");
    let mut position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.owner, Addr::unchecked("alice"));

    let transferred =
        sync_position_on_transfer(&mut deps.storage, &mut position, "1", &bob, &fee_state).unwrap();

    assert!(transferred, "Should detect ownership transfer");
    assert_eq!(position.owner, bob);

    // Fee state must be PRESERVED (not reset). The position carries its
    // accrued fees with it; Bob inherits them and can claim via CollectFees.
    assert_eq!(
        position.fee_growth_inside_0_last, prior_growth_0,
        "fee_growth_inside_0_last must not be reset on transfer"
    );
    assert_eq!(
        position.fee_growth_inside_1_last, prior_growth_1,
        "fee_growth_inside_1_last must not be reset on transfer"
    );
    assert_eq!(
        position.unclaimed_fees_0, preserved_unclaimed_0,
        "unclaimed_fees_0 must not be zeroed on transfer"
    );
    assert_eq!(
        position.unclaimed_fees_1, preserved_unclaimed_1,
        "unclaimed_fees_1 must not be zeroed on transfer"
    );

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
        matches!(err, ContractError::LpFeeOutOfRange { .. }),
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
        matches!(err, ContractError::LpFeeOutOfRange { .. }),
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
        distributed_so_far: Uint128::zero(),
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE
        .save(&mut deps.storage, &dist_state)
        .unwrap();

    // `execute_emergency_withdraw_initiate` reads the admin-tunable
    // delay at runtime via `query_wasm_smart`, so the synchronous
    // wasm-querier must mock the factory's response. The pool's
    // configured factory_addr from `setup_pool_post_threshold` is
    // `"factory_contract"`.
    deps.querier.update_wasm(move |query| match query {
        cosmwasm_std::WasmQuery::Smart { contract_addr, .. }
            if contract_addr == "factory_contract" =>
        {
            let resp = pool_factory_interfaces::EmergencyWithdrawDelayResponse {
                delay_seconds: 86_400,
            };
            cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Ok(
                cosmwasm_std::to_json_binary(&resp).unwrap(),
            ))
        }
        _ => cosmwasm_std::SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
            error: "unmocked wasm query".to_string(),
            request: cosmwasm_std::Binary::default(),
        }),
    });

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

// ---------------------------------------------------------------------------
// `Commit` must reject multi-denom funds via `must_pay`. Pre-fix,
// attaching `[ubluechip: amount, ibc/...: Y]` would let the bluechip-side
// equality check pass while the IBC side was silently absorbed into the
// pool's bank balance with no withdrawal path. This test exercises the
// fix: commit with extras must be rejected.
// ---------------------------------------------------------------------------
#[test]
fn test_h1_commit_rejects_multi_denom_funds() {
    use crate::msg::CommitFeeInfo;
    use crate::state::CommitLimitInfo;
    use crate::state::{COMMITFEEINFO, COMMIT_LIMIT_INFO, IS_THRESHOLD_HIT};
    use cosmwasm_std::{to_json_binary, Binary, ContractResult, SystemError, SystemResult, WasmQuery};
    use pool_factory_interfaces::ConversionResponse;

    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    COMMITFEEINFO
        .save(
            &mut deps.storage,
            &CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("bluechip_wallet"),
                creator_wallet_address: Addr::unchecked("creator_wallet"),
                commit_fee_bluechip: Decimal::percent(1),
                commit_fee_creator: Decimal::percent(5),
            },
        )
        .unwrap();
    COMMIT_LIMIT_INFO
        .save(
            &mut deps.storage,
            &CommitLimitInfo {
                commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
                max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
                creator_excess_liquidity_lock_days: 14,
                min_commit_usd_pre_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_PRE_THRESHOLD,
                min_commit_usd_post_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_POST_THRESHOLD,
            },
        )
        .unwrap();
    IS_THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();

    // Mock the oracle so usd_value computation passes before the
    // funds-validation gate fires.
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { msg: _, .. } => {
            let response = ConversionResponse {
                amount: Uint128::new(100_000_000), // commit's USD value
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
    let user = Addr::unchecked("committer");
    let amount = Uint128::new(100_000_000);

    // Attaching ubluechip + a stray IBC denom must reject. Pre-fix this
    // call would have silently absorbed the IBC funds into the pool.
    let result = execute(
        deps.as_mut(),
        env,
        message_info(
            &user,
            &[
                Coin::new(amount.u128(), "ubluechip"),
                Coin::new(42_000_000u128, "ibc/27394FB...ATOM"),
            ],
        ),
        ExecuteMsg::Commit {
            asset: TokenInfo {
                info: TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                amount,
            },
            transaction_deadline: None,
            belief_price: None,
            max_spread: None,
        },
    );

    let err = result.expect_err("multi-denom commit must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Invalid commit funds")
            || msg.contains("must_pay")
            || msg.contains("additional denoms")
            || msg.contains("Sent more than one denomination")
            || msg.contains("Multiple denominations")
            || msg.contains("multiple"),
        "expected multi-denom rejection error, got: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// `prepare_deposit` must reject any attached coin whose denom isn't one
// of the pool's configured native sides. Pre-fix, an attached foreign
// denom would be silently kept in the pool's bank balance.
// ---------------------------------------------------------------------------
#[test]
fn test_h2_deposit_rejects_non_pool_native_denom() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let result = execute_deposit_liquidity(
        deps.as_mut(),
        mock_env(),
        message_info(
            &Addr::unchecked("provider"),
            &[
                Coin::new(50_000u128, "ubluechip"),
                Coin::new(42_000u128, "ibc/27394FB...ATOM"),
            ],
        ),
        Addr::unchecked("provider"),
        Uint128::new(50_000),
        Uint128::new(50_000),
        None,
        None,
        None,
    );

    let err = result.expect_err("deposit with non-pool-native denom must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("Unexpected funds") && msg.contains("ibc/27394FB...ATOM"),
        "expected unexpected-funds error mentioning the bad denom, got: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// Verify the gate accepts a clean deposit (only pool-native denoms).
// Defends against an over-broad fix that rejects legitimate deposits too.
// ---------------------------------------------------------------------------
#[test]
fn test_h2_deposit_accepts_clean_native_funds() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // ubluechip alone — the only native side of this Native/CW20 pool.
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

    assert!(
        result.is_ok(),
        "clean ubluechip-only deposit must succeed; got: {:?}",
        result.err()
    );
}

// ---------------------------------------------------------------------------
// Auto-pause-on-low-liquidity / auto-unpause-on-deposit cycle.
// `remove_partial_liquidity` that drains reserves below MIN must arm
// POOL_PAUSED + POOL_PAUSED_AUTO. A subsequent deposit that restores
// reserves above MIN must clear both. Admin pauses must NOT be cleared
// by a deposit.
// ---------------------------------------------------------------------------
#[test]
fn test_m2_helper_arms_auto_pause_when_reserves_below_min() {
    use crate::state::{
        maybe_auto_pause_on_low_liquidity, PoolState, MINIMUM_LIQUIDITY, POOL_PAUSED,
        POOL_PAUSED_AUTO,
    };
    let mut deps = mock_dependencies();

    // Case 1: reserves below MIN, pool unpaused → arms auto-pause.
    let drained = PoolState {
        pool_contract_address: Addr::unchecked("pool"),
        nft_ownership_accepted: true,
        reserve0: MINIMUM_LIQUIDITY - Uint128::new(1),
        reserve1: MINIMUM_LIQUIDITY * Uint128::new(10),
        total_liquidity: Uint128::new(100),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    let armed = maybe_auto_pause_on_low_liquidity(&mut deps.storage, &drained).unwrap();
    assert!(armed, "should arm auto-pause when reserve0 < MIN");
    assert_eq!(POOL_PAUSED.load(&deps.storage).unwrap(), true);
    assert_eq!(POOL_PAUSED_AUTO.load(&deps.storage).unwrap(), true);

    // Case 2: helper is idempotent — calling again on already-paused pool
    // returns false (no override) and leaves both flags as-is.
    let armed_again = maybe_auto_pause_on_low_liquidity(&mut deps.storage, &drained).unwrap();
    assert!(!armed_again, "helper must not re-arm an already-paused pool");
    assert_eq!(POOL_PAUSED.load(&deps.storage).unwrap(), true);
    assert_eq!(POOL_PAUSED_AUTO.load(&deps.storage).unwrap(), true);

    // Case 3: reserves healthy → helper is no-op even on a fresh pool.
    let mut deps2 = mock_dependencies();
    let healthy = PoolState {
        pool_contract_address: Addr::unchecked("pool"),
        nft_ownership_accepted: true,
        reserve0: MINIMUM_LIQUIDITY * Uint128::new(2),
        reserve1: MINIMUM_LIQUIDITY * Uint128::new(2),
        total_liquidity: Uint128::new(1_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    let armed = maybe_auto_pause_on_low_liquidity(&mut deps2.storage, &healthy).unwrap();
    assert!(!armed);
    assert_eq!(POOL_PAUSED.may_load(&deps2.storage).unwrap(), None);
    assert_eq!(POOL_PAUSED_AUTO.may_load(&deps2.storage).unwrap(), None);
}

#[test]
fn test_m2_helper_does_not_override_admin_pause() {
    use crate::state::{
        maybe_auto_pause_on_low_liquidity, PoolState, MINIMUM_LIQUIDITY, POOL_PAUSED,
        POOL_PAUSED_AUTO,
    };
    let mut deps = mock_dependencies();

    // Pool is already admin-paused (POOL_PAUSED true, POOL_PAUSED_AUTO false).
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();
    POOL_PAUSED_AUTO.save(&mut deps.storage, &false).unwrap();

    // Reserves drop below MIN. Helper must NOT flip POOL_PAUSED_AUTO=true,
    // which would otherwise let a deposit auto-clear the admin pause.
    let drained = PoolState {
        pool_contract_address: Addr::unchecked("pool"),
        nft_ownership_accepted: true,
        reserve0: MINIMUM_LIQUIDITY - Uint128::new(1),
        reserve1: MINIMUM_LIQUIDITY * Uint128::new(10),
        total_liquidity: Uint128::new(100),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    let armed = maybe_auto_pause_on_low_liquidity(&mut deps.storage, &drained).unwrap();
    assert!(!armed, "helper must not override an existing admin pause");
    assert_eq!(POOL_PAUSED.load(&deps.storage).unwrap(), true);
    assert_eq!(POOL_PAUSED_AUTO.load(&deps.storage).unwrap(), false);
}

#[test]
fn test_m2_admin_pause_overrides_auto_flag() {
    use crate::admin::execute_pause;
    use crate::state::{POOL_INFO, POOL_PAUSED, POOL_PAUSED_AUTO};
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Pre-arm an auto-pause (simulating a prior remove that drained).
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();
    POOL_PAUSED_AUTO.save(&mut deps.storage, &true).unwrap();

    // Admin then issues an explicit Pause. The auto-flag must clear so
    // a later deposit (which would auto-unpause auto-state) can't
    // override the admin's intent.
    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    let factory_info = message_info(&pool_info.factory_addr, &[]);
    execute_pause(deps.as_mut(), mock_env(), factory_info).unwrap();

    assert_eq!(POOL_PAUSED.load(&deps.storage).unwrap(), true);
    assert_eq!(POOL_PAUSED_AUTO.load(&deps.storage).unwrap(), false);
}

// ---------------------------------------------------------------------------
// Migrate must reject downgrades. With cw2 stored at version "9.9.9"
// (a far-future version that exceeds the current CARGO_PKG_VERSION),
// migrate must error rather than silently overwrite.
// ---------------------------------------------------------------------------
#[test]
fn test_m3_migrate_rejects_downgrade() {
    use crate::contract::migrate;
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Force a "stored" semver that exceeds anything realistic the
    // current binary could be.
    cw2::set_contract_version(&mut deps.storage, "bluechip-contracts-creator-pool", "9.9.9")
        .unwrap();

    let res = migrate(
        deps.as_mut(),
        mock_env(),
        crate::msg::MigrateMsg::UpdateVersion {},
    );
    let err = res.expect_err("downgrade migration must be rejected");
    assert!(
        err.to_string().contains("downgrade"),
        "expected downgrade-rejection error, got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// Per-address rate limit on ContinueDistribution. A second call from
// the same address within the cooldown window must reject.
// ---------------------------------------------------------------------------
#[test]
fn test_m5_continue_distribution_rate_limit_per_address() {
    use crate::msg::ExecuteMsg;
    use crate::state::{
        COMMIT_LEDGER, DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
        DistributionState, DISTRIBUTION_STATE, EXPECTED_FACTORY,
    };
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    EXPECTED_FACTORY
        .save(
            &mut deps.storage,
            &crate::state::ExpectedFactory {
                expected_factory_address: Addr::unchecked("factory_contract"),
            },
        )
        .unwrap();

    // Seed a non-empty ledger so the first call processes work and
    // emits a bounty msg (otherwise the no-op early-return path would
    // not stamp the rate-limit timestamp the same way — actually it
    // does, but seeding makes the test exercise the productive branch).
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("committer1"),
            &Uint128::new(5_000_000_000),
        )
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE.save(&mut deps.storage, &dist_state).unwrap();

    let keeper = Addr::unchecked("keeper1");
    let env = mock_env();

    // First call from keeper1: succeeds.
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&keeper, &[]),
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();

    // Restock ledger so the second call has work to do (otherwise it
    // would return Err("NothingToRecover") before reaching rate-limit
    // gate — we're testing rate-limit, not the empty-ledger reject).
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("committer2"),
            &Uint128::new(5_000_000_000),
        )
        .unwrap();

    // Second call from same keeper, same block: must rate-limit reject.
    let res = execute(
        deps.as_mut(),
        env.clone(),
        message_info(&keeper, &[]),
        ExecuteMsg::ContinueDistribution {},
    );
    let err = res.expect_err("rapid second call must be rate-limited");
    assert!(
        err.to_string().contains("Rate-limited"),
        "expected rate-limit error, got: {}",
        err
    );

    // Different keeper in same block: NOT rate-limited (per-address).
    // Need to also restore DISTRIBUTION_STATE because the first call
    // emptied the original ledger and removed the state. Re-seed both.
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
        distributed_so_far: Uint128::zero(),
    };
    DISTRIBUTION_STATE.save(&mut deps.storage, &dist_state).unwrap();

    let keeper2 = Addr::unchecked("keeper2");
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&keeper2, &[]),
        ExecuteMsg::ContinueDistribution {},
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// `RecoverStuckStates` must reject when pool is drained. The recovery
// branches don't produce fund-flow on a drained pool but they would
// leave misleading DISTRIBUTION_STATE. Failing here keeps post-drain
// state queries honest.
// ---------------------------------------------------------------------------
#[test]
fn test_m6_recover_rejects_on_drained_pool() {
    use crate::msg::ExecuteMsg;
    use crate::state::{
        EmergencyWithdrawalInfo, ExpectedFactory, RecoveryType, EMERGENCY_DRAINED,
        EMERGENCY_WITHDRAWAL, EXPECTED_FACTORY,
    };
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

    // Mark the pool as drained.
    EMERGENCY_DRAINED.save(&mut deps.storage, &true).unwrap();
    EMERGENCY_WITHDRAWAL
        .save(
            &mut deps.storage,
            &EmergencyWithdrawalInfo {
                withdrawn_at: 1_600_000_000,
                recipient: Addr::unchecked("bluechip_wallet"),
                amount0: Uint128::new(1_000_000),
                amount1: Uint128::new(1_000_000),
                total_liquidity_at_withdrawal: Uint128::new(1_000),
            },
        )
        .unwrap();

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::RecoverStuckStates {
            recovery_type: RecoveryType::Both,
        },
    );
    let err = res.expect_err("recovery on drained pool must reject");
    assert!(
        matches!(err, ContractError::EmergencyDrained {}),
        "expected EmergencyDrained, got: {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// `trigger_threshold_payout` emits an `AcceptOwnership` SubMsg so the
// pool locks in its CW721 ownership at threshold-cross time rather than
// lazily on first deposit. Closes the pending-ownership window between
// factory's TransferOwnership and first LP activity.
// ---------------------------------------------------------------------------
#[test]
fn test_m7_threshold_payout_emits_accept_ownership() {
    use crate::generic_helpers::trigger_threshold_payout;
    use crate::msg::CommitFeeInfo;
    use crate::state::{
        CommitLimitInfo, NATIVE_RAISED_FROM_COMMIT, ThresholdPayoutAmounts, POOL_FEE_STATE,
        POOL_INFO, POOL_STATE,
    };
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Seed NATIVE_RAISED_FROM_COMMIT directly. After the audit-fix
    // gross→net refactor this value is interpreted as the post-fee
    // total that has actually entered the pool's bank balance —
    // `trigger_threshold_payout` reads it directly as
    // `pools_bluechip_seed` with no further recovery multiply. This
    // test only asserts NFT-ownership behavior, so the exact seed
    // amount is not load-bearing here; we just need it non-zero so
    // the seed-side branch executes.
    NATIVE_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(25_000_000_000))
        .unwrap();

    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    // Force pre-threshold "ownership pending" state. setup_pool_storage
    // initializes nft_ownership_accepted = true; we want to simulate the
    // realistic post-finalize / pre-threshold-cross window where the
    // factory has dispatched TransferOwnership but the pool hasn't yet
    // accepted it.
    pool_state.nft_ownership_accepted = false;
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 14,
        min_commit_usd_pre_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_PRE_THRESHOLD,
        min_commit_usd_post_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_POST_THRESHOLD,
    };
    let payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000),
        bluechip_reward_amount: Uint128::new(25_000_000_000),
        pool_seed_amount: Uint128::new(350_000_000_000),
        commit_return_amount: Uint128::new(500_000_000_000),
    };
    let fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip_wallet"),
        creator_wallet_address: Addr::unchecked("creator_wallet"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };

    // Sanity: pre-cross, NFT not yet accepted.
    assert!(!pool_state.nft_ownership_accepted);

    let payout_msgs = trigger_threshold_payout(
        &mut deps.storage,
        &pool_info,
        &mut pool_state,
        &mut pool_fee_state,
        &commit_config,
        &payout,
        &fee_info,
        &mock_env(),
    )
    .unwrap();

    // Post-cross: flag flipped, AcceptOwnership message present.
    assert!(
        pool_state.nft_ownership_accepted,
        "trigger_threshold_payout must flip nft_ownership_accepted"
    );
    let accept_msg_present = payout_msgs.other_msgs.iter().any(|m| {
        if let CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) = m {
            if contract_addr != &pool_info.position_nft_address.to_string() {
                return false;
            }
            // Match the AcceptOwnership variant by parsing the body.
            let parsed: Result<
                pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg<()>,
                _,
            > = cosmwasm_std::from_json(msg);
            matches!(
                parsed,
                Ok(pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::UpdateOwnership(
                    pool_factory_interfaces::cw721_msgs::Action::AcceptOwnership
                ))
            )
        } else {
            false
        }
    });
    assert!(
        accept_msg_present,
        "expected AcceptOwnership SubMsg in payout messages, got: {:?}",
        payout_msgs.other_msgs
    );
}

// ---------------------------------------------------------------------------
// H6 audit fix: distribution liveness primitives
// ---------------------------------------------------------------------------
//
// Coverage for the four-part fix (per-mint reply isolation, skip
// primitive, self-recover, claim entry):
//
//   - Per-mint isolation: a single failing recipient lands in
//     `FAILED_MINTS` rather than reverting the whole batch tx; the
//     other rows in the batch still mint, the cursor advances.
//   - SkipDistributionUser: factory-only escape hatch removes a row
//     from COMMIT_LEDGER, credits the user's pro-rata reward into
//     FAILED_MINTS, resets failure counters, re-enables distribution.
//   - SelfRecoverDistribution: permissionless after the 7-day
//     `PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS` window; rejected
//     before the window, accepted after.
//   - ClaimFailedDistribution: committer (or anyone with their key)
//     pulls a previously-failed mint out of FAILED_MINTS, optionally
//     redirected to a fresh wallet. Re-failures recurse cleanly back
//     into FAILED_MINTS via the same reply-isolation harness.
mod distribution_liveness_tests {
    use super::*;
    use crate::contract::reply;
    use crate::state::{
        ExpectedFactory, FAILED_MINTS, PendingMint, PENDING_MINT_REPLIES,
        PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS, REPLY_ID_DISTRIBUTION_MINT_BASE,
        STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS,
    };
    use cosmwasm_std::testing::MockApi;
    use cosmwasm_std::{Binary, Reply, SubMsgResponse, SubMsgResult, Timestamp};

    /// Bech32-valid address from a human-readable label. Production
    /// passes addresses that have always come through `addr_validate`
    /// (info.sender + storage round-trips). The handlers we're testing
    /// call `addr_validate` on String params, so test inputs that
    /// reach them must be bech32-valid — `Addr::unchecked("label")`
    /// is not. `MockApi::default().addr_make(...)` produces a stable
    /// bech32 address derived from the label.
    fn label_addr(label: &str) -> Addr {
        MockApi::default().addr_make(label)
    }

    fn factory_addr() -> Addr {
        // EXPECTED_FACTORY's auth check compares `info.sender` to a
        // stored Addr by equality, so any consistent value works as
        // long as the test installs the same address into both. We
        // keep `Addr::unchecked` here for symmetry with the existing
        // `check_correct_factory` helper in threshold_tests.
        Addr::unchecked("factory_address")
    }

    fn install_factory(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
        EXPECTED_FACTORY
            .save(
                &mut deps.storage,
                &ExpectedFactory {
                    expected_factory_address: factory_addr(),
                },
            )
            .unwrap();
    }

    fn synthetic_reply(id: u64, ok: bool, err_msg: Option<&str>) -> Reply {
        #[allow(deprecated)]
        let ok_response = SubMsgResponse {
            events: vec![],
            data: None,
            msg_responses: vec![],
        };
        Reply {
            id,
            payload: Binary::default(),
            gas_used: 0,
            result: if ok {
                SubMsgResult::Ok(ok_response)
            } else {
                SubMsgResult::Err(err_msg.unwrap_or("CW20 mint rejected by recipient").to_string())
            },
        }
    }

    /// Per-mint isolation: when `process_distribution_batch` dispatches
    /// a per-user mint as a `reply_always` SubMsg and the mint fails,
    /// the contract's reply handler must
    ///   (a) NOT propagate the error,
    ///   (b) clear the PENDING_MINT_REPLIES stash for that id,
    ///   (c) accumulate the failed amount under the user in FAILED_MINTS,
    ///   (d) emit `distribution_mint_isolated_failure` action.
    /// This is the load-bearing liveness invariant — without it, a
    /// single rejecting recipient reverts the batch tx and stalls
    /// distribution for every committer.
    #[test]
    fn reply_distribution_mint_failure_is_isolated_into_failed_mints() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let user = Addr::unchecked("poison_committer");
        let amount = Uint128::new(123_456);
        let reply_id = REPLY_ID_DISTRIBUTION_MINT_BASE + 7;

        PENDING_MINT_REPLIES
            .save(
                &mut deps.storage,
                reply_id,
                &PendingMint {
                    user: user.clone(),
                    amount,
                },
            )
            .unwrap();

        // Reply handler must NOT propagate the error; it's the whole
        // point of the isolation.
        let r = synthetic_reply(reply_id, false, Some("recipient blacklisted"));
        let res = reply(deps.as_mut(), mock_env(), r)
            .expect("reply must Ok on Err result; isolation invariant");

        // Stash cleared.
        assert!(PENDING_MINT_REPLIES
            .may_load(&deps.storage, reply_id)
            .unwrap()
            .is_none());

        // FAILED_MINTS now holds the owed amount under the user.
        let owed = FAILED_MINTS.load(&deps.storage, &user).unwrap();
        assert_eq!(owed, amount);

        // Action attribute identifies the isolated-failure path so
        // off-chain monitoring can flag it.
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "distribution_mint_isolated_failure"));
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "user" && a.value == user.to_string()));
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "reason" && a.value.contains("blacklisted")));
    }

    /// Reply Ok branch: stash cleared, NO FAILED_MINTS write, success
    /// attribute emitted. Pre-existing entries for the user are preserved
    /// (they belong to PRIOR failed mints, not this one).
    #[test]
    fn reply_distribution_mint_success_clears_stash_only() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let user = Addr::unchecked("happy_committer");
        let reply_id = REPLY_ID_DISTRIBUTION_MINT_BASE + 99;
        // Pre-existing FAILED_MINTS entry — must be untouched on success.
        FAILED_MINTS
            .save(&mut deps.storage, &user, &Uint128::new(1_000))
            .unwrap();

        PENDING_MINT_REPLIES
            .save(
                &mut deps.storage,
                reply_id,
                &PendingMint {
                    user: user.clone(),
                    amount: Uint128::new(50),
                },
            )
            .unwrap();

        let r = synthetic_reply(reply_id, true, None);
        let res = reply(deps.as_mut(), mock_env(), r).expect("ok branch");

        assert!(PENDING_MINT_REPLIES
            .may_load(&deps.storage, reply_id)
            .unwrap()
            .is_none());
        // Pre-existing entry preserved.
        assert_eq!(
            FAILED_MINTS.load(&deps.storage, &user).unwrap(),
            Uint128::new(1_000)
        );
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "distribution_mint_succeeded"));
    }

    /// Multiple isolated failures across batches accumulate per-user.
    /// Without the `checked_add` accumulator, a second failure would
    /// overwrite the first. Verify saturation-safe addition.
    #[test]
    fn reply_distribution_mint_failures_accumulate_per_user() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let user = Addr::unchecked("repeat_failure");
        let id1 = REPLY_ID_DISTRIBUTION_MINT_BASE + 100;
        let id2 = REPLY_ID_DISTRIBUTION_MINT_BASE + 101;

        for (id, amt) in [(id1, 250u128), (id2, 750u128)] {
            PENDING_MINT_REPLIES
                .save(
                    &mut deps.storage,
                    id,
                    &PendingMint {
                        user: user.clone(),
                        amount: Uint128::new(amt),
                    },
                )
                .unwrap();
            let r = synthetic_reply(id, false, None);
            reply(deps.as_mut(), mock_env(), r).unwrap();
        }

        assert_eq!(
            FAILED_MINTS.load(&deps.storage, &user).unwrap(),
            Uint128::new(1_000),
            "two failures must accumulate, not overwrite"
        );
    }

    /// Reply id ≥ BASE but with no PENDING_MINT_REPLIES stash falls
    /// through to the canonical "unknown reply id" handler — preserves
    /// the pre-existing regression (`reply_unknown_id_returns_error`
    /// uses 0xDEADBEEF which is in this range).
    #[test]
    fn reply_in_distribution_range_without_stash_is_unknown() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let r = synthetic_reply(REPLY_ID_DISTRIBUTION_MINT_BASE + 12_345, true, None);
        let err = reply(deps.as_mut(), mock_env(), r).unwrap_err();
        assert!(
            err.to_string().contains("unknown reply id"),
            "fallthrough must produce unknown-id error, got: {}",
            err
        );
    }

    // ----- SkipDistributionUser ---------------------------------------

    /// SkipDistributionUser auth: only the configured factory may invoke.
    #[test]
    fn skip_distribution_user_unauthorized_is_rejected() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let info = message_info(&Addr::unchecked("not_factory"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SkipDistributionUser {
                user: "anyone".to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    /// SkipDistributionUser on an absent ledger row returns
    /// `LedgerEntryNotFound` so the operator's input mistake doesn't
    /// silently no-op.
    #[test]
    fn skip_distribution_user_absent_row_returns_not_found() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let info = message_info(&factory_addr(), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SkipDistributionUser {
                user: "cosmos1".to_string() + &"a".repeat(38),
            },
        )
        .unwrap_err();
        // addr_validate may reject the synthetic address shape — both
        // outcomes are acceptable failure modes (Std vs LedgerEntryNotFound).
        match err {
            ContractError::LedgerEntryNotFound { .. } => {}
            ContractError::Std(_) => {}
            other => panic!("expected LedgerEntryNotFound or addr-validate Std, got: {:?}", other),
        }
    }

    /// SkipDistributionUser happy path: removes COMMIT_LEDGER row,
    /// computes pro-rata reward against the live DistributionState,
    /// accumulates into FAILED_MINTS, resets `consecutive_failures`,
    /// re-enables `is_distributing`, decrements `distributions_remaining`.
    #[test]
    fn skip_distribution_user_credits_failed_mints_and_unblocks_state() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        // The handler validates the user string param, so use a
        // bech32-valid address here (see `label_addr` doc).
        let user = label_addr("poison_user");
        // Committer paid $1000 USD; reward share = $1000 / $10000 of 1_000_000_000
        // = 100_000_000 owed.
        let usd_paid = Uint128::new(1_000_000_000);
        COMMIT_LEDGER
            .save(&mut deps.storage, &user, &usd_paid)
            .unwrap();

        let dist = DistributionState {
            is_distributing: false, // simulate post-stall
            total_to_distribute: Uint128::new(1_000_000_000),
            total_committed_usd: Uint128::new(10_000_000_000),
            last_processed_key: None,
            distributions_remaining: 5,
            estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
            max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
            last_successful_batch_size: None,
            consecutive_failures: 5,
            started_at: mock_env().block.time,
            last_updated: mock_env().block.time,
            distributed_so_far: Uint128::zero(),
        };
        DISTRIBUTION_STATE.save(&mut deps.storage, &dist).unwrap();

        let info = message_info(&factory_addr(), &[]);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SkipDistributionUser {
                user: user.to_string(),
            },
        )
        .unwrap();

        // Row removed.
        assert!(COMMIT_LEDGER.may_load(&deps.storage, &user).unwrap().is_none());
        // FAILED_MINTS credited at the pro-rata amount.
        let credited = FAILED_MINTS.load(&deps.storage, &user).unwrap();
        assert_eq!(credited, Uint128::new(100_000_000));

        // DistributionState: counters reset, distribution re-enabled,
        // remaining decremented.
        let dist_after = DISTRIBUTION_STATE.load(&deps.storage).unwrap();
        assert_eq!(dist_after.consecutive_failures, 0);
        assert!(dist_after.is_distributing);
        assert_eq!(dist_after.distributions_remaining, 4);

        // Observability attribute exposes the credited amount.
        assert!(res.attributes.iter().any(|a| a.key
            == "credited_to_failed_mints"
            && a.value == "100000000"));
    }

    // ----- SelfRecoverDistribution ------------------------------------

    /// Below the 7-day window, self-recover must reject so the admin's
    /// shorter (1h) recovery path has uncontested priority.
    #[test]
    fn self_recover_before_window_is_rejected() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let started = mock_env().block.time;
        let dist = DistributionState {
            is_distributing: true,
            total_to_distribute: Uint128::new(1),
            total_committed_usd: Uint128::new(1),
            last_processed_key: None,
            distributions_remaining: 1,
            estimated_gas_per_distribution: DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION,
            max_gas_per_tx: DEFAULT_MAX_GAS_PER_TX,
            last_successful_batch_size: None,
            consecutive_failures: 0,
            started_at: started,
            last_updated: started,
            distributed_so_far: Uint128::zero(),
        };
        DISTRIBUTION_STATE.save(&mut deps.storage, &dist).unwrap();

        // Just under the window.
        let mut env = mock_env();
        env.block.time = started.plus_seconds(PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS - 1);

        let info = message_info(&Addr::unchecked("any_caller"), &[]);
        let err = execute(
            deps.as_mut(),
            env,
            info,
            ExecuteMsg::SelfRecoverDistribution {},
        )
        .unwrap_err();
        match err {
            ContractError::DistributionNotStalledForSelfRecover {
                window,
                admin_window,
                ..
            } => {
                assert_eq!(window, PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS);
                assert_eq!(admin_window, STUCK_DISTRIBUTION_RECOVERY_WINDOW_SECONDS);
            }
            other => panic!("expected DistributionNotStalledForSelfRecover, got: {:?}", other),
        }
    }

    /// After the 7-day window, ANY caller can restart distribution.
    /// Cursor reset to None, counters cleared, `distributed_so_far`
    /// preserved for the dust-settlement invariant.
    #[test]
    fn self_recover_after_window_restarts_with_preserved_distributed_so_far() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let started = mock_env().block.time;
        let preserved = Uint128::new(777_777_777);
        let dist = DistributionState {
            is_distributing: false, // pretend it stalled
            total_to_distribute: Uint128::new(1_000_000_000),
            total_committed_usd: Uint128::new(10_000_000_000),
            last_processed_key: Some(Addr::unchecked("checkpoint")),
            distributions_remaining: 7,
            estimated_gas_per_distribution: 999,
            max_gas_per_tx: 999_999,
            last_successful_batch_size: Some(3),
            consecutive_failures: 5,
            started_at: started,
            last_updated: started,
            distributed_so_far: preserved,
        };
        DISTRIBUTION_STATE.save(&mut deps.storage, &dist).unwrap();

        // Seed two committers so the recovery path lands in the
        // "remaining > 0 → restart" branch.
        for label in ["committer_a", "committer_b"] {
            COMMIT_LEDGER
                .save(
                    &mut deps.storage,
                    &Addr::unchecked(label),
                    &Uint128::new(1_000),
                )
                .unwrap();
        }

        let mut env = mock_env();
        env.block.time = started.plus_seconds(PUBLIC_DISTRIBUTION_RECOVERY_WINDOW_SECONDS + 1);

        let info = message_info(&Addr::unchecked("public_keeper"), &[]);
        let res = execute(
            deps.as_mut(),
            env.clone(),
            info,
            ExecuteMsg::SelfRecoverDistribution {},
        )
        .expect("post-window must succeed");

        let dist_after = DISTRIBUTION_STATE.load(&deps.storage).unwrap();
        assert!(dist_after.is_distributing);
        assert!(dist_after.last_processed_key.is_none(), "cursor must be reset");
        assert_eq!(dist_after.consecutive_failures, 0);
        assert_eq!(dist_after.distributed_so_far, preserved,
            "distributed_so_far must be preserved across restart so dust settlement stays correct");
        assert_eq!(dist_after.distributions_remaining, 2);
        assert_eq!(dist_after.last_updated, env.block.time);

        // Observability: action attribute and stall_elapsed_seconds attr.
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "self_recover_distribution"));
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "remaining_committers" && a.value == "2"));
    }

    /// Self-recover with no DISTRIBUTION_STATE returns the dedicated
    /// error so callers don't rely on a generic "not found" shape.
    #[test]
    fn self_recover_no_distribution_returns_dedicated_error() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let info = message_info(&Addr::unchecked("nobody"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SelfRecoverDistribution {},
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::NoDistributionToSelfRecover));
    }

    // ----- ClaimFailedDistribution ------------------------------------

    /// Claim auth: caller must have a non-zero FAILED_MINTS entry.
    #[test]
    fn claim_failed_distribution_no_entry_rejected() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let info = message_info(&Addr::unchecked("not_a_committer"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimFailedDistribution { recipient: None },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::NoFailedMintEntry { .. }));
    }

    /// Happy path: caller has a FAILED_MINTS entry; handler dispatches
    /// a SubMsg::reply_always for the mint, removes the FAILED_MINTS
    /// entry up front, and stashes a PENDING_MINT entry for the new
    /// reply id. On reply success the stash clears. On reply failure
    /// the amount is re-credited under the original committer for
    /// another retry.
    #[test]
    fn claim_failed_distribution_dispatches_isolated_submsg() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let user = Addr::unchecked("recovered_committer");
        let owed = Uint128::new(444_444);
        FAILED_MINTS.save(&mut deps.storage, &user, &owed).unwrap();

        // Caller specifies an alternate recipient (e.g., a fresh wallet
        // because their original is the reason the mint failed).
        // Bech32-valid because the handler addr_validates the param.
        let alternate = label_addr("fresh_wallet");
        let info = message_info(&user, &[]);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimFailedDistribution {
                recipient: Some(alternate.to_string()),
            },
        )
        .expect("claim must succeed");

        // FAILED_MINTS entry removed up front.
        assert!(FAILED_MINTS.may_load(&deps.storage, &user).unwrap().is_none());

        // Exactly one SubMsg dispatched, in the reply_always range.
        assert_eq!(res.messages.len(), 1);
        let sub = &res.messages[0];
        assert!(sub.id >= REPLY_ID_DISTRIBUTION_MINT_BASE);

        // PENDING_MINT_REPLIES recorded the user as the canonical
        // accounting key (NOT the alternate recipient) so a re-failure
        // re-credits the original committer.
        let pending = PENDING_MINT_REPLIES
            .load(&deps.storage, sub.id)
            .unwrap();
        assert_eq!(pending.user, user);
        assert_eq!(pending.amount, owed);

        // The mint message itself targets the alternate recipient.
        if let CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) = &sub.msg {
            let parsed: cw20::Cw20ExecuteMsg = from_json(msg).unwrap();
            match parsed {
                cw20::Cw20ExecuteMsg::Mint { recipient, amount } => {
                    assert_eq!(recipient, alternate.to_string());
                    assert_eq!(amount, owed);
                }
                other => panic!("expected Mint, got: {:?}", other),
            }
        } else {
            panic!("expected Wasm Execute SubMsg, got: {:?}", sub.msg);
        }
    }

    /// Re-failure recursion: the alternate recipient is ALSO blocked.
    /// The reply handler must re-credit the ORIGINAL committer's
    /// FAILED_MINTS entry so they can try yet another recipient. This
    /// is the loop-closure invariant — without it, the second failure
    /// would orphan the funds.
    #[test]
    fn claim_failed_distribution_re_failure_re_credits_original_committer() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let user = Addr::unchecked("loop_committer");
        let owed = Uint128::new(99_999);
        FAILED_MINTS.save(&mut deps.storage, &user, &owed).unwrap();

        // Bech32 needed for addr_validate.
        let alternate = label_addr("alternate_also_blocked");
        let info = message_info(&user, &[]);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimFailedDistribution {
                recipient: Some(alternate.to_string()),
            },
        )
        .unwrap();
        let reply_id = res.messages[0].id;

        // Dispatched but not yet replied — FAILED_MINTS is empty.
        assert!(FAILED_MINTS.may_load(&deps.storage, &user).unwrap().is_none());

        // Simulate the alternate ALSO rejecting the mint.
        let r = synthetic_reply(reply_id, false, Some("alternate also blacklisted"));
        reply(deps.as_mut(), mock_env(), r).unwrap();

        // FAILED_MINTS re-credited under the ORIGINAL committer (`user`),
        // NOT under the alternate. The user can now try yet another
        // recipient on a fresh ClaimFailedDistribution call.
        assert_eq!(
            FAILED_MINTS.load(&deps.storage, &user).unwrap(),
            owed,
        );
        // Alternate has no FAILED_MINTS entry — they were a recipient
        // address only, never the canonical accounting key.
        assert!(FAILED_MINTS
            .may_load(&deps.storage, &alternate)
            .unwrap()
            .is_none());
    }

    /// Default recipient: when `recipient: None`, the mint is wired to
    /// the caller (committer) themselves. Useful for the "the recipient
    /// is fine again, just retry" case.
    #[test]
    fn claim_failed_distribution_defaults_recipient_to_caller() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);

        let user = Addr::unchecked("self_claim_committer");
        FAILED_MINTS
            .save(&mut deps.storage, &user, &Uint128::new(1))
            .unwrap();

        let info = message_info(&user, &[]);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimFailedDistribution { recipient: None },
        )
        .unwrap();
        let sub = &res.messages[0];
        if let CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) = &sub.msg {
            if let cw20::Cw20ExecuteMsg::Mint { recipient, .. } = from_json(msg).unwrap() {
                assert_eq!(recipient, user.to_string(),
                    "default recipient must be info.sender");
            } else {
                panic!("not a Mint");
            }
        } else {
            panic!("not a Wasm Execute");
        }
    }

    /// Drained pool: every liveness primitive must reject so the
    /// post-drain invariant ("the pool no longer pays out from this
    /// contract") is uniform across all entry points.
    #[test]
    fn liveness_primitives_reject_on_drained_pool() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        install_factory(&mut deps);
        EMERGENCY_DRAINED.save(&mut deps.storage, &true).unwrap();

        // Skip
        let info = message_info(&factory_addr(), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SkipDistributionUser {
                user: "anyone".to_string(),
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("Drained")
            || format!("{:?}", err).contains("drained"));

        // Self-recover
        let info = message_info(&Addr::unchecked("anyone"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::SelfRecoverDistribution {},
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("Drained")
            || format!("{:?}", err).contains("drained"));

        // Claim
        let info = message_info(&Addr::unchecked("anyone"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimFailedDistribution { recipient: None },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("Drained")
            || format!("{:?}", err).contains("drained"));
    }

    /// Suppress unused-import lint in this test module — the timestamp
    /// import is referenced through `setup_pool_storage`'s internals.
    #[allow(dead_code)]
    fn _ts_marker(_t: Timestamp) {}
}

// ---------------------------------------------------------------------------
// H1 audit fix: creator-pool deposit balance verification
// ---------------------------------------------------------------------------
//
// The creator-pool dispatcher used to call the no-verify deposit /
// add-to-position variants on the assumption that the pool's CW20 was
// always a vanilla cw20-base freshly minted by the factory — true
// today, but a single careless future `update_pool_token_address` or
// factory upgrade permitting third-party CW20s would let the pool
// credit user-claimed amounts to reserves while the actual on-chain
// CW20 balance lagged (fee-on-transfer, rebasing, malicious receiver).
//
// The fix flips the dispatcher to the `_with_verify` variants and
// wires `DEPOSIT_VERIFY_REPLY_ID` into the contract's `reply()`
// dispatcher so the post-balance delta is checked before the
// transaction commits.
//
// Tests in this module confirm that production deposits routed through
// `ExecuteMsg::DepositLiquidity` / `ExecuteMsg::AddToPosition` now:
//   - emit a final SubMsg tagged `reply_on_success` with the verify
//     reply id (the anchor the reply handler hooks onto), and
//   - persist `DEPOSIT_VERIFY_CTX` carrying the pre-balance snapshot
//     and credited delta for the reply to consume.
mod deposit_verify_tests {
    use super::*;
    use crate::contract::reply;
    use crate::testing::liquidity_tests::setup_pool_post_threshold;
    use cosmwasm_std::{
        to_json_binary, Binary, Coin, ContractResult, Reply, ReplyOn, SubMsgResponse,
        SubMsgResult, SystemResult, WasmQuery,
    };
    use cw20::BalanceResponse as Cw20BalanceResponse;
    use pool_core::state::{
        DepositVerifyContext, DEPOSIT_VERIFY_CTX, DEPOSIT_VERIFY_REPLY_ID,
    };

    /// Install a CW20 balance querier that returns a fixed balance for
    /// any cw20 query and an `OwnerOf` querier for the NFT contract so
    /// the deposit / add-to-position handlers can look up position
    /// ownership during dispatch. The CW20 balance is parametric so
    /// callers can simulate matching deltas (success path) or
    /// mismatched deltas (fee-on-transfer / rebasing rejection path)
    /// against a known pre-balance.
    fn install_balance_querier(
        deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
        cw20_balance: Uint128,
        nft_owner: &str,
    ) {
        let owner = nft_owner.to_string();
        deps.querier.update_wasm(move |query| match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    if let Ok(pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf {
                        ..
                    }) = cosmwasm_std::from_json(msg)
                    {
                        let resp =
                            pool_factory_interfaces::cw721_msgs::OwnerOfResponse {
                                owner: owner.clone(),
                                approvals: vec![],
                            };
                        return SystemResult::Ok(ContractResult::Ok(
                            to_json_binary(&resp).unwrap(),
                        ));
                    }
                }
                if let Ok(cw20::Cw20QueryMsg::Balance { .. }) =
                    cosmwasm_std::from_json(msg)
                {
                    let resp = Cw20BalanceResponse {
                        balance: cw20_balance,
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&resp).unwrap(),
                    ));
                }
                SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                    error: format!("unexpected wasm query to {}", contract_addr),
                    request: msg.clone(),
                })
            }
            _ => SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
                kind: "non-Smart wasm query".to_string(),
            }),
        });
    }

    /// `ExecuteMsg::DepositLiquidity` (creator-pool dispatch path)
    /// must emit a final SubMsg tagged `reply_on_success` carrying
    /// `DEPOSIT_VERIFY_REPLY_ID`. This is the dispatch-anchor — without
    /// it the reply handler never fires and the verify invariant is
    /// vacuous.
    #[test]
    fn deposit_liquidity_through_dispatcher_emits_verify_reply_anchor() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        // Pre-balance is read by `prepare_deposit` — install ANY value;
        // the test only asserts the SubMsg shape, not the reply outcome.
        install_balance_querier(&mut deps, Uint128::zero(), "liquidity_provider");

        let user = Addr::unchecked("liquidity_provider");
        let bluechip_amount = Uint128::new(1_000_000_000);
        let token_amount = Uint128::new(14_893_617_021);
        let info = message_info(
            &user,
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: bluechip_amount,
            }],
        );

        let res = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::DepositLiquidity {
                amount0: bluechip_amount,
                amount1: token_amount,
                min_amount0: None,
                min_amount1: None,
                transaction_deadline: None,
            },
        )
        .expect("dispatch must succeed");

        // The LAST outgoing SubMsg must carry the verify reply id and
        // be wired as `reply_on_success`. Production code uses the
        // last-message anchor so the reply runs strictly after every
        // bank/cw20 transfer the deposit emitted.
        let last = res
            .messages
            .last()
            .expect("response must contain at least one SubMsg");
        assert_eq!(
            last.id, DEPOSIT_VERIFY_REPLY_ID,
            "last SubMsg must carry DEPOSIT_VERIFY_REPLY_ID — got id {} \
             (creator-pool dispatcher must route DepositLiquidity through \
             execute_deposit_liquidity_with_verify, not the unverified variant)",
            last.id
        );
        assert!(
            matches!(last.reply_on, ReplyOn::Success),
            "DEPOSIT_VERIFY_REPLY_ID must be wired reply_on_success — got {:?}; \
             reply_always or reply_on_error would let a deposit subroutine \
             error short-circuit the verification entirely.",
            last.reply_on
        );

        // DEPOSIT_VERIFY_CTX is the transient handoff to the reply
        // handler; it carries the pre-balance snapshot and credited
        // delta. Must be present after a verify-path deposit.
        let ctx = DEPOSIT_VERIFY_CTX
            .may_load(&deps.storage)
            .unwrap()
            .expect("DEPOSIT_VERIFY_CTX must be saved by the verify-path deposit");
        assert!(
            ctx.cw20_side1_addr.is_some(),
            "creator-pool's pair has the CW20 on side 1; verify ctx must record it"
        );
        // The credited delta on side 1 should equal the user-supplied
        // CW20 amount (subject to the deposit prep math; for a first
        // deposit shape the credit is the user-supplied amount minus
        // any internal adjustment — we assert non-zero rather than an
        // exact value because the deposit math is exercised separately
        // in liquidity_tests).
        assert!(
            !ctx.expected_delta1.is_zero(),
            "expected non-zero credited delta on the CW20 side"
        );
    }

    /// `ExecuteMsg::AddToPosition` must also route through the verify
    /// path. Without this assertion, a future refactor that flips just
    /// one of the two dispatcher arms back to the unverified variant
    /// would silently regress the H1 invariant on add-to-position.
    #[test]
    fn add_to_position_through_dispatcher_emits_verify_reply_anchor() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        install_balance_querier(&mut deps, Uint128::zero(), "liquidity_provider");

        let user = Addr::unchecked("liquidity_provider");
        // First seed a position. We go through the dispatcher so the
        // verify path produces a position id "2" (id 1 belongs to the
        // initial setup). The setup_pool_post_threshold helper does
        // not pre-create LIQUIDITY_POSITIONS; the deposit handler
        // mints id 2 because NEXT_POSITION_ID starts at 1 and is
        // bumped post-deposit.
        let mut env = mock_env();
        let info = message_info(
            &user,
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(1_000_000_000),
            }],
        );
        execute(
            deps.as_mut(),
            env.clone(),
            info,
            ExecuteMsg::DepositLiquidity {
                amount0: Uint128::new(1_000_000_000),
                amount1: Uint128::new(14_893_617_021),
                min_amount0: None,
                min_amount1: None,
                transaction_deadline: None,
            },
        )
        .expect("first deposit must succeed");

        // DEPOSIT_VERIFY_CTX is one-shot — the next deposit-class call
        // saves a fresh one. Remove the prior context so the next save
        // doesn't see a stale snapshot. (Production: the reply handler
        // removes it before the next handler call ever fires.)
        DEPOSIT_VERIFY_CTX.remove(&mut deps.storage);

        // Advance past `min_commit_interval` (60s in setup_pool_storage)
        // so the second deposit-class call from the same user isn't
        // rate-limited as a "too frequent commit" — that gate is
        // orthogonal to the verify-path assertion this test exists for.
        env.block.time = env.block.time.plus_seconds(120);

        let info = message_info(
            &user,
            &[Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(500_000_000),
            }],
        );
        let res = execute(
            deps.as_mut(),
            env,
            info,
            ExecuteMsg::AddToPosition {
                position_id: "2".to_string(),
                amount0: Uint128::new(500_000_000),
                amount1: Uint128::new(7_446_808_510),
                min_amount0: None,
                min_amount1: None,
                transaction_deadline: None,
            },
        )
        .expect("AddToPosition dispatch must succeed");

        let last = res
            .messages
            .last()
            .expect("response must carry SubMsgs");
        assert_eq!(
            last.id, DEPOSIT_VERIFY_REPLY_ID,
            "AddToPosition must also route through the verify path; got id {}",
            last.id
        );
        assert!(matches!(last.reply_on, ReplyOn::Success));

        let ctx = DEPOSIT_VERIFY_CTX
            .may_load(&deps.storage)
            .unwrap()
            .expect("AddToPosition with verify must save DEPOSIT_VERIFY_CTX");
        assert!(ctx.cw20_side1_addr.is_some());
        assert!(!ctx.expected_delta1.is_zero());
    }

    /// Reply id `DEPOSIT_VERIFY_REPLY_ID` must route to
    /// `handle_deposit_verify_reply` in creator-pool's `reply()`
    /// dispatcher. Without this dispatch arm, the post-balance check
    /// never runs even though the deposit handler dispatches the
    /// SubMsg. Test: synthesize a Reply with the verify id, install
    /// a balance querier whose post value matches the expected delta
    /// for the saved context, and assert success path attributes
    /// land (proves we hit the verify handler, not the catch-all
    /// unknown-id error).
    #[test]
    fn reply_dispatcher_routes_deposit_verify_id_to_handler() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);

        // Synthetic CTX: pre-balance 1_000, expected delta 250 → post
        // must be 1_250 for the verify handler's strict equality
        // check to pass.
        let pool_addr = Addr::unchecked("pool_contract");
        let cw20 = Addr::unchecked("token_contract");
        DEPOSIT_VERIFY_CTX
            .save(
                &mut deps.storage,
                &DepositVerifyContext {
                    pool_addr: pool_addr.clone(),
                    cw20_side0_addr: None,
                    cw20_side1_addr: Some(cw20.clone()),
                    pre_balance0: Uint128::zero(),
                    pre_balance1: Uint128::new(1_000),
                    expected_delta0: Uint128::zero(),
                    expected_delta1: Uint128::new(250),
                },
            )
            .unwrap();
        install_balance_querier(&mut deps, Uint128::new(1_250), "anyone");

        #[allow(deprecated)]
        let ok_response = SubMsgResponse {
            events: vec![],
            data: None,
            msg_responses: vec![],
        };
        let r = Reply {
            id: DEPOSIT_VERIFY_REPLY_ID,
            payload: Binary::default(),
            gas_used: 0,
            result: SubMsgResult::Ok(ok_response),
        };
        let res = reply(deps.as_mut(), mock_env(), r)
            .expect("verify reply must succeed when post == pre + expected");

        assert!(
            res.attributes
                .iter()
                .any(|a| a.key == "action" && a.value == "deposit_balance_verified"),
            "must hit handle_deposit_verify_reply (success path emits this attribute); \
             got attrs: {:?}",
            res.attributes
        );
        assert!(
            DEPOSIT_VERIFY_CTX
                .may_load(&deps.storage)
                .unwrap()
                .is_none(),
            "verify handler must clear DEPOSIT_VERIFY_CTX on success"
        );
    }

    /// Reply dispatcher rejects on a fee-on-transfer / rebasing-down
    /// shortfall: post-balance < pre + expected, so delta != expected
    /// and the verify handler returns Err. Creator-pool's `reply`
    /// returns `StdResult<Response>`, so the typed `ContractError`
    /// from the verify handler is mapped into `StdError::generic_err`
    /// — assert the error string contains the canonical "balance
    /// delta does not match" phrase that off-chain monitoring keys on.
    #[test]
    fn reply_dispatcher_rejects_balance_shortfall() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);

        let pool_addr = Addr::unchecked("pool_contract");
        let cw20 = Addr::unchecked("token_contract");
        DEPOSIT_VERIFY_CTX
            .save(
                &mut deps.storage,
                &DepositVerifyContext {
                    pool_addr,
                    cw20_side0_addr: None,
                    cw20_side1_addr: Some(cw20.clone()),
                    pre_balance0: Uint128::zero(),
                    pre_balance1: Uint128::new(1_000),
                    expected_delta0: Uint128::zero(),
                    expected_delta1: Uint128::new(250),
                },
            )
            .unwrap();
        // Post = 1_240 — short by 10 base units (simulates a 4% FoT
        // tax on the credited 250).
        install_balance_querier(&mut deps, Uint128::new(1_240), "anyone");

        #[allow(deprecated)]
        let ok_response = SubMsgResponse {
            events: vec![],
            data: None,
            msg_responses: vec![],
        };
        let r = Reply {
            id: DEPOSIT_VERIFY_REPLY_ID,
            payload: Binary::default(),
            gas_used: 0,
            result: SubMsgResult::Ok(ok_response),
        };
        let err = reply(deps.as_mut(), mock_env(), r)
            .expect_err("shortfall must propagate as Err to revert the parent tx");
        let msg = err.to_string();
        assert!(
            msg.contains("balance delta") && msg.contains("does not match"),
            "shortfall error must carry the canonical phrase off-chain monitoring \
             keys on; got: {}",
            msg
        );
    }

    /// Reply dispatcher also rejects an inflation overage (post >
    /// pre + expected). Without this, an attacker controlling a CW20
    /// that mints to the pool mid-deposit could grow the pool's
    /// reserve without paying for it (the strict-equality check
    /// blocks both shortfall and overage).
    #[test]
    fn reply_dispatcher_rejects_inflation_overage() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);

        let pool_addr = Addr::unchecked("pool_contract");
        let cw20 = Addr::unchecked("token_contract");
        DEPOSIT_VERIFY_CTX
            .save(
                &mut deps.storage,
                &DepositVerifyContext {
                    pool_addr,
                    cw20_side0_addr: None,
                    cw20_side1_addr: Some(cw20.clone()),
                    pre_balance0: Uint128::zero(),
                    pre_balance1: Uint128::new(1_000),
                    expected_delta0: Uint128::zero(),
                    expected_delta1: Uint128::new(250),
                },
            )
            .unwrap();
        // Post = 1_500 — overage of 250 (simulates a CW20 that
        // double-mints during transfer or a rebase-up event landing
        // mid-deposit).
        install_balance_querier(&mut deps, Uint128::new(1_500), "anyone");

        #[allow(deprecated)]
        let ok_response = SubMsgResponse {
            events: vec![],
            data: None,
            msg_responses: vec![],
        };
        let r = Reply {
            id: DEPOSIT_VERIFY_REPLY_ID,
            payload: Binary::default(),
            gas_used: 0,
            result: SubMsgResult::Ok(ok_response),
        };
        let err = reply(deps.as_mut(), mock_env(), r)
            .expect_err("overage must propagate as Err");
        assert!(
            err.to_string().contains("does not match"),
            "overage rejection must surface; got: {}",
            err
        );
    }
}

// ---------------------------------------------------------------------------
// H-NFT-1 audit fix: empty-position persistence on full removal
// ---------------------------------------------------------------------------
//
// Pre-fix, `RemoveAllLiquidity` on a non-first-depositor position
// (`locked_liquidity == 0`) deleted the LIQUIDITY_POSITIONS storage row
// while leaving the user's CW721 NFT in place — no BurnNft was ever
// dispatched. The NFT became a "tombstone": tradeable on secondary
// markets but functionally inert, since every pool-side handler
// (AddToPosition, CollectFees, RemoveLiquidity) loaded LIQUIDITY_POSITIONS
// and errored with "not found".
//
// Option A fix: keep the row alive at `liquidity == 0`. The NFT
// remains rehydrate-able — a future `AddToPosition` against the same
// token id grows the position from zero. Mirrors Uniswap V3's
// empty-position model.
//
// Tests in this module confirm:
//   - The row persists with `liquidity == 0` after full removal.
//   - OWNER_POSITIONS index entry persists so frontends can still list
//     the empty NFT under "your positions."
//   - AddToPosition successfully rehydrates the empty position.
//   - CollectFees on an empty position is a clean no-op (no double-debit).
//   - First-depositor positions still drop to `locked_liquidity` rather
//     than zero (the locked-floor invariant is preserved).
mod empty_position_persistence_tests {
    use super::*;
    use crate::testing::liquidity_tests::{create_test_position, setup_pool_post_threshold};
    use cosmwasm_std::{
        to_json_binary, ContractResult, SystemResult, WasmQuery,
    };
    use pool_core::liquidity::{
        execute_collect_fees, execute_remove_all_liquidity,
    };
    use pool_core::state::{LIQUIDITY_POSITIONS, OWNER_POSITIONS, POOL_STATE};

    /// Install a CW721 OwnerOf querier that returns the given owner for
    /// any token id. Also answers CW20 Balance queries with a fixed
    /// balance so the verify-path deposit can run its pre/post snapshot
    /// without erroring out on an unstubbed balance read.
    fn install_owner_querier(
        deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
        owner: &str,
    ) {
        let owner_str = owner.to_string();
        deps.querier.update_wasm(move |query| match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    if let Ok(pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf {
                        ..
                    }) = cosmwasm_std::from_json(msg)
                    {
                        let resp =
                            pool_factory_interfaces::cw721_msgs::OwnerOfResponse {
                                owner: owner_str.clone(),
                                approvals: vec![],
                            };
                        return SystemResult::Ok(ContractResult::Ok(
                            to_json_binary(&resp).unwrap(),
                        ));
                    }
                }
                if let Ok(cw20::Cw20QueryMsg::Balance { .. }) =
                    cosmwasm_std::from_json(msg)
                {
                    // Fixed pool CW20 balance for the verify-path
                    // pre/post snapshot. The actual value doesn't
                    // matter for these tests — only that the query
                    // resolves so the deposit handler reaches the
                    // SubMsg-emit step.
                    let resp = cw20::BalanceResponse {
                        balance: Uint128::zero(),
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&resp).unwrap(),
                    ));
                }
                SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                    error: format!("unexpected wasm query to {}", contract_addr),
                    request: msg.clone(),
                })
            }
            _ => SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
                kind: "non-Smart wasm query".to_string(),
            }),
        });
    }

    /// Full-removal path keeps LIQUIDITY_POSITIONS row alive at zero
    /// liquidity for non-first-depositor positions. Pre-fix the row
    /// was deleted; this assertion is the regression fence.
    #[test]
    fn full_removal_keeps_position_row_alive_at_zero() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        create_test_position(&mut deps, 5, "lp_holder", Uint128::new(10_000_000));
        // create_test_position only seeds LIQUIDITY_POSITIONS; production
        // deposits seed OWNER_POSITIONS too. Seed it here so the post-
        // removal assertion has something to inspect.
        OWNER_POSITIONS
            .save(
                &mut deps.storage,
                (&Addr::unchecked("lp_holder"), "5"),
                &true,
            )
            .unwrap();
        install_owner_querier(&mut deps, "lp_holder");

        let info = message_info(&Addr::unchecked("lp_holder"), &[]);
        execute_remove_all_liquidity(
            deps.as_mut(),
            mock_env(),
            info,
            "5".to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("full removal must succeed");

        // The row must still be present — no tombstone NFT pointing at
        // a deleted storage entry.
        let pos = LIQUIDITY_POSITIONS
            .load(&deps.storage, "5")
            .expect("LIQUIDITY_POSITIONS row must persist after full removal");
        assert_eq!(
            pos.liquidity,
            Uint128::zero(),
            "non-first-depositor full exit must drop liquidity to zero"
        );
        assert_eq!(
            pos.locked_liquidity,
            Uint128::zero(),
            "no locked floor on non-first-depositor positions"
        );
        assert_eq!(pos.unclaimed_fees_0, Uint128::zero());
        assert_eq!(pos.unclaimed_fees_1, Uint128::zero());

        // OWNER_POSITIONS index entry must also persist so the user's
        // empty NFT still shows up in their position list.
        assert!(
            OWNER_POSITIONS
                .may_load(&deps.storage, (&Addr::unchecked("lp_holder"), "5"))
                .unwrap()
                .is_some(),
            "OWNER_POSITIONS entry must persist for the empty NFT"
        );
    }

    /// First-depositor full removal still drops to the locked floor
    /// (MINIMUM_LIQUIDITY), not to zero. This is the
    /// `locked_liquidity > 0` branch that was already correct pre-fix
    /// — guard that the new code didn't accidentally collapse both
    /// branches and break the first-depositor's perpetual fee right.
    #[test]
    fn first_depositor_full_removal_drops_to_locked_floor_not_zero() {
        use pool_core::state::MINIMUM_LIQUIDITY;
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        // Manually seed a first-depositor-style position: locked_liquidity
        // > 0 indicates the perma-locked slice was honored on initial
        // deposit.
        let position = pool_core::state::Position {
            liquidity: Uint128::new(50_000_000),
            owner: Addr::unchecked("first_depositor"),
            fee_growth_inside_0_last: cosmwasm_std::Decimal::zero(),
            fee_growth_inside_1_last: cosmwasm_std::Decimal::zero(),
            created_at: 1_600_000_000,
            last_fee_collection: 1_600_000_000,
            fee_size_multiplier: cosmwasm_std::Decimal::one(),
            unclaimed_fees_0: Uint128::zero(),
            unclaimed_fees_1: Uint128::zero(),
            locked_liquidity: MINIMUM_LIQUIDITY,
        };
        LIQUIDITY_POSITIONS
            .save(&mut deps.storage, "1", &position)
            .unwrap();
        OWNER_POSITIONS
            .save(
                &mut deps.storage,
                (&Addr::unchecked("first_depositor"), "1"),
                &true,
            )
            .unwrap();
        install_owner_querier(&mut deps, "first_depositor");

        let info = message_info(&Addr::unchecked("first_depositor"), &[]);
        execute_remove_all_liquidity(
            deps.as_mut(),
            mock_env(),
            info,
            "1".to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("first-depositor full removal must succeed");

        let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
        assert_eq!(
            pos.liquidity, MINIMUM_LIQUIDITY,
            "first-depositor exit must keep liquidity at locked floor"
        );
        assert_eq!(
            pos.locked_liquidity, MINIMUM_LIQUIDITY,
            "locked_liquidity itself must not change"
        );
    }

    /// AddToPosition rehydrates an emptied position. Pre-fix this would
    /// fail with `LIQUIDITY_POSITIONS::not_found` because the storage row
    /// had been deleted on full removal; post-fix the row is alive at
    /// zero liquidity and AddToPosition grows it from there.
    #[test]
    fn add_to_position_rehydrates_emptied_position() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        create_test_position(&mut deps, 7, "rehydrator", Uint128::new(5_000_000));
        install_owner_querier(&mut deps, "rehydrator");

        // Step 1: drain the position to zero.
        let user = Addr::unchecked("rehydrator");
        execute_remove_all_liquidity(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            "7".to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("removal must succeed");
        // Confirm the row is at zero (not deleted).
        let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "7").unwrap();
        assert_eq!(pos.liquidity, Uint128::zero());

        // Step 2: re-deposit into the same position via AddToPosition.
        // We advance time past `min_commit_interval` to avoid the
        // per-user commit gate (orthogonal to the rehydration we're
        // testing).
        let mut env = mock_env();
        env.block.time = env.block.time.plus_seconds(120);
        let info = message_info(
            &user,
            &[cosmwasm_std::Coin {
                denom: "ubluechip".to_string(),
                amount: Uint128::new(1_000_000_000),
            }],
        );
        let res = execute(
            deps.as_mut(),
            env,
            info,
            ExecuteMsg::AddToPosition {
                position_id: "7".to_string(),
                amount0: Uint128::new(1_000_000_000),
                amount1: Uint128::new(14_893_617_021),
                min_amount0: None,
                min_amount1: None,
                transaction_deadline: None,
            },
        )
        .expect("re-deposit into emptied position must succeed");

        // Pool state advanced — total_liquidity grew, position now has
        // non-zero liquidity. The exact value depends on prep math; the
        // load-bearing assertion is "the rehydration is possible at all,"
        // which it now is.
        assert!(!res.messages.is_empty());
        let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "7").unwrap();
        assert!(
            !pos.liquidity.is_zero(),
            "AddToPosition into a zero-liquidity row must produce non-zero liquidity, \
             got: {}",
            pos.liquidity
        );

        // Pool's total_liquidity must have grown to reflect the
        // re-deposit (sanity check that the position's growth was
        // booked into the pool, not just the position).
        let pool_state = POOL_STATE.load(&deps.storage).unwrap();
        // setup_pool_post_threshold seeds total_liquidity = 91_104_335_791
        // so any growth above that confirms the rehydration was credited.
        assert!(
            pool_state.total_liquidity > Uint128::new(91_104_335_791),
            "total_liquidity must have grown from re-deposit"
        );
    }

    /// CollectFees on an empty position is a clean no-op: zero fees
    /// transferred, no underflow on `fee_reserve`, position row stays
    /// at zero. This is the second canary against a future change that
    /// might assume `position.liquidity > 0` when calling collect.
    #[test]
    fn collect_fees_on_empty_position_is_noop() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        create_test_position(&mut deps, 9, "no_fees_yet", Uint128::new(5_000_000));
        install_owner_querier(&mut deps, "no_fees_yet");

        // Drain to zero first.
        let user = Addr::unchecked("no_fees_yet");
        execute_remove_all_liquidity(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            "9".to_string(),
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Now CollectFees on the empty position: should succeed with
        // zero fees transferred. The fee-transfer messages should be
        // absent or zero-valued; what we assert here is just "no error."
        let res = execute_collect_fees(
            deps.as_mut(),
            mock_env(),
            message_info(&user, &[]),
            "9".to_string(),
        )
        .expect("collect_fees on empty position must succeed");

        // Position row still alive, still at zero liquidity.
        let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "9").unwrap();
        assert_eq!(pos.liquidity, Uint128::zero());
        // unclaimed_fees stays zero — no fees accrued, no fees swept.
        assert_eq!(pos.unclaimed_fees_0, Uint128::zero());
        assert_eq!(pos.unclaimed_fees_1, Uint128::zero());
        // Response carries the action attribute regardless of fee
        // amount — confirms we hit the success path.
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "collect_fees"));
    }
}

// ---------------------------------------------------------------------------
// H-NFT-4 audit fix: per-position emergency-claim escrow
// ---------------------------------------------------------------------------
//
// Pre-fix, Phase-2 emergency-drain swept ALL pool funds (including
// LP-owned reserves and pending fees) to `bluechip_wallet_address`.
// Active LPs could exit during the 24h window, but set-and-forget
// LPs lost everything. Post-fix, LP funds escrow in
// `EMERGENCY_DRAIN_SNAPSHOT` for 1 year, claimable per-position via
// `ClaimEmergencyShare`. After dormancy, the unclaimed residual
// sweeps to the bluechip wallet via `SweepUnclaimedEmergencyShares`.
mod emergency_claim_escrow_tests {
    use super::*;
    use crate::testing::liquidity_tests::{create_test_position, setup_pool_post_threshold};
    use cosmwasm_std::{
        to_json_binary, ContractResult, SystemResult, WasmQuery,
    };
    use pool_core::state::{
        EmergencyDrainSnapshot, EMERGENCY_CLAIM_DORMANCY_SECONDS, EMERGENCY_DRAINED,
        EMERGENCY_DRAIN_SNAPSHOT, LIQUIDITY_POSITIONS, POOL_FEE_STATE, POOL_INFO, POOL_STATE,
    };

    /// Install a CW721 OwnerOf querier returning the given owner for
    /// any token id. Local helper; mirrors the pattern used in
    /// other H-NFT tests.
    fn install_owner_querier(
        deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
        owner: &str,
    ) {
        let owner_str = owner.to_string();
        deps.querier.update_wasm(move |query| match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    let resp = pool_factory_interfaces::cw721_msgs::OwnerOfResponse {
                        owner: owner_str.clone(),
                        approvals: vec![],
                    };
                    return SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&resp).unwrap(),
                    ));
                }
                SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                    error: format!("unexpected wasm query to {}", contract_addr),
                    request: msg.clone(),
                })
            }
            _ => SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
                kind: "non-Smart wasm query".to_string(),
            }),
        });
    }

    /// Seed the pool's storage to a "post Phase-2 drain" shape
    /// directly: install EMERGENCY_DRAINED + EMERGENCY_DRAIN_SNAPSHOT
    /// with the requested totals + dormancy. Bypasses the actual
    /// Phase-1/Phase-2 flow so each test focuses on the claim/sweep
    /// behavior in isolation.
    fn seed_post_drain_state(
        deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
        env: &cosmwasm_std::Env,
        reserve0: Uint128,
        reserve1: Uint128,
        fee_reserve_0: Uint128,
        fee_reserve_1: Uint128,
        total_liquidity: Uint128,
    ) {
        EMERGENCY_DRAINED.save(&mut deps.storage, &true).unwrap();
        let drained_at = env.block.time;
        let dormancy_expires_at =
            drained_at.plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS);
        EMERGENCY_DRAIN_SNAPSHOT
            .save(
                &mut deps.storage,
                &EmergencyDrainSnapshot {
                    drained_at,
                    dormancy_expires_at,
                    reserve0_at_drain: reserve0,
                    reserve1_at_drain: reserve1,
                    fee_reserve_0_at_drain: fee_reserve_0,
                    fee_reserve_1_at_drain: fee_reserve_1,
                    total_liquidity_at_drain: total_liquidity,
                    total_claimed_0: Uint128::zero(),
                    total_claimed_1: Uint128::zero(),
                    residual_swept: false,
                },
            )
            .unwrap();
        // Phase-2's accounting wipe is reproduced here so other paths
        // that load POOL_STATE see the canonical post-drain shape.
        let mut ps = POOL_STATE.load(&deps.storage).unwrap();
        ps.reserve0 = Uint128::zero();
        ps.reserve1 = Uint128::zero();
        ps.total_liquidity = Uint128::zero();
        POOL_STATE.save(&mut deps.storage, &ps).unwrap();
        let mut fs = POOL_FEE_STATE.load(&deps.storage).unwrap();
        fs.fee_reserve_0 = Uint128::zero();
        fs.fee_reserve_1 = Uint128::zero();
        POOL_FEE_STATE.save(&mut deps.storage, &fs).unwrap();
    }

    /// ClaimEmergencyShare without a drained pool returns the dedicated
    /// `NoEmergencyDrainSnapshot` error so callers don't mistake "pool
    /// is fine" for "your claim was processed."
    #[test]
    fn claim_emergency_share_without_drain_rejects() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        create_test_position(&mut deps, 1, "alice", Uint128::new(10_000_000));
        install_owner_querier(&mut deps, "alice");

        let info = message_info(&Addr::unchecked("alice"), &[]);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info,
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::NoEmergencyDrainSnapshot));
    }

    /// Single-position pro-rata math: solo LP should claim 100% of
    /// `(reserve_*_at_drain + fee_reserve_*_at_drain)`. Validates the
    /// denominator-numerator wiring at the simplest case.
    #[test]
    fn claim_emergency_share_solo_lp_gets_full_pot() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        let alice_liq = Uint128::new(10_000_000);
        create_test_position(&mut deps, 1, "alice", alice_liq);
        install_owner_querier(&mut deps, "alice");
        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(800_000_000),     // reserve0
            Uint128::new(1_200_000_000),   // reserve1
            Uint128::new(50_000_000),      // fee_reserve_0
            Uint128::new(75_000_000),      // fee_reserve_1
            alice_liq,                      // sole LP
        );

        let info = message_info(&Addr::unchecked("alice"), &[]);
        let res = execute(
            deps.as_mut(),
            env,
            info,
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .expect("solo claim must succeed");

        // Solo LP gets the full LP-side pot.
        let expected_total_0 = Uint128::new(800_000_000 + 50_000_000);
        let expected_total_1 = Uint128::new(1_200_000_000 + 75_000_000);

        let total_0_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "total_0")
            .unwrap();
        let total_1_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "total_1")
            .unwrap();
        assert_eq!(total_0_attr.value, expected_total_0.to_string());
        assert_eq!(total_1_attr.value, expected_total_1.to_string());

        // Position is marked spent — second claim must reject.
        let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
        assert_eq!(pos.liquidity, Uint128::zero());

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("alice"), &[]),
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ContractError::NoClaimableEmergencyShare { .. }
        ));

        // Snapshot's running tally bumped by the claim.
        let snap = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
        assert_eq!(snap.total_claimed_0, expected_total_0);
        assert_eq!(snap.total_claimed_1, expected_total_1);
    }

    /// Two-LP pro-rata: 30/70 split. Each gets 30% / 70% of the
    /// LP-side pot. Verifies the multiply_ratio math matches the
    /// expected proportional split on uneven liquidity weights.
    #[test]
    fn claim_emergency_share_two_lps_split_pro_rata() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        // Alice 30%, Bob 70%.
        let alice_liq = Uint128::new(3_000);
        let bob_liq = Uint128::new(7_000);
        let total_liq = alice_liq.checked_add(bob_liq).unwrap();
        create_test_position(&mut deps, 1, "alice", alice_liq);
        create_test_position(&mut deps, 2, "bob", bob_liq);

        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(1_000_000),
            Uint128::new(2_000_000),
            Uint128::zero(), // simplify: no pending fees
            Uint128::zero(),
            total_liq,
        );

        // Alice claims first.
        install_owner_querier(&mut deps, "alice");
        let res_a = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&Addr::unchecked("alice"), &[]),
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .unwrap();
        let alice_total_0: u128 = res_a
            .attributes
            .iter()
            .find(|a| a.key == "total_0")
            .unwrap()
            .value
            .parse()
            .unwrap();
        let alice_total_1: u128 = res_a
            .attributes
            .iter()
            .find(|a| a.key == "total_1")
            .unwrap()
            .value
            .parse()
            .unwrap();
        // 30% of 1_000_000 = 300_000; 30% of 2_000_000 = 600_000.
        assert_eq!(alice_total_0, 300_000);
        assert_eq!(alice_total_1, 600_000);

        // Bob claims second.
        install_owner_querier(&mut deps, "bob");
        let res_b = execute(
            deps.as_mut(),
            env,
            message_info(&Addr::unchecked("bob"), &[]),
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "2".to_string(),
            },
        )
        .unwrap();
        let bob_total_0: u128 = res_b
            .attributes
            .iter()
            .find(|a| a.key == "total_0")
            .unwrap()
            .value
            .parse()
            .unwrap();
        let bob_total_1: u128 = res_b
            .attributes
            .iter()
            .find(|a| a.key == "total_1")
            .unwrap()
            .value
            .parse()
            .unwrap();
        // 70% shares.
        assert_eq!(bob_total_0, 700_000);
        assert_eq!(bob_total_1, 1_400_000);

        // Snapshot's running tally equals exactly the LP-side pot —
        // no dust on round splits.
        let snap = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
        assert_eq!(snap.total_claimed_0, Uint128::new(1_000_000));
        assert_eq!(snap.total_claimed_1, Uint128::new(2_000_000));
    }

    /// CW721 ownership gate: a non-owner cannot claim against another
    /// position. Mirrors the auth model used by every other LP-state
    /// mutation; sync_position_on_transfer would update `position.owner`
    /// if the NFT changed hands, but verify_position_ownership runs
    /// first and rejects mismatched senders before sync runs.
    #[test]
    fn claim_emergency_share_rejects_non_owner() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        create_test_position(&mut deps, 1, "alice", Uint128::new(1_000));
        // CW721 says "alice" owns position 1.
        install_owner_querier(&mut deps, "alice");
        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(100),
            Uint128::new(200),
            Uint128::zero(),
            Uint128::zero(),
            Uint128::new(1_000),
        );

        // Bob attempts to claim Alice's position.
        let err = execute(
            deps.as_mut(),
            env,
            message_info(&Addr::unchecked("bob"), &[]),
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    /// Sweep-unclaimed: pre-dormancy attempt must reject with the
    /// dedicated dormancy-not-elapsed error so the admin's intent
    /// can't bypass the year-long claim window.
    #[test]
    fn sweep_before_dormancy_rejects() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        // Seed factory_addr in POOL_INFO so the auth check finds it.
        let pi = POOL_INFO.load(&deps.storage).unwrap();
        let factory = pi.factory_addr.clone();
        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(1_000_000),
            Uint128::new(1_000_000),
            Uint128::zero(),
            Uint128::zero(),
            Uint128::new(1_000),
        );

        // Just shy of the dormancy.
        let mut early_env = env.clone();
        early_env.block.time = early_env
            .block
            .time
            .plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS - 1);

        let err = execute(
            deps.as_mut(),
            early_env,
            message_info(&factory, &[]),
            ExecuteMsg::SweepUnclaimedEmergencyShares {},
        )
        .unwrap_err();
        match err {
            ContractError::EmergencyClaimDormancyNotElapsed { .. } => {}
            other => panic!("expected EmergencyClaimDormancyNotElapsed, got: {:?}", other),
        }
    }

    /// Sweep-unclaimed by non-factory must reject with Unauthorized,
    /// even after the dormancy elapsed.
    #[test]
    fn sweep_unauthorized_rejects() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(1),
            Uint128::new(1),
            Uint128::zero(),
            Uint128::zero(),
            Uint128::new(1),
        );

        let mut late_env = env;
        late_env.block.time = late_env
            .block
            .time
            .plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);

        let err = execute(
            deps.as_mut(),
            late_env,
            message_info(&Addr::unchecked("not_factory"), &[]),
            ExecuteMsg::SweepUnclaimedEmergencyShares {},
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    /// Sweep-unclaimed happy path: after the 1-year dormancy, factory
    /// admin can sweep the residual to the bluechip wallet. Residual
    /// = drainable - already_claimed. Tests with a partial pre-claim
    /// to confirm the residual subtracts correctly.
    #[test]
    fn sweep_after_dormancy_transfers_residual_to_bluechip_wallet() {
        let mut deps = mock_dependencies();
        setup_pool_post_threshold(&mut deps);
        let pi = POOL_INFO.load(&deps.storage).unwrap();
        let factory = pi.factory_addr.clone();

        let alice_liq = Uint128::new(3_000);
        let bob_liq = Uint128::new(7_000);
        let total_liq = alice_liq.checked_add(bob_liq).unwrap();
        create_test_position(&mut deps, 1, "alice", alice_liq);
        create_test_position(&mut deps, 2, "bob", bob_liq);

        let env = mock_env();
        seed_post_drain_state(
            &mut deps,
            &env,
            Uint128::new(1_000_000),
            Uint128::new(2_000_000),
            Uint128::zero(),
            Uint128::zero(),
            total_liq,
        );

        // Alice claims 30%; Bob never claims. Bob's 70% becomes the
        // residual after the dormancy elapses.
        install_owner_querier(&mut deps, "alice");
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&Addr::unchecked("alice"), &[]),
            ExecuteMsg::ClaimEmergencyShare {
                position_id: "1".to_string(),
            },
        )
        .unwrap();

        let mut late_env = env;
        late_env.block.time = late_env
            .block
            .time
            .plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);

        let res = execute(
            deps.as_mut(),
            late_env,
            message_info(&factory, &[]),
            ExecuteMsg::SweepUnclaimedEmergencyShares {},
        )
        .expect("sweep after dormancy must succeed");

        // Residual = total - claimed = (1_000_000 - 300_000) = 700_000
        // on side 0; (2_000_000 - 600_000) = 1_400_000 on side 1.
        let res_0_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "residual_0")
            .unwrap();
        let res_1_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "residual_1")
            .unwrap();
        assert_eq!(res_0_attr.value, "700000");
        assert_eq!(res_1_attr.value, "1400000");

        // residual_swept latch flipped — second sweep is a no-op
        // error to prevent double-sweeping a since-bumped tally.
        let snap = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
        assert!(snap.residual_swept);

        let err = execute(
            deps.as_mut(),
            mock_env().tap(|e| {
                e.block.time = e
                    .block
                    .time
                    .plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS + 100)
            }),
            message_info(&factory, &[]),
            ExecuteMsg::SweepUnclaimedEmergencyShares {},
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::NoUnclaimedEmergencyResidual));
    }

    /// `tap` helper to mutate an Env in a single expression. Local
    /// because we don't want to pull a tap crate just for tests.
    trait TapEnv {
        fn tap<F: FnOnce(&mut Self)>(self, f: F) -> Self;
    }
    impl TapEnv for cosmwasm_std::Env {
        fn tap<F: FnOnce(&mut Self)>(mut self, f: F) -> Self {
            f(&mut self);
            self
        }
    }
}
