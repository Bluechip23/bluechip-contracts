use crate::asset::{TokenInfo, TokenType};
use crate::contract::{execute, instantiate};
use crate::msg::CommitFeeInfo;
use crate::msg::{ExecuteMsg, PoolConfigUpdate, PoolInstantiateMsg};
use crate::state::{ThresholdPayoutAmounts, ORACLE_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE};
use cosmwasm_std::testing::{message_info, mock_dependencies, mock_env, MockApi};
use cosmwasm_std::{to_json_binary, Addr, Coin, Decimal, Uint128};

fn mock_instantiate_msg() -> PoolInstantiateMsg {
    // Both the CreatorToken entry and `token_address` must be bech32-valid
    // (cosmwasm's mock API rejects raw strings via addr_validate) AND must
    // equal each other (post-audit pair-shape invariant). Using the same
    // MockApi-derived address for both satisfies both.
    let api = MockApi::default();
    let token_addr = api.addr_make("creator_token");
    // Pre-4d this test used is_standard_pool: Some(true) to skip
    // threshold_payout validation. Now that flag is gone; supply the
    // fixed threshold_payout shape `validate_pool_threshold_payments`
    // accepts (creator=325B, bluechip=25B, pool=350B, commit=500B,
    // total=1.2T).
    let threshold_payout = to_json_binary(&ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000),
        bluechip_reward_amount: Uint128::new(25_000_000_000),
        pool_seed_amount: Uint128::new(350_000_000_000),
        commit_return_amount: Uint128::new(500_000_000_000),
    })
    .unwrap();
    PoolInstantiateMsg {
        pool_id: 1,
        pool_token_info: [
            TokenType::Native {
                denom: "ublue".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: token_addr.clone(),
            },
        ],
        cw20_token_contract_id: 123,
        used_factory_addr: Addr::unchecked("factory_addr"),
        threshold_payout: Some(threshold_payout),
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip_wallet"),
            creator_wallet_address: Addr::unchecked("creator_wallet"),
            commit_fee_bluechip: Decimal::percent(1),
            commit_fee_creator: Decimal::percent(1),
        },
        commit_threshold_limit_usd: Uint128::new(1000),
        commit_amount_for_threshold: Uint128::new(1000),
        position_nft_address: Addr::unchecked("nft_addr"),
        token_address: token_addr,
        max_bluechip_lock_per_pool: Uint128::new(10000),
        creator_excess_liquidity_lock_days: 7,
    }
}

#[test]
fn test_pause_unpause() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = message_info(&Addr::unchecked("factory_addr"), &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();
    // Simulate post-threshold state: admin tests exercise swap/emergency_
    // withdraw flows that used to rely on is_standard_pool: Some(true) to
    // force IS_THRESHOLD_HIT=true at instantiate. With that flag gone in 4d,
    // creator-pool starts pre-threshold; tests that want post-threshold
    // behavior seed it explicitly.
    crate::state::IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    // Verify initial state (not paused)
    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap_or(false);
    assert!(!is_paused);

    // Call Pause from factory
    let pause_msg = ExecuteMsg::Pause {};
    execute(deps.as_mut(), mock_env(), info.clone(), pause_msg).unwrap();

    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(is_paused);

    // Try to swap (should fail)
    let swap_msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Native {
                denom: "ublue".to_string(),
            },
            amount: Uint128::new(100),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    let user_info = message_info(&Addr::unchecked("user"), &[Coin::new(100u128, "ublue")]);
    let res = execute(deps.as_mut(), mock_env(), user_info.clone(), swap_msg);

    match res {
        Err(e) => {
            let debug_err = format!("{:?}", e);
            assert!(debug_err.contains("PoolPausedLowLiquidity"));
        }
        Ok(_) => panic!("Swap should have failed"),
    }

    // Call Unpause from factory
    let unpause_msg = ExecuteMsg::Unpause {};
    execute(deps.as_mut(), mock_env(), info.clone(), unpause_msg).unwrap();

    let is_paused = POOL_PAUSED.load(&deps.storage).unwrap();
    assert!(!is_paused);
}

#[test]
fn test_emergency_withdraw() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = message_info(&Addr::unchecked("factory_addr"), &[]);
    let base_env = mock_env();
    instantiate(deps.as_mut(), base_env.clone(), info.clone(), msg).unwrap();
    // Simulate post-threshold state (see mock_instantiate_msg's comment).
    crate::state::IS_THRESHOLD_HIT
        .save(&mut deps.storage, &true)
        .unwrap();

    // Inject some liquidity mock manually for testing.
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(1000); // 1000 ublue
    pool_state.reserve1 = Uint128::new(2000); // 2000 creator token
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    let initiate_res = execute(
        deps.as_mut(),
        base_env.clone(),
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    let action = initiate_res
        .attributes
        .iter()
        .find(|a| a.key == "action")
        .unwrap();
    assert_eq!(action.value, "emergency_withdraw_initiated");

    // Pool should be paused immediately on initiation.
    assert!(POOL_PAUSED.load(&deps.storage).unwrap());

    // No funds moved yet — reserves are still intact.
    let ps = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(ps.reserve0, Uint128::new(1000));
    assert_eq!(ps.reserve1, Uint128::new(2000));

    // Calling again before timelock should fail.
    let early_err = execute(
        deps.as_mut(),
        base_env.clone(),
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(format!("{}", early_err).contains("timelock not yet elapsed"));

    let mut env_after = base_env.clone();
    env_after.block.time = env_after.block.time.plus_seconds(86_401); // 24 h + 1 s

    let exec_res = execute(
        deps.as_mut(),
        env_after,
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    let action = exec_res
        .attributes
        .iter()
        .find(|a| a.key == "action")
        .unwrap();
    assert_eq!(action.value, "emergency_withdraw");

    let amount0 = exec_res
        .attributes
        .iter()
        .find(|a| a.key == "amount0")
        .unwrap();
    assert_eq!(amount0.value, "1000");

    let amount1 = exec_res
        .attributes
        .iter()
        .find(|a| a.key == "amount1")
        .unwrap();
    assert_eq!(amount1.value, "2000");

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::zero());
    assert_eq!(pool_state.reserve1, Uint128::zero());

    assert_eq!(exec_res.messages.len(), 2);
}

#[test]
fn test_cancel_emergency_withdraw() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = message_info(&Addr::unchecked("factory_addr"), &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();
    // Simulate post-threshold state: admin tests exercise swap/emergency_
    // withdraw flows that used to rely on is_standard_pool: Some(true) to
    // force IS_THRESHOLD_HIT=true at instantiate. With that flag gone in 4d,
    // creator-pool starts pre-threshold; tests that want post-threshold
    // behavior seed it explicitly.
    crate::state::IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    // Inject reserves.
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(500);
    pool_state.reserve1 = Uint128::new(1000);
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    // Phase 1: initiate
    execute(
        deps.as_mut(),
        mock_env(),
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();
    assert!(POOL_PAUSED.load(&deps.storage).unwrap());

    // Cancel — pool should be unpaused and no drain occurs.
    let cancel_res = execute(
        deps.as_mut(),
        mock_env(),
        info.clone(),
        ExecuteMsg::CancelEmergencyWithdraw {},
    )
    .unwrap();

    let action = cancel_res
        .attributes
        .iter()
        .find(|a| a.key == "action")
        .unwrap();
    assert_eq!(action.value, "emergency_withdraw_cancelled");

    // Pool unpaused, reserves intact.
    assert!(!POOL_PAUSED.load(&deps.storage).unwrap());
    let ps = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(ps.reserve0, Uint128::new(500));
    assert_eq!(ps.reserve1, Uint128::new(1000));
}

#[test]
fn test_update_config_all() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let api = MockApi::default();
    let new_oracle = api.addr_make("new_oracle");
    let info = message_info(&Addr::unchecked("factory_addr"), &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();
    // Simulate post-threshold state: admin tests exercise swap/emergency_
    // withdraw flows that used to rely on is_standard_pool: Some(true) to
    // force IS_THRESHOLD_HIT=true at instantiate. With that flag gone in 4d,
    // creator-pool starts pre-threshold; tests that want post-threshold
    // behavior seed it explicitly.
    crate::state::IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::percent(5)),    // was 0.3%
        min_commit_interval: Some(60),        // was something else
        usd_payment_tolerance_bps: Some(200), // 2%
        oracle_address: Some(new_oracle.to_string()),
    };

    let exec_msg = ExecuteMsg::UpdateConfigFromFactory { update };
    execute(deps.as_mut(), mock_env(), info.clone(), exec_msg).unwrap();

    // Verify updates
    let specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(specs.lp_fee, Decimal::percent(5));
    assert_eq!(specs.min_commit_interval, 60);
    assert_eq!(specs.usd_payment_tolerance_bps, 200);

    let oracle_info = ORACLE_INFO.load(&deps.storage).unwrap();
    assert_eq!(oracle_info.oracle_addr, new_oracle);
}

#[test]
fn test_unauthorized_admin_actions() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = message_info(&Addr::unchecked("factory_addr"), &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();
    // Simulate post-threshold state: admin tests exercise swap/emergency_
    // withdraw flows that used to rely on is_standard_pool: Some(true) to
    // force IS_THRESHOLD_HIT=true at instantiate. With that flag gone in 4d,
    // creator-pool starts pre-threshold; tests that want post-threshold
    // behavior seed it explicitly.
    crate::state::IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();

    let hacker = message_info(&Addr::unchecked("hacker"), &[]);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        hacker.clone(),
        ExecuteMsg::Pause {},
    )
    .unwrap_err();
    assert!(format!("{:?}", err).contains("Unauthorized"));

    // Emergency Withdraw
    let err = execute(
        deps.as_mut(),
        mock_env(),
        hacker.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(format!("{:?}", err).contains("Unauthorized"));

    // Update Config
    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::percent(100)),
        min_commit_interval: None,
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };
    let err = execute(
        deps.as_mut(),
        mock_env(),
        hacker.clone(),
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap_err();
    assert!(format!("{:?}", err).contains("Unauthorized"));
}
