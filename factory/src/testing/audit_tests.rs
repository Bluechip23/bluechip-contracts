
use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
};
use cosmwasm_std::{

    Addr, Coin, Decimal, Empty, OwnedDeps, Uint128,

};

use crate::error::ContractError;
use crate::execute::{execute, instantiate, pool_creation_reply, encode_reply_id, FINALIZE_POOL, MINT_CREATE_POOL, SET_TOKENS};
use crate::internal_bluechip_price_oracle::{
    calculate_weighted_price_with_atom, BlueChipPriceInternalOracle, PriceCache,
    PoolCumulativeSnapshot, INTERNAL_ORACLE,
};
use crate::asset::TokenType;
use crate::mock_querier::WasmMockQuerier;
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolConfigUpdate, PoolDetails};
use crate::state::{
    FactoryInstantiate, PENDING_CONFIG, POOL_COUNTER, POOL_REGISTRY, POOL_THRESHOLD_MINTED,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, SETCOMMIT, TEMP_POOL_CREATION,
};
use crate::testing::tests::{setup_atom_pool, create_instantiate_reply};
use pool_factory_interfaces::PoolStateResponseForFactory;

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


#[test]
fn test_update_pool_config_sends_message_to_pool() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register a pool
    POOL_REGISTRY.save(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1")).unwrap();

    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::percent(5)),
        min_commit_interval: Some(120),
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    // Step 1: Propose — no messages sent yet
    let msg = ExecuteMsg::ProposePoolConfigUpdate {
        pool_id: 1,
        pool_config: update,
    };
    let res = execute(deps.as_mut(), env.clone(), admin_info.clone(), msg).unwrap();
    assert_eq!(res.messages.len(), 0);
    assert!(res.attributes.iter().any(|a| a.key == "pool_id" && a.value == "1"));

    // Step 2: Execute after timelock — should send WasmMsg to pool
    let mut future_env = env;
    future_env.block.time = future_env.block.time.plus_seconds(86400 * 2 + 1);
    let apply_msg = ExecuteMsg::ExecutePoolConfigUpdate { pool_id: 1 };
    let res = execute(deps.as_mut(), future_env, admin_info, apply_msg).unwrap();
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
        lp_fee: Some(Decimal::percent(5)),
        min_commit_interval: None,
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    let msg = ExecuteMsg::ProposePoolConfigUpdate {
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
        lp_fee: None,
        min_commit_interval: None,
        usd_payment_tolerance_bps: None,
        oracle_address: None,
    };

    let msg = ExecuteMsg::ProposePoolConfigUpdate {
        pool_id: 99,
        pool_config: update,
    };

    let err = execute(deps.as_mut(), env, admin_info, msg).unwrap_err();
    // Pool 99 not found in registry
    assert!(err.to_string().contains("not found") || err.to_string().contains("type: cw_storage_plus"));
}

#[test]
fn test_m_new_3_rotation_skips_pools_without_prior_snapshot() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let atom_addr = ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string();
    let creator_addr = "creator_pool_1".to_string();

    // Register creator pool in POOLS_BY_ID so is_bluechip_second lookup works
    let pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::Bluechip { denom: "ubluechip".to_string() },
            TokenType::CreatorToken { contract_addr: Addr::unchecked(&creator_addr) },
        ],
        creator_pool_addr: Addr::unchecked(&creator_addr),
    };
    POOLS_BY_ID.save(&mut deps.storage, 1, &pool_details).unwrap();

    // Save pool state for the creator pool with enough liquidity
    let creator_pool_state = PoolStateResponseForFactory {
        pool_contract_address: Addr::unchecked(&creator_addr),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(50_000_000_000),
        reserve1: Uint128::new(10_000_000_000),
        total_liquidity: Uint128::new(10_000_000),
        block_time_last: 1000,
        price0_cumulative_last: Uint128::new(500_000),
        price1_cumulative_last: Uint128::new(100_000),
        assets: vec![],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(&mut deps.storage, Addr::unchecked(&creator_addr), &creator_pool_state)
        .unwrap();

    let pool_addresses = vec![atom_addr.clone(), creator_addr.clone()];

    // Provide a previous snapshot ONLY for the atom pool (simulates rotation
    // where atom pool was retained but creator_pool is newly selected).
    let prev_snapshots = vec![
        PoolCumulativeSnapshot {
            pool_address: atom_addr.clone(),
            price0_cumulative: Uint128::new(50_000),
            block_time: 500,
        },
    ];

    let result = calculate_weighted_price_with_atom(
        deps.as_ref(),
        &pool_addresses,
        &prev_snapshots,
    );

    // Should succeed — atom pool has a snapshot and produces a price.
    // Creator pool should be skipped (not fall back to spot).
    assert!(result.is_ok(), "Oracle should succeed with at least the atom pool: {:?}", result.err());

    let (weighted_price, atom_price, new_snapshots) = result.unwrap();

    // Both pools should get new snapshots (for next update cycle)
    assert_eq!(new_snapshots.len(), 2, "Both pools should record snapshots for next cycle");

    // The weighted price should come only from the atom pool (since creator
    // pool was skipped). This means weighted_price == atom_pool_price.
    assert_eq!(weighted_price, atom_price,
        "Price should come only from atom pool since creator pool had no prior snapshot");
}

/// When prev_snapshots is completely empty (bootstrap / first-ever update),
/// all pools should fall back to spot price — not be skipped.
#[test]
fn test_m_new_3_bootstrap_uses_spot_price_for_all() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let atom_addr = ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string();

    let pool_addresses = vec![atom_addr.clone()];
    let prev_snapshots: Vec<PoolCumulativeSnapshot> = vec![];

    let result = calculate_weighted_price_with_atom(
        deps.as_ref(),
        &pool_addresses,
        &prev_snapshots,
    );

    // Should succeed using spot price (bootstrap case)
    assert!(result.is_ok(), "Bootstrap case should use spot price: {:?}", result.err());

    let (weighted_price, _atom_price, new_snapshots) = result.unwrap();
    assert!(!weighted_price.is_zero(), "Should produce a non-zero price from spot reserves");
    assert_eq!(new_snapshots.len(), 1, "Should record snapshot for next cycle");
}

#[test]
fn test_m_new_4_confidence_interval_threshold_arithmetic() {
    // The check in query_pyth_atom_usd_price is:
    //   let conf_threshold = (price_data.price as u64) / 20; // 5%
    //   if price_data.conf > conf_threshold { return Err(...) }

    // Case 1: price = 1000, conf = 49 (4.9%) -> should PASS
    let price: i64 = 1000;
    let conf: u64 = 49;
    let threshold = (price as u64) / 20;
    assert_eq!(threshold, 50);
    assert!(conf <= threshold, "4.9% confidence should pass the 5% threshold");

    // Case 2: price = 1000, conf = 51 (5.1%) -> should FAIL
    let conf: u64 = 51;
    assert!(conf > threshold, "5.1% confidence should fail the 5% threshold");

    // Case 3: price = 1000, conf = 50 (exactly 5%) -> should PASS (<=)
    let conf: u64 = 50;
    assert!(conf <= threshold, "Exactly 5% should pass (boundary)");

    // Case 4: typical Pyth price $10.50 at -8 expo = 1_050_000_000
    let price: i64 = 1_050_000_000;
    let conf: u64 = 60_000_000; // ~5.7% -> should FAIL
    let threshold = (price as u64) / 20; // 52_500_000
    assert!(conf > threshold, "5.7% confidence on real Pyth price should fail");

    // Case 5: tight confidence on real price
    let conf: u64 = 10_000_000; // ~0.95% -> should PASS
    assert!(conf <= threshold, "~1% confidence on real Pyth price should pass");
}

/// When the same creator creates two pools, both pool's CommitInfo entries
/// should be independently stored (keyed by pool_id, not creator address).
#[test]
fn test_m_new_5_multi_pool_creator_no_setcommit_collision() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = mock_info("admin", &[]);

    // Create first pool
    let create_msg_1 = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip { denom: "ubluechip".to_string() },
                TokenType::CreatorToken { contract_addr: Addr::unchecked("WILL_BE_CREATED") },
            ],
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            cw20_token_contract_id: 10,
            threshold_payout: None,
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("ubluechip"),
                creator_wallet_address: Addr::unchecked("admin"),
                commit_fee_bluechip: Decimal::percent(1),
                commit_fee_creator: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(100),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "ORCL".to_string(),
            creator_token_address: Addr::unchecked("token0000"),
            max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
            creator_excess_liquidity_lock_days: 7,
            is_standard_pool: None,
        },
        token_info: CreatorTokenInfo {
            name: "TokenA".to_string(),
            symbol: "TOKA".to_string(),
            decimal: 6,
        },
    };

    execute(deps.as_mut(), env.clone(), admin_info.clone(), create_msg_1).unwrap();
    let pool_id_1 = POOL_COUNTER.load(&deps.storage).unwrap();

    // Complete the reply chain for pool 1
    let token_reply = create_instantiate_reply(encode_reply_id(pool_id_1, SET_TOKENS), "token_addr_1");
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();
    let nft_reply = create_instantiate_reply(encode_reply_id(pool_id_1, MINT_CREATE_POOL), "nft_addr_1");
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();
    let pool_reply = create_instantiate_reply(encode_reply_id(pool_id_1, FINALIZE_POOL), "pool_addr_1");
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Verify pool 1 commit info
    let commit_1 = SETCOMMIT.load(&deps.storage, pool_id_1).unwrap();
    assert_eq!(commit_1.pool_id, pool_id_1);
    assert_eq!(commit_1.creator_pool_addr, Addr::unchecked("pool_addr_1"));

    // Create second pool from the SAME creator (admin)
    let create_msg_2 = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip { denom: "ubluechip".to_string() },
                TokenType::CreatorToken { contract_addr: Addr::unchecked("WILL_BE_CREATED") },
            ],
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            cw20_token_contract_id: 10,
            threshold_payout: None,
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("ubluechip"),
                creator_wallet_address: Addr::unchecked("admin"),
                commit_fee_bluechip: Decimal::percent(1),
                commit_fee_creator: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(200),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "ORCL".to_string(),
            creator_token_address: Addr::unchecked("token0000"),
            max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
            creator_excess_liquidity_lock_days: 7,
            is_standard_pool: None,
        },
        token_info: CreatorTokenInfo {
            name: "TokenB".to_string(),
            symbol: "TOKB".to_string(),
            decimal: 6,
        },
    };

    execute(deps.as_mut(), env.clone(), admin_info, create_msg_2).unwrap();
    let pool_id_2 = POOL_COUNTER.load(&deps.storage).unwrap();
    assert_ne!(pool_id_1, pool_id_2, "Second pool should get a new ID");

    // Complete the reply chain for pool 2
    let token_reply = create_instantiate_reply(encode_reply_id(pool_id_2, SET_TOKENS), "token_addr_2");
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();
    let nft_reply = create_instantiate_reply(encode_reply_id(pool_id_2, MINT_CREATE_POOL), "nft_addr_2");
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();
    let pool_reply = create_instantiate_reply(encode_reply_id(pool_id_2, FINALIZE_POOL), "pool_addr_2");
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Verify pool 2 commit info
    let commit_2 = SETCOMMIT.load(&deps.storage, pool_id_2).unwrap();
    assert_eq!(commit_2.pool_id, pool_id_2);
    assert_eq!(commit_2.creator_pool_addr, Addr::unchecked("pool_addr_2"));

    // KEY ASSERTION: Pool 1's commit info should still be intact
    // (This would fail with the old creator-address key, as pool 2 would overwrite pool 1)
    let commit_1_after = SETCOMMIT.load(&deps.storage, pool_id_1).unwrap();
    assert_eq!(commit_1_after.pool_id, pool_id_1,
        "Pool 1 commit info should not be overwritten by pool 2");
    assert_eq!(commit_1_after.creator_pool_addr, Addr::unchecked("pool_addr_1"),
        "Pool 1 pool address should still be pool_addr_1, not pool_addr_2");
}

#[test]
fn test_l_new_8_factory_migration_contract_name() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // After instantiate, cw2 should be set
    let version_info = cw2::get_contract_version(&deps.storage).unwrap();
    assert_eq!(version_info.contract, "crates.io:bluechip-factory",
        "Instantiate should set contract name to crates.io:bluechip-factory");

    // Simulate migration (set version to older to allow migration)
    cw2::set_contract_version(&mut deps.storage, "crates.io:bluechip-factory", "0.1.0").unwrap();

    let env = mock_env();
    let res = crate::migrate::migrate(deps.as_mut(), env, Empty {});
    assert!(res.is_ok(), "Migration should succeed: {:?}", res.err());

    // After migration, contract name should still be "crates.io:bluechip-factory"
    let version_info = cw2::get_contract_version(&deps.storage).unwrap();
    assert_eq!(version_info.contract, "crates.io:bluechip-factory",
        "Migration should maintain the same contract name");
}

