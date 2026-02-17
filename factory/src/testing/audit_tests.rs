/// Factory audit and missing-coverage tests.
///
/// Coverage:
/// - NotifyThresholdCrossed: valid notification, double-call prevention, unauthorized caller
/// - CancelConfigUpdate: cancel pending timelock
/// - Config timelock enforcement: cannot execute before 48h
/// - UpdatePoolConfig: send config update to specific pool
use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
};
use cosmwasm_std::{
    to_json_binary, Addr, Coin, Decimal, OwnedDeps, Timestamp, Uint128,
};

use crate::error::ContractError;
use crate::execute::{execute, instantiate};
use crate::mock_querier::WasmMockQuerier;
use crate::msg::ExecuteMsg;
use crate::pool_struct::{CommitFeeInfo, PoolConfigUpdate};
use crate::state::{
    FactoryInstantiate, PENDING_CONFIG, POOL_REGISTRY, POOL_THRESHOLD_MINTED,
};
use crate::testing::tests::setup_atom_pool;

const ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS: &str =
    "cosmos1atom_bluechip_pool_test_addr_000000000000";

fn mock_deps_with_querier(
    contract_balance: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
    let custom_querier: WasmMockQuerier =
        WasmMockQuerier::new(MockQuerier::new(&[(MOCK_CONTRACT_ADDR, contract_balance)]));

    OwnedDeps {
        storage: MockStorage::default(),
        api: MockApi::default(),
        querier: custom_querier,
        custom_query_type: Default::default(),
    }
}

fn default_factory_config() -> FactoryInstantiate {
    FactoryInstantiate {
        cw721_nft_contract_id: 58,
        factory_admin_address: Addr::unchecked("admin"),
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("ubluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 14,
        atom_bluechip_anchor_pool_address: Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS),
        bluechip_mint_contract_address: None,
    }
}

fn setup_factory(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
) {
    let config = default_factory_config();
    let env = mock_env();
    let info = mock_info("admin", &[]);
    setup_atom_pool(deps);
    instantiate(deps.as_mut(), env, info, config).unwrap();
}

// ============================================================================
// NotifyThresholdCrossed
// ============================================================================

#[test]
fn test_notify_threshold_crossed_unauthorized_caller() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register pool 1 at a specific address
    POOL_REGISTRY.save(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1")).unwrap();

    let env = mock_env();

    // A random address tries to notify - should fail
    let hacker_info = mock_info("hacker", &[]);
    let msg = ExecuteMsg::NotifyThresholdCrossed { pool_id: 1 };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(
        err.to_string().contains("Only the registered pool contract"),
        "Expected pool authorization error, got: {}",
        err
    );
}

#[test]
fn test_notify_threshold_crossed_double_call_prevention() {
    let mut deps = mock_deps_with_querier(&[
        Coin { denom: "ubluechip".to_string(), amount: Uint128::new(1_000_000_000_000) },
    ]);
    setup_factory(&mut deps);

    // Register pool 1
    POOL_REGISTRY.save(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1")).unwrap();

    // Mark as already minted
    POOL_THRESHOLD_MINTED.save(&mut deps.storage, 1, &true).unwrap();

    let env = mock_env();
    let pool_info = mock_info("pool_contract_1", &[]);
    let msg = ExecuteMsg::NotifyThresholdCrossed { pool_id: 1 };

    let err = execute(deps.as_mut(), env, pool_info, msg).unwrap_err();
    assert!(
        err.to_string().contains("already triggered"),
        "Expected double-mint prevention error, got: {}",
        err
    );
}

#[test]
fn test_notify_threshold_crossed_unregistered_pool() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Don't register any pool in POOL_REGISTRY

    let env = mock_env();
    let pool_info = mock_info("pool_contract_1", &[]);
    let msg = ExecuteMsg::NotifyThresholdCrossed { pool_id: 999 };

    let err = execute(deps.as_mut(), env, pool_info, msg).unwrap_err();
    assert!(
        err.to_string().contains("not found in registry"),
        "Expected registry error, got: {}",
        err
    );
}

// ============================================================================
// CancelConfigUpdate
// ============================================================================

#[test]
fn test_cancel_config_update() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    // Propose a config update first
    let new_config = default_factory_config();
    let propose_msg = ExecuteMsg::ProposeConfigUpdate { config: new_config };
    execute(deps.as_mut(), env.clone(), admin_info.clone(), propose_msg).unwrap();

    // Verify pending config exists
    assert!(PENDING_CONFIG.may_load(&deps.storage).unwrap().is_some());

    // Cancel it
    let cancel_msg = ExecuteMsg::CancelConfigUpdate {};
    let res = execute(deps.as_mut(), env, admin_info, cancel_msg).unwrap();

    assert!(res.attributes.iter().any(|a| a.value == "cancel_config_update"));

    // Pending config should be gone
    assert!(PENDING_CONFIG.may_load(&deps.storage).unwrap().is_none());
}

#[test]
fn test_cancel_config_update_unauthorized() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    // Propose
    let propose_msg = ExecuteMsg::ProposeConfigUpdate { config: default_factory_config() };
    execute(deps.as_mut(), env.clone(), admin_info, propose_msg).unwrap();

    // Non-admin tries to cancel
    let hacker_info = mock_info("hacker", &[]);
    let cancel_msg = ExecuteMsg::CancelConfigUpdate {};
    let err = execute(deps.as_mut(), env, hacker_info, cancel_msg).unwrap_err();
    assert!(err.to_string().contains("Only the admin"));
}

// ============================================================================
// Config Timelock Enforcement
// ============================================================================

#[test]
fn test_config_update_before_timelock_fails() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut env = mock_env();
    let admin_info = mock_info("admin", &[]);

    // Propose config update
    let propose_msg = ExecuteMsg::ProposeConfigUpdate { config: default_factory_config() };
    execute(deps.as_mut(), env.clone(), admin_info.clone(), propose_msg).unwrap();

    // Try to execute immediately (before 48h timelock)
    let update_msg = ExecuteMsg::UpdateConfig {};
    let err = execute(deps.as_mut(), env.clone(), admin_info.clone(), update_msg.clone()).unwrap_err();

    match err {
        ContractError::TimelockNotExpired { effective_after } => {
            assert!(effective_after > env.block.time);
        }
        _ => panic!("Expected TimelockNotExpired, got: {}", err),
    }

    // Advance time past 48 hours
    env.block.time = env.block.time.plus_seconds(86400 * 2 + 1);
    let res = execute(deps.as_mut(), env, admin_info, update_msg).unwrap();
    assert!(res.attributes.iter().any(|a| a.value == "execute_update_config"));
}

// ============================================================================
// UpdatePoolConfig
// ============================================================================

#[test]
fn test_update_pool_config_sends_message_to_pool() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register a pool
    POOL_REGISTRY.save(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1")).unwrap();

    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    let update = PoolConfigUpdate {
        commit_fee_info: None,
        commit_limit_usd: None,
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: None,
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
        lp_fee: Some(Decimal::percent(5)),
        min_commit_interval: Some(120),
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    let msg = ExecuteMsg::UpdatePoolConfig {
        pool_id: 1,
        pool_config: update,
    };

    let res = execute(deps.as_mut(), env, admin_info, msg).unwrap();

    // Should send a WasmMsg::Execute to the pool contract
    assert_eq!(res.messages.len(), 1);
    assert!(res.attributes.iter().any(|a| a.key == "pool_id" && a.value == "1"));
}

#[test]
fn test_update_pool_config_unauthorized() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    POOL_REGISTRY.save(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1")).unwrap();

    let env = mock_env();
    let hacker_info = mock_info("hacker", &[]);

    let update = PoolConfigUpdate {
        commit_fee_info: None,
        commit_limit_usd: None,
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: None,
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
        lp_fee: Some(Decimal::percent(5)),
        min_commit_interval: None,
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    let msg = ExecuteMsg::UpdatePoolConfig {
        pool_id: 1,
        pool_config: update,
    };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(err.to_string().contains("Only the admin"));
}

#[test]
fn test_update_pool_config_nonexistent_pool() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Don't register pool 99
    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    let update = PoolConfigUpdate {
        commit_fee_info: None,
        commit_limit_usd: None,
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: None,
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
        lp_fee: None,
        min_commit_interval: None,
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    let msg = ExecuteMsg::UpdatePoolConfig {
        pool_id: 99,
        pool_config: update,
    };

    let err = execute(deps.as_mut(), env, admin_info, msg).unwrap_err();
    // Pool 99 not found in registry
    assert!(err.to_string().contains("not found") || err.to_string().contains("type: cw_storage_plus"));
}

