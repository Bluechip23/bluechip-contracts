use crate::asset::{TokenInfo, TokenType};
use crate::contract::{execute, instantiate};
use crate::msg::CommitFeeInfo;
use crate::msg::{ExecuteMsg, PoolConfigUpdate, PoolInstantiateMsg};
use crate::state::{PoolSpecs, ORACLE_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE};
use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
use cosmwasm_std::{Addr, Coin, Decimal, Uint128};

fn mock_instantiate_msg() -> PoolInstantiateMsg {
    PoolInstantiateMsg {
        pool_id: 1,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "ublue".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("creator_token"),
            },
        ],
        cw20_token_contract_id: 123,
        used_factory_addr: Addr::unchecked("factory_addr"),
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip_wallet"),
            creator_wallet_address: Addr::unchecked("creator_wallet"),
            commit_fee_bluechip: Decimal::percent(1),
            commit_fee_creator: Decimal::percent(1),
        },
        commit_threshold_limit_usd: Uint128::new(1000),
        commit_amount_for_threshold: Uint128::new(1000),
        position_nft_address: Addr::unchecked("nft_addr"),
        token_address: Addr::unchecked("token_addr"),
        max_bluechip_lock_per_pool: Uint128::new(10000),
        creator_excess_liquidity_lock_days: 7,
        is_standard_pool: Some(true),
    }
}

#[test]
fn test_pause_unpause() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = mock_info("factory_addr", &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();

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
            info: TokenType::Bluechip {
                denom: "ublue".to_string(),
            },
            amount: Uint128::new(100),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    // Note: SimpleSwap logic checks pause FIRST.
    // However, SimpleSwap might fail for other reasons (like no liquidity), so we need to ensure it fails with "PoolPausedLowLiquidity" specifically or generic Paused if we separated them.
    // In contract.rs: execute_simple_swap checks is_paused and returns PoolPausedLowLiquidity.

    // We need to set up some liquidity first to pass other checks if pause check wasn't first?
    // Actually pause check is usually very early.
    // But let's act as a user
    let user_info = mock_info("user", &[Coin::new(100, "ublue")]);
    let res = execute(deps.as_mut(), mock_env(), user_info.clone(), swap_msg);
    // Since we didn't add liquidity, it might fail on empty reserves if pause check was after.
    // But if pause check is first, it should be PoolPausedLowLiquidity.
    // Note: The error enum name is PoolPausedLowLiquidity but it is used for manual pause too now.
    match res {
        Err(e) => {
            // In string form it might look like "Pool is paused or has low liquidity"
            // Or we can check the debug output
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
    let info = mock_info("factory_addr", &[]);
    let base_env = mock_env();
    instantiate(deps.as_mut(), base_env.clone(), info.clone(), msg).unwrap();

    // Inject some liquidity mock manually for testing.
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(1000); // 1000 ublue
    pool_state.reserve1 = Uint128::new(2000); // 2000 creator token
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    // --- Phase 1: initiate the emergency withdrawal ---
    // H-3 FIX: EmergencyWithdraw is now two-phase. The first call pauses the
    // pool and sets a 24-hour timelock; no funds are moved yet.
    let initiate_res = execute(
        deps.as_mut(),
        base_env.clone(),
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    let action = initiate_res.attributes.iter().find(|a| a.key == "action").unwrap();
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
    assert!(format!("{:?}", early_err).contains("timelock not yet elapsed"));

    // --- Phase 2: execute after the 24-hour delay ---
    let mut env_after = base_env.clone();
    env_after.block.time = env_after.block.time.plus_seconds(86_401); // 24 h + 1 s

    let exec_res = execute(
        deps.as_mut(),
        env_after,
        info.clone(),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    let action = exec_res.attributes.iter().find(|a| a.key == "action").unwrap();
    assert_eq!(action.value, "emergency_withdraw");

    let amount0 = exec_res.attributes.iter().find(|a| a.key == "amount0").unwrap();
    assert_eq!(amount0.value, "1000");

    let amount1 = exec_res.attributes.iter().find(|a| a.key == "amount1").unwrap();
    assert_eq!(amount1.value, "2000");

    // Reserves zeroed.
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::zero());
    assert_eq!(pool_state.reserve1, Uint128::zero());

    // Two transfer messages (native bluechip + CW20 creator token).
    assert_eq!(exec_res.messages.len(), 2);
}

#[test]
fn test_cancel_emergency_withdraw() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = mock_info("factory_addr", &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();

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

    let action = cancel_res.attributes.iter().find(|a| a.key == "action").unwrap();
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
    let info = mock_info("factory_addr", &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();

    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::percent(5)),    // was 0.3%
        min_commit_interval: Some(60),        // was something else
        usd_payment_tolerance_bps: Some(200), // 2%
        oracle_address: Some("new_oracle".to_string()),
        // other fields None
        commit_fee_info: None,
        commit_limit_usd: None,
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: None,
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
    };

    let exec_msg = ExecuteMsg::UpdateConfigFromFactory { update };
    execute(deps.as_mut(), mock_env(), info.clone(), exec_msg).unwrap();

    // Verify updates
    let specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(specs.lp_fee, Decimal::percent(5));
    assert_eq!(specs.min_commit_interval, 60);
    assert_eq!(specs.usd_payment_tolerance_bps, 200);

    let oracle_info = ORACLE_INFO.load(&deps.storage).unwrap();
    assert_eq!(oracle_info.oracle_addr, Addr::unchecked("new_oracle"));
}

#[test]
fn test_unauthorized_admin_actions() {
    let mut deps = mock_dependencies();
    let msg = mock_instantiate_msg();
    let info = mock_info("factory_addr", &[]);
    instantiate(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();

    let hacker = mock_info("hacker", &[]);

    // Pause
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
        commit_fee_info: None,
        commit_limit_usd: None,
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: None,
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
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
