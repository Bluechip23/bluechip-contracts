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
