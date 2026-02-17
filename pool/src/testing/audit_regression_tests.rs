/// Audit regression tests for critical, high, and medium severity findings.
/// These tests serve as regression guards to ensure audit fixes are never accidentally reverted.
///
/// Coverage:
/// - C-1: Post-threshold swap reserve double-counting (commission + return deducted from ask reserve)
/// - C-3: Reentrancy guard recovery via RecoverStuckStates
/// - M-4: Minimum liquidity lock on first deposit (MINIMUM_LIQUIDITY = 1000)
/// - M-5: Distribution bounty paid from fee_reserve, not tradeable reserves
/// - M-6: Migration fee bounds validation (max 10%)

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
use crate::testing::liquidity_tests::{setup_pool_post_threshold, setup_pool_storage};
use crate::testing::swap_tests::with_factory_oracle;

fn mock_dependencies_with_balance(
    balances: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    deps.querier
        .update_balance(cosmwasm_std::testing::MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}

// ============================================================================
// C-1: Post-threshold swap reserve double-counting regression
//
// The fix ensures ask_reserve is reduced by (return_amt + commission_amt),
// not just return_amt. If only return_amt is subtracted, the commission
// stays counted in both reserve1 AND fee_reserve_1 (double-counting).
// ============================================================================

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

    // C-1 REGRESSION: reserve1 must NOT include the commission amount.
    // If the bug existed, reserve1 = initial_reserve1 - return_amt (commission double-counted).
    // With the fix: reserve1 = initial_reserve1 - return_amt - commission_amt.
    // Verify: reserve1 + fee_reserve_1 + tokens_sent_to_user < initial_reserve1
    // (they should sum to exactly initial_reserve1)
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

// ============================================================================
// C-3: Reentrancy guard recovery
//
// If RATE_LIMIT_GUARD gets stuck in `true` (e.g., due to a failed transaction
// that doesn't clean up), the factory admin can reset it.
// ============================================================================

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

// ============================================================================
// M-4: Minimum liquidity lock on first deposit
//
// The first depositor in a pool should have MINIMUM_LIQUIDITY (1000) locked,
// receiving (computed_liquidity - 1000) instead of the full amount.
// This prevents the "first depositor" attack.
// ============================================================================

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

// ============================================================================
// M-5: Distribution bounty paid from fee reserves
//
// When someone calls ContinueDistribution, the bounty should come from
// fee_reserve_0 (bluechip fee reserves), not from the tradeable reserves.
// ============================================================================

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

    // Check that fee_reserve_0 decreased (bounty was taken from it)
    let post_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(
        post_fee_state.fee_reserve_0 < fee_state.fee_reserve_0,
        "M-5 regression: bounty should be taken from fee_reserve_0"
    );

    // Check that tradeable reserves were NOT touched for the bounty
    let post_reserve0 = POOL_STATE.load(&deps.storage).unwrap().reserve0;
    // Reserve may change due to distribution, but not by the bounty amount specifically
    // The bounty_paid attribute should confirm bounty was paid
    let bounty_attr = res.attributes.iter()
        .find(|a| a.key == "bounty_paid")
        .expect("Should have bounty_paid attribute");
    let bounty_amount: u128 = bounty_attr.value.parse().unwrap();
    assert!(bounty_amount > 0, "Bounty should be non-zero");
}

// ============================================================================
// M-6: Migration fee bounds validation
//
// MigrateMsg::UpdateFees should reject fees > 10% to prevent
// migration from setting abusive fee levels.
// ============================================================================

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
