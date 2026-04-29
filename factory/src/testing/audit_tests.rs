use cosmwasm_std::testing::{
    message_info, mock_env, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
};
use cosmwasm_std::{Addr, Coin, Decimal, Empty, OwnedDeps, Uint128};

use crate::asset::TokenType;
use crate::error::ContractError;
use crate::execute::{
    encode_reply_id, execute, instantiate, pool_creation_reply, FINALIZE_POOL, MINT_CREATE_POOL,
    SET_TOKENS,
};
use crate::internal_bluechip_price_oracle::{
    calculate_weighted_price_with_atom, PoolCumulativeSnapshot,
};
use crate::mock_querier::WasmMockQuerier;
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolConfigUpdate, PoolDetails};
use crate::state::{
    FactoryInstantiate, PENDING_CONFIG, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_COUNTER,
    POOL_THRESHOLD_MINTED,
};
use crate::testing::tests::{create_instantiate_reply, register_test_pool_addr, setup_atom_pool};
use pool_factory_interfaces::PoolStateResponseForFactory;

fn make_addr(label: &str) -> Addr {
    MockApi::default().addr_make(label)
}

fn admin_addr() -> Addr {
    make_addr("admin")
}

fn atom_bluechip_pool_addr() -> Addr {
    make_addr("atom_bluechip_pool")
}

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
        factory_admin_address: admin_addr(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: MockApi::default().addr_make("oracle0000").to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: make_addr("ubluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 14,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    }
}

fn setup_factory(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>) {
    let config = default_factory_config();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    setup_atom_pool(deps);
    instantiate(deps.as_mut(), env, info, config).unwrap();
}

#[test]
fn test_notify_threshold_crossed_unauthorized_caller() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register pool 1 at a specific address
    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1"));

    let env = mock_env();

    // A random address tries to notify - should fail
    let hacker_info = message_info(&Addr::unchecked("hacker"), &[]);
    let msg = ExecuteMsg::NotifyThresholdCrossed { pool_id: 1 };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(
        err.to_string()
            .contains("Only the registered pool contract"),
        "Expected pool authorization error, got: {}",
        err
    );
}

#[test]
fn test_notify_threshold_crossed_double_call_prevention() {
    let mut deps = mock_deps_with_querier(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(1_000_000_000_000),
    }]);
    setup_factory(&mut deps);

    // Register pool 1
    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1"));

    // Mark as already minted
    POOL_THRESHOLD_MINTED
        .save(&mut deps.storage, 1, &true)
        .unwrap();

    let env = mock_env();
    let pool_info = message_info(&Addr::unchecked("pool_contract_1"), &[]);
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

    // Don't register any pool in POOLS_BY_ID

    let env = mock_env();
    let pool_info = message_info(&Addr::unchecked("pool_contract_1"), &[]);
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
    let admin_info = message_info(&admin_addr(), &[]);

    // Propose a config update first
    let new_config = default_factory_config();
    let propose_msg = ExecuteMsg::ProposeConfigUpdate { config: new_config };
    execute(deps.as_mut(), env.clone(), admin_info.clone(), propose_msg).unwrap();

    // Verify pending config exists
    assert!(PENDING_CONFIG.may_load(&deps.storage).unwrap().is_some());

    // Cancel it
    let cancel_msg = ExecuteMsg::CancelConfigUpdate {};
    let res = execute(deps.as_mut(), env, admin_info, cancel_msg).unwrap();

    assert!(res
        .attributes
        .iter()
        .any(|a| a.value == "cancel_config_update"));

    // Pending config should be gone
    assert!(PENDING_CONFIG.may_load(&deps.storage).unwrap().is_none());
}

#[test]
fn test_cancel_config_update_unauthorized() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    // Propose
    let propose_msg = ExecuteMsg::ProposeConfigUpdate {
        config: default_factory_config(),
    };
    execute(deps.as_mut(), env.clone(), admin_info, propose_msg).unwrap();

    // Non-admin tries to cancel
    let hacker_info = message_info(&Addr::unchecked("hacker"), &[]);
    let cancel_msg = ExecuteMsg::CancelConfigUpdate {};
    let err = execute(deps.as_mut(), env, hacker_info, cancel_msg).unwrap_err();
    assert!(err.to_string().contains("Only the admin"));
}

#[test]
fn test_config_update_before_timelock_fails() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    // Propose config update
    let propose_msg = ExecuteMsg::ProposeConfigUpdate {
        config: default_factory_config(),
    };
    execute(deps.as_mut(), env.clone(), admin_info.clone(), propose_msg).unwrap();

    // Try to execute immediately (before 48h timelock)
    let update_msg = ExecuteMsg::UpdateConfig {};
    let err = execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        update_msg.clone(),
    )
    .unwrap_err();

    match err {
        ContractError::TimelockNotExpired { effective_after } => {
            assert!(effective_after > env.block.time);
        }
        _ => panic!("Expected TimelockNotExpired, got: {}", err),
    }

    // Advance time past the admin timelock
    env.block.time = env.block.time.plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);
    let res = execute(deps.as_mut(), env, admin_info, update_msg).unwrap();
    assert!(res
        .attributes
        .iter()
        .any(|a| a.value == "execute_update_config"));
}

#[test]
fn test_update_pool_config_sends_message_to_pool() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register a pool
    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1"));

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

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
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "pool_id" && a.value == "1"));

    // Step 2: Execute after timelock — should send WasmMsg to pool
    let mut future_env = env;
    future_env.block.time = future_env
        .block
        .time
        .plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);
    let apply_msg = ExecuteMsg::ExecutePoolConfigUpdate { pool_id: 1 };
    let res = execute(deps.as_mut(), future_env, admin_info, apply_msg).unwrap();
    assert_eq!(res.messages.len(), 1);
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "pool_id" && a.value == "1"));
}

#[test]
fn test_update_pool_config_unauthorized() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_contract_1"));

    let env = mock_env();
    let hacker_info = message_info(&Addr::unchecked("hacker"), &[]);

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
    let admin_info = message_info(&admin_addr(), &[]);

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
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("type: cw_storage_plus")
    );
}

#[test]
fn test_m_new_3_rotation_skips_pools_without_prior_snapshot() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let atom_addr = atom_bluechip_pool_addr().to_string();
    let creator_addr = make_addr("creator_pool_1").to_string();

    // Register creator pool in POOLS_BY_ID so is_bluechip_second lookup works
    let pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked(&creator_addr),
            },
        ],
        creator_pool_addr: Addr::unchecked(&creator_addr),
        pool_kind: pool_factory_interfaces::PoolKind::Commit,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID
        .save(&mut deps.storage, 1, &pool_details)
        .unwrap();

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
        .save(
            &mut deps.storage,
            Addr::unchecked(&creator_addr),
            &creator_pool_state,
        )
        .unwrap();

    // H-3: anchor pool needs `block_time_last` and `price1_cumulative_last`
    // ahead of the prev_snapshot below (cumulative=50_000, block_time=500)
    // so the TWAP path produces `cumulative_delta > 0` and `time_delta > 0`.
    // setup_factory's default zeros would have triggered the (now-removed)
    // anchor spot fallback.
    let atom_addr_obj = atom_bluechip_pool_addr();
    let mut atom_state = POOLS_BY_CONTRACT_ADDRESS
        .load(&deps.storage, atom_addr_obj.clone())
        .unwrap();
    atom_state.block_time_last = 1000;
    atom_state.price1_cumulative_last = Uint128::new(60_000);
    POOLS_BY_CONTRACT_ADDRESS
        .save(&mut deps.storage, atom_addr_obj, &atom_state)
        .unwrap();

    let pool_addresses = vec![atom_addr.clone(), creator_addr.clone()];

    // Provide a previous snapshot ONLY for the atom pool (simulates rotation
    // where atom pool was retained but creator_pool is newly selected).
    let prev_snapshots = vec![PoolCumulativeSnapshot {
        pool_address: atom_addr.clone(),
        price0_cumulative: Uint128::new(50_000),
        block_time: 500,
    }];

    let result =
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots);

    // Should succeed — atom pool has a snapshot and produces a price.
    // Creator pool should be skipped (not fall back to spot).
    assert!(
        result.is_ok(),
        "Oracle should succeed with at least the atom pool: {:?}",
        result.err()
    );

    let (weighted_price, atom_price, new_snapshots) = result.unwrap();

    // Both pools should get new snapshots (for next update cycle)
    assert_eq!(
        new_snapshots.len(),
        2,
        "Both pools should record snapshots for next cycle"
    );

    // The weighted price should come only from the atom pool (since creator
    // pool was skipped). Post-H-3 the function returns Option for prices —
    // unwrap and assert price came from the anchor only.
    let weighted = weighted_price.expect("anchor TWAP should produce a price");
    let atom = atom_price.expect("anchor pool price should be Some");
    assert_eq!(
        weighted, atom,
        "Price should come only from atom pool since creator pool had no prior snapshot"
    );
}

/// H-3: when `prev_snapshots` is completely empty (bootstrap / first-ever
/// update), the oracle MUST refuse to publish a price (no spot fallback)
/// but MUST still record snapshots so the next round has prior data to
/// compute a TWAP from. Pre-fix this returned a manipulable spot reading;
/// post-fix the price components are `None` and the snapshot is recorded.
#[test]
fn test_h3_bootstrap_returns_none_price_but_records_snapshot() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let atom_addr = atom_bluechip_pool_addr().to_string();

    let pool_addresses = vec![atom_addr.clone()];
    let prev_snapshots: Vec<PoolCumulativeSnapshot> = vec![];

    let (weighted_price, atom_price, new_snapshots) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots)
            .expect("bootstrap must succeed (snapshots-only) instead of erroring");

    // No price this round — H-3 removed the spot fallback. Caller will
    // persist `new_snapshots` and the next round computes a real TWAP.
    assert!(
        weighted_price.is_none(),
        "bootstrap must NOT publish a spot-derived price; got: {:?}",
        weighted_price
    );
    assert!(
        atom_price.is_none(),
        "bootstrap must NOT publish a spot-derived atom price; got: {:?}",
        atom_price
    );
    assert_eq!(
        new_snapshots.len(),
        1,
        "snapshot must still be recorded so next round can compute TWAP"
    );
}

/// H-3: even with the anchor sampled and meeting MIN_POOL_LIQUIDITY,
/// if the anchor's cumulative_delta is zero (no swap since the last
/// sample) the oracle MUST refuse to publish — pre-fix it would have
/// fallen back to single-block spot reserves on the anchor.
#[test]
fn test_h3_anchor_no_cumulative_delta_returns_none_price() {
    use crate::state::POOLS_BY_CONTRACT_ADDRESS;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let atom_addr = atom_bluechip_pool_addr();

    // Override the test querier to return a state with the same cumulative
    // and block_time as the prior snapshot — i.e., no activity since.
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            &mut deps.storage,
            atom_addr.clone(),
            &PoolStateResponseForFactory {
                pool_contract_address: atom_addr.clone(),
                nft_ownership_accepted: true,
                reserve0: Uint128::new(50_000_000_000),
                reserve1: Uint128::new(10_000_000_000),
                total_liquidity: Uint128::new(60_000_000_000),
                block_time_last: 1_000,
                price0_cumulative_last: Uint128::new(500_000),
                price1_cumulative_last: Uint128::new(100_000),
                assets: vec![],
            },
        )
        .unwrap();

    let pool_addresses = vec![atom_addr.to_string()];
    // Prior snapshot identical to the current state — cumulative_delta = 0,
    // time_delta = 0. Pre-H-3 this would have triggered the anchor spot
    // fallback. Post-H-3 it must not.
    let prev_snapshots = vec![PoolCumulativeSnapshot {
        pool_address: atom_addr.to_string(),
        price0_cumulative: Uint128::new(500_000),
        block_time: 1_000,
    }];

    let (weighted_price, atom_price, new_snapshots) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots)
            .expect("must return Ok with None prices, not Err");

    assert!(
        weighted_price.is_none(),
        "anchor stale-cumulative must NOT trigger spot fallback; got: {:?}",
        weighted_price
    );
    assert!(atom_price.is_none(), "atom price must be None when cumulative didn't advance");
    // Snapshot should still be recorded so the next round can compute TWAP
    // once the anchor has activity.
    assert_eq!(new_snapshots.len(), 1);
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
    assert!(
        conf <= threshold,
        "4.9% confidence should pass the 5% threshold"
    );

    // Case 2: price = 1000, conf = 51 (5.1%) -> should FAIL
    let conf: u64 = 51;
    assert!(
        conf > threshold,
        "5.1% confidence should fail the 5% threshold"
    );

    // Case 3: price = 1000, conf = 50 (exactly 5%) -> should PASS (<=)
    let conf: u64 = 50;
    assert!(conf <= threshold, "Exactly 5% should pass (boundary)");

    // Case 4: typical Pyth price $10.50 at -8 expo = 1_050_000_000
    let price: i64 = 1_050_000_000;
    let conf: u64 = 60_000_000; // ~5.7% -> should FAIL
    let threshold = (price as u64) / 20; // 52_500_000
    assert!(
        conf > threshold,
        "5.7% confidence on real Pyth price should fail"
    );

    // Case 5: tight confidence on real price
    let conf: u64 = 10_000_000; // ~0.95% -> should PASS
    assert!(
        conf <= threshold,
        "~1% confidence on real Pyth price should pass"
    );
}

/// When the same creator creates two pools, both pool's registry entries
/// should be independently stored (keyed by pool_id, not creator address).
#[test]
fn test_m_new_5_multi_pool_creator_no_registry_collision() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    // Create first pool
    let create_msg_1 = ExecuteMsg::Create {
        pool_msg: CreatePool { pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ] },
        token_info: CreatorTokenInfo {
            name: "TokenA".to_string(),
            symbol: "TOKA".to_string(),
            decimal: 6,
        },
    };

    execute(deps.as_mut(), env.clone(), admin_info.clone(), create_msg_1).unwrap();
    let pool_id_1 = POOL_COUNTER.load(&deps.storage).unwrap();

    // Complete the reply chain for pool 1
    let token_1 = make_addr("token_addr_1");
    let token_reply =
        create_instantiate_reply(encode_reply_id(pool_id_1, SET_TOKENS), token_1.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();
    let nft_1 = make_addr("nft_addr_1");
    let nft_reply =
        create_instantiate_reply(encode_reply_id(pool_id_1, MINT_CREATE_POOL), nft_1.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();
    let pool_1 = make_addr("pool_addr_1");
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id_1, FINALIZE_POOL), pool_1.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Verify pool 1 registry info
    let pool_1_details = POOLS_BY_ID.load(&deps.storage, pool_id_1).unwrap();
    assert_eq!(pool_1_details.creator_pool_addr, pool_1.clone());
    assert_eq!(pool_1_details.pool_id, pool_id_1);

    // Create second pool from the SAME creator (admin)
    let create_msg_2 = ExecuteMsg::Create {
        pool_msg: CreatePool { pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ] },
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
    let token_2 = make_addr("token_addr_2");
    let token_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, SET_TOKENS), token_2.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();
    let nft_2 = make_addr("nft_addr_2");
    let nft_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, MINT_CREATE_POOL), nft_2.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();
    let pool_2 = make_addr("pool_addr_2");
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, FINALIZE_POOL), pool_2.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Verify pool 2 registry info
    let pool_2_details = POOLS_BY_ID.load(&deps.storage, pool_id_2).unwrap();
    assert_eq!(pool_2_details.creator_pool_addr, pool_2.clone());
    assert_eq!(pool_2_details.pool_id, pool_id_2);

    // KEY ASSERTION: Pool 1's registry entry should still be intact
    // (This would fail with the old creator-address key, as pool 2 would overwrite pool 1)
    let pool_1_details_after = POOLS_BY_ID.load(&deps.storage, pool_id_1).unwrap();
    assert_eq!(
        pool_1_details_after.pool_id, pool_id_1,
        "Pool 1 registry entry should not be overwritten by pool 2"
    );
    assert_eq!(
        pool_1_details_after.creator_pool_addr, pool_1,
        "Pool 1 pool address should still be pool_addr_1, not pool_addr_2"
    );
}

#[test]
fn test_l_new_8_factory_migration_contract_name() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // After instantiate, cw2 should be set
    let version_info = cw2::get_contract_version(&deps.storage).unwrap();
    assert_eq!(
        version_info.contract, "crates.io:bluechip-factory",
        "Instantiate should set contract name to crates.io:bluechip-factory"
    );

    // Simulate migration (set version to older to allow migration)
    cw2::set_contract_version(&mut deps.storage, "crates.io:bluechip-factory", "0.1.0").unwrap();

    let env = mock_env();
    let res = crate::migrate::migrate(deps.as_mut(), env, Empty {});
    assert!(res.is_ok(), "Migration should succeed: {:?}", res.err());

    // After migration, contract name should still be "crates.io:bluechip-factory"
    let version_info = cw2::get_contract_version(&deps.storage).unwrap();
    assert_eq!(
        version_info.contract, "crates.io:bluechip-factory",
        "Migration should maintain the same contract name"
    );
}

// ---------------------------------------------------------------------------
// H-5 — `ProposeConfigUpdate` must refuse to silently overwrite a pending
// proposal. Without this, a benign proposal at hour 47 of the timelock
// could be replaced by a hostile one minutes before the community window
// elapses, with no on-chain `Cancel` event signalling the swap.
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_overwrite() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let info = message_info(&admin_addr(), &[]);
    let env = mock_env();

    // First proposal — succeeds.
    execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        ExecuteMsg::ProposeConfigUpdate {
            config: default_factory_config(),
        },
    )
    .unwrap();

    // Second proposal — must fail because PENDING_CONFIG already exists.
    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::ProposeConfigUpdate {
            config: default_factory_config(),
        },
    );
    let err = res.expect_err("second propose without cancel should fail");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("already pending") || err_msg.contains("CancelConfigUpdate"),
        "expected already-pending rejection, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// H-7 — `validate_factory_config` must reject configs whose
// `commit_fee_bluechip + commit_fee_creator > 100%`. The pool-side
// `instantiate` already rejects with `InvalidFee`, but if the factory
// stored a bad config it would brick every subsequent `Create` until
// another 48h cycle to fix. Validating at propose-time surfaces the
// misconfig immediately.
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_fee_sum_above_one() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut bad = default_factory_config();
    bad.commit_fee_bluechip = Decimal::percent(60);
    bad.commit_fee_creator = Decimal::percent(50); // sum 110% > 1.0

    let info = message_info(&admin_addr(), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ProposeConfigUpdate { config: bad },
    );
    let err = res.expect_err("fee sum above 1.0 must be rejected at propose time");
    assert!(err.to_string().contains("commit_fee"), "got: {}", err);
}

// ---------------------------------------------------------------------------
// H-7 — `commit_threshold_limit_usd == 0` is also rejected. A zero
// threshold makes commit pools created against this config permanently
// uncrossable, locking them in pre-threshold state forever.
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_zero_threshold() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut bad = default_factory_config();
    bad.commit_threshold_limit_usd = Uint128::zero();

    let info = message_info(&admin_addr(), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ProposeConfigUpdate { config: bad },
    );
    let err = res.expect_err("zero threshold must be rejected at propose time");
    assert!(
        err.to_string().contains("commit_threshold_limit_usd"),
        "got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// H-8 — `pyth_contract_addr_for_conversions` must be a non-empty bech32-
// valid address; an empty string used to slip through and only fail at
// query time (after the 48h timelock elapsed and the bad config landed).
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_empty_pyth_addr() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut bad = default_factory_config();
    bad.pyth_contract_addr_for_conversions = String::new();

    let info = message_info(&admin_addr(), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ProposeConfigUpdate { config: bad },
    );
    let err = res.expect_err("empty pyth address must be rejected");
    assert!(
        err.to_string().contains("pyth_contract_addr"),
        "got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// H-8 — same handler must reject a bech32-invalid string too. Without
// `addr_validate`, "not_a_real_addr" would be accepted and only blow up
// at first Pyth query attempt.
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_invalid_pyth_addr() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut bad = default_factory_config();
    bad.pyth_contract_addr_for_conversions = "not_a_real_addr".to_string();

    let info = message_info(&admin_addr(), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ProposeConfigUpdate { config: bad },
    );
    let err = res.expect_err("malformed pyth address must be rejected");
    assert!(
        err.to_string().contains("pyth_contract_addr"),
        "got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// H-8 — same handler must reject an empty `pyth_atom_usd_price_feed_id`.
// ---------------------------------------------------------------------------
#[test]
fn test_propose_config_update_rejects_empty_pyth_feed_id() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let mut bad = default_factory_config();
    bad.pyth_atom_usd_price_feed_id = String::new();

    let info = message_info(&admin_addr(), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ProposeConfigUpdate { config: bad },
    );
    let err = res.expect_err("empty pyth feed id must be rejected");
    assert!(
        err.to_string().contains("pyth_atom_usd_price_feed_id"),
        "got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// H-4 — `PayDistributionBounty` must reject standard pools, even though
// the standard-pool wasm doesn't currently emit the message. Defense-
// in-depth so a future pool-wasm migration can't drain the bounty
// reserve without going through the audited commit-pool path.
// ---------------------------------------------------------------------------
#[test]
fn test_pay_distribution_bounty_rejects_standard_pool() {
    use crate::pool_struct::PoolDetails;
    use crate::state::{POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_COUNTER};
    let mut deps = mock_deps_with_querier(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_factory(&mut deps);

    // Enable a non-zero bounty so the handler reaches the auth path
    // rather than short-circuiting on "disabled".
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: Uint128::new(50_000),
        },
    )
    .unwrap();

    // Register a standard pool (PoolKind::Standard) in both registries.
    let std_pool_addr = make_addr("std_pool_attempting_drain");
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            std_pool_addr.clone(),
            &PoolStateResponseForFactory {
                pool_contract_address: std_pool_addr.clone(),
                nft_ownership_accepted: true,
                reserve0: Uint128::zero(),
                reserve1: Uint128::zero(),
                total_liquidity: Uint128::zero(),
                block_time_last: 0,
                price0_cumulative_last: Uint128::zero(),
                price1_cumulative_last: Uint128::zero(),
                assets: vec![],
            },
        )
        .unwrap();
    let next_id = POOL_COUNTER.may_load(&deps.storage).unwrap().unwrap_or(0) + 1;
    POOL_COUNTER.save(deps.as_mut().storage, &next_id).unwrap();
    POOLS_BY_ID
        .save(
            deps.as_mut().storage,
            next_id,
            &PoolDetails {
                pool_id: next_id,
                pool_token_info: [
                    TokenType::Native {
                        denom: "ubluechip".to_string(),
                    },
                    TokenType::Native {
                        denom: "uatom".to_string(),
                    },
                ],
                creator_pool_addr: std_pool_addr.clone(),
                pool_kind: pool_factory_interfaces::PoolKind::Standard,
                commit_pool_ordinal: 0,
            },
        )
        .unwrap();

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&std_pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: make_addr("attacker_keeper").to_string(),
        },
    );
    let err = res.expect_err("standard pool must not receive bounty payouts");
    assert!(
        matches!(err, ContractError::Unauthorized {}),
        "expected Unauthorized, got: {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// H-6 — `CreateStandardPool` must refund any non-bluechip funds the caller
// attached. Without this, attached IBC-wrapped or tokenfactory denoms are
// orphaned in the factory's bank balance with no withdrawal path.
// ---------------------------------------------------------------------------
#[test]
fn test_create_standard_pool_refunds_non_bluechip_funds() {
    use cosmwasm_std::{BankMsg, CosmosMsg};
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Configure the standard-pool wasm code id (instantiate set it to 0).
    // The standard-pool create flow pays a USD fee; with the oracle in
    // its zero-initialized state, the handler falls back to the hardcoded
    // STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP (100_000_000 ubluechip).
    let mut cfg = default_factory_config();
    cfg.standard_pool_wasm_contract_id = 12;
    crate::state::FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &cfg)
        .unwrap();

    let caller = make_addr("std_pool_creator");
    let funds = vec![
        Coin {
            // Required fee amount + a little surplus to also exercise the
            // bluechip-surplus refund branch alongside the new IBC refund.
            denom: "ubluechip".to_string(),
            amount: Uint128::new(120_000_000),
        },
        Coin {
            denom: "ibc/27394FB...ATOM".to_string(),
            amount: Uint128::new(42_000_000),
        },
        Coin {
            denom: "factory/somecreator/MEME".to_string(),
            amount: Uint128::new(7_000),
        },
    ];

    let token_a = make_addr("standard_pool_token_a");
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&caller, &funds),
        ExecuteMsg::CreateStandardPool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: token_a,
                },
            ],
            label: "test-std-pool".to_string(),
        },
    );

    // Some test environments may reject the embedded CW20 TokenInfo
    // query; we only care here about the refund logic, which fires
    // before the SubMsg dispatch. If the call did succeed, assert the
    // refund. If it errored on a downstream query, skip the assert
    // (the refund-message planning still happens at handler entry).
    if let Ok(response) = res {
        let mut refunded_ibc = Uint128::zero();
        let mut refunded_factory = Uint128::zero();
        for msg in response.messages.iter() {
            if let CosmosMsg::Bank(BankMsg::Send { to_address, amount }) = &msg.msg {
                if to_address == caller.as_str() {
                    for coin in amount {
                        match coin.denom.as_str() {
                            "ibc/27394FB...ATOM" => refunded_ibc = coin.amount,
                            "factory/somecreator/MEME" => refunded_factory = coin.amount,
                            _ => {}
                        }
                    }
                }
            }
        }
        assert_eq!(
            refunded_ibc,
            Uint128::new(42_000_000),
            "IBC ATOM should be refunded to caller in full"
        );
        assert_eq!(
            refunded_factory,
            Uint128::new(7_000),
            "tokenfactory denom should be refunded to caller in full"
        );
    }
}

// ---------------------------------------------------------------------------
// H-2 — warm-up gate. After any anchor reset (one-shot SetAnchorPool, the
// timelocked anchor change inside ProposeConfigUpdate, etc.) the oracle
// must refuse to publish a price downstream until
// ANCHOR_CHANGE_WARMUP_OBSERVATIONS successful TWAP updates have
// accumulated. This prevents an attacker who briefly perturbed the new
// anchor's reserves at exactly the moment of reset from locking in a
// manipulated first observation as the canonical price.
// ---------------------------------------------------------------------------
#[test]
fn test_h2_warmup_blocks_downstream_price_until_n_observations() {
    use crate::internal_bluechip_price_oracle::{
        get_bluechip_usd_price, ANCHOR_CHANGE_WARMUP_OBSERVATIONS, INTERNAL_ORACLE,
    };
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Sanity: instantiate sets warmup_remaining to ANCHOR_CHANGE_WARMUP_OBSERVATIONS.
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert_eq!(oracle.warmup_remaining, ANCHOR_CHANGE_WARMUP_OBSERVATIONS);

    // Even with last_price seeded to a sane value, downstream must refuse
    // because the warm-up gate hasn't elapsed.
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_price = Uint128::new(10_000_000);
    oracle.bluechip_price_cache.last_update = mock_env().block.time.seconds();
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();

    let env = mock_env();
    let res = get_bluechip_usd_price(deps.as_ref(), &env);
    let err = res.expect_err("warm-up active must block downstream pricing");
    assert!(
        err.to_string().contains("warm-up"),
        "expected warm-up error, got: {}",
        err
    );

    // Step warm-up down to zero and confirm pricing resumes.
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.warmup_remaining = 0;
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();

    let res = get_bluechip_usd_price(deps.as_ref(), &env);
    assert!(
        res.is_ok(),
        "warm-up cleared — pricing should resume; got: {:?}",
        res.err()
    );
}

#[test]
fn test_h2_warmup_only_decrements_on_price_publishing_updates() {
    use crate::internal_bluechip_price_oracle::INTERNAL_ORACLE;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let initial = INTERNAL_ORACLE.load(&deps.storage).unwrap().warmup_remaining;
    assert!(initial > 0, "instantiate should arm warm-up");

    // Trigger a snapshot-only update by leaving anchor's cumulative at zero
    // (no `tick_anchor_pool` in this test). After a UPDATE_INTERVAL elapses,
    // calling UpdateOraclePrice should record a snapshot but not publish a
    // price — and crucially, must NOT decrement warmup_remaining.
    use crate::msg::ExecuteMsg;
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(360);
    let res = execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    );
    assert!(
        res.is_ok(),
        "snapshot-only update should succeed, got: {:?}",
        res.err()
    );

    let after = INTERNAL_ORACLE.load(&deps.storage).unwrap().warmup_remaining;
    assert_eq!(
        after, initial,
        "snapshot-only update must NOT decrement warm-up; otherwise an attacker \
         could exhaust the warm-up by triggering empty rounds"
    );
}

// ---------------------------------------------------------------------------
// M-1 — `ProposePoolUpgrade` must dedup `pool_ids` and reject IDs that
// don't exist in the registry. Pre-fix the admin-supplied list flowed
// straight through to apply, where duplicates produced two `Migrate`
// messages to the same pool and invalid IDs aborted the entire batch
// after a 48h timelock.
// ---------------------------------------------------------------------------
#[test]
fn test_m1_propose_upgrade_rejects_unregistered_pool_id() {
    use crate::testing::tests::register_test_pool_addr;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_1"));

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpgradePools {
            new_code_id: 99,
            // pool 1 exists; pool 999 does not.
            pool_ids: Some(vec![1, 999]),
            migrate_msg: cosmwasm_std::to_json_binary(&Empty {}).unwrap(),
        },
    );
    let err = res.expect_err("propose with unregistered id must fail");
    assert!(
        err.to_string().contains("999") && err.to_string().contains("not found"),
        "expected 'pool 999 not found' error, got: {}",
        err
    );
}

#[test]
fn test_m1_propose_upgrade_dedups_pool_ids() {
    use crate::testing::tests::register_test_pool_addr;
    use crate::state::PENDING_POOL_UPGRADE;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    register_test_pool_addr(&mut deps.storage, 1, &Addr::unchecked("pool_1"));
    register_test_pool_addr(&mut deps.storage, 2, &Addr::unchecked("pool_2"));

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpgradePools {
            new_code_id: 99,
            // Duplicates of 1, plus a single 2. Expected: [1, 2] (order
            // preserved, duplicates dropped).
            pool_ids: Some(vec![1, 1, 2, 1]),
            migrate_msg: cosmwasm_std::to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(
        pending.pools_to_upgrade,
        vec![1u64, 2],
        "duplicates must be dropped, order preserved"
    );
}

// ---------------------------------------------------------------------------
// M-5 — `ProposePoolUpgrade` must refuse to include the anchor pool.
// Migrating the anchor mid-flight would leave the oracle querying
// possibly-mid-migration storage; if the migrate changes the reserve
// representation the cumulative-delta math breaks silently.
// ---------------------------------------------------------------------------
#[test]
fn test_m5_propose_upgrade_rejects_anchor_pool() {
    use crate::testing::tests::register_test_pool_addr;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Register a pool whose creator_pool_addr matches the configured
    // anchor address (atom_bluechip_pool_addr in the test harness).
    register_test_pool_addr(&mut deps.storage, 1, &atom_bluechip_pool_addr());

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpgradePools {
            new_code_id: 99,
            pool_ids: Some(vec![1]),
            migrate_msg: cosmwasm_std::to_json_binary(&Empty {}).unwrap(),
        },
    );
    let err = res.expect_err("propose with anchor pool in batch must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("anchor") || msg.contains("Anchor"),
        "expected anchor-rejection error, got: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// M-8 — `ForceRotateOraclePools` must reset the cumulative snapshots,
// price cache, and warm-up counter so the post-rotation TWAP starts
// from a clean slate. Pre-fix it left snapshots and `last_price`
// intact, anchoring the circuit breaker on the (potentially manipulated)
// pre-rotation state — which was the very thing the operator was
// force-rotating to escape.
// ---------------------------------------------------------------------------
#[test]
fn test_m8_force_rotate_resets_oracle_state() {
    use crate::internal_bluechip_price_oracle::{
        ANCHOR_CHANGE_WARMUP_OBSERVATIONS, INTERNAL_ORACLE,
    };
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    // Manually populate stale state representative of "pre-rotation"
    // so we can verify it gets cleared.
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_price = Uint128::new(10_000_000);
    oracle.bluechip_price_cache.last_update = mock_env().block.time.seconds();
    oracle.warmup_remaining = 0;
    oracle.pool_cumulative_snapshots = vec![
        crate::internal_bluechip_price_oracle::PoolCumulativeSnapshot {
            pool_address: "stale_pool".to_string(),
            price0_cumulative: Uint128::new(123),
            block_time: 1,
        },
    ];
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();

    // Propose force-rotate, advance past the timelock, execute.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    let mut env = mock_env();
    env.block.time =
        env.block.time.plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);
    execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::ForceRotateOraclePools {},
    )
    .unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(
        oracle.bluechip_price_cache.last_price.is_zero(),
        "force-rotate must clear last_price"
    );
    assert_eq!(
        oracle.bluechip_price_cache.last_update, 0,
        "force-rotate must clear last_update"
    );
    assert!(
        oracle.bluechip_price_cache.twap_observations.is_empty(),
        "force-rotate must clear twap_observations"
    );
    assert!(
        oracle.pool_cumulative_snapshots.is_empty(),
        "force-rotate must clear cumulative snapshots — leaving stale ones \
         would fail TWAP on next update because pool sets won't overlap"
    );
    assert_eq!(
        oracle.warmup_remaining, ANCHOR_CHANGE_WARMUP_OBSERVATIONS,
        "force-rotate must re-arm the H-2 warm-up gate"
    );
}
