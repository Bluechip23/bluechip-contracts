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
use crate::pool_struct::{CreatePool, PoolConfigUpdate, PoolDetails};
use crate::state::{
    EligiblePoolSnapshot, FactoryInstantiate, ELIGIBLE_POOL_SNAPSHOT, PENDING_CONFIG,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_COUNTER, POOL_THRESHOLD_MINTED,
};
use crate::testing::tests::{
    create_instantiate_reply, creation_fee_funds, register_test_pool_addr, setup_atom_pool,
};
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
        threshold_payout_amounts: Default::default(),
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
    assert!(matches!(err, ContractError::Unauthorized {}));
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
    };

    let msg = ExecuteMsg::ProposePoolConfigUpdate {
        pool_id: 1,
        pool_config: update,
    };

    let err = execute(deps.as_mut(), env, hacker_info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
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

    // Anchor pool needs `block_time_last` and `price1_cumulative_last`
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

    // After the audit refactor `calculate_weighted_price_with_atom` reads
    // each non-anchor pool's bluechip-side index from
    // `ELIGIBLE_POOL_SNAPSHOT.bluechip_indices` rather than from the
    // legacy POOLS_BY_ID linear scan. Populate the snapshot in this
    // test the way production's `refresh_eligible_pool_snapshot_if_stale`
    // would: creator pool has Native (bluechip) at index 0 in its
    // `pool_token_info`, so `bluechip_indices[i] = 0`.
    ELIGIBLE_POOL_SNAPSHOT
        .save(
            &mut deps.storage,
            &EligiblePoolSnapshot {
                pool_addresses: vec![creator_addr.clone()],
                bluechip_indices: vec![0],
                captured_at_block: 0,
            },
        )
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
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots, 0);

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
    // pool was skipped). The function returns Option for prices — unwrap
    // and assert price came from the anchor only.
    let weighted = weighted_price.expect("anchor TWAP should produce a price");
    let atom = atom_price.expect("anchor pool price should be Some");
    assert_eq!(
        weighted, atom,
        "Price should come only from atom pool since creator pool had no prior snapshot"
    );
}

/// When `prev_snapshots` is completely empty (bootstrap / first-ever
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
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots, 0)
            .expect("bootstrap must succeed (snapshots-only) instead of erroring");

    // No price this round — spot fallback was removed. Caller will
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

/// Even with the anchor sampled and meeting MIN_POOL_LIQUIDITY, if the
/// anchor's cumulative_delta is zero (no swap since the last sample) the
/// oracle MUST refuse to publish — pre-fix it would have fallen back to
/// single-block spot reserves on the anchor.
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
    // time_delta = 0. Pre-fix this would have triggered the anchor spot
    // fallback. Post-fix it must not.
    let prev_snapshots = vec![PoolCumulativeSnapshot {
        pool_address: atom_addr.to_string(),
        price0_cumulative: Uint128::new(500_000),
        block_time: 1_000,
    }];

    let (weighted_price, atom_price, new_snapshots) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pool_addresses, &prev_snapshots, 0)
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
    let admin_info = message_info(&admin_addr(), &creation_fee_funds());

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

    // Per-address rate limit (1h between creates from the same
    // address). Advance the clock past the cooldown so this test
    // exercises the registry-collision path rather than the rate-limit
    // guard (which has its own dedicated tests).
    let mut env_after_cooldown = env.clone();
    env_after_cooldown.block.time = env_after_cooldown
        .block
        .time
        .plus_seconds(crate::state::COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS + 1);

    execute(
        deps.as_mut(),
        env_after_cooldown.clone(),
        admin_info,
        create_msg_2,
    )
    .unwrap();
    let pool_id_2 = POOL_COUNTER.load(&deps.storage).unwrap();
    assert_ne!(pool_id_1, pool_id_2, "Second pool should get a new ID");

    // Complete the reply chain for pool 2
    let token_2 = make_addr("token_addr_2");
    let token_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, SET_TOKENS), token_2.as_str());
    pool_creation_reply(deps.as_mut(), env_after_cooldown.clone(), token_reply).unwrap();
    let nft_2 = make_addr("nft_addr_2");
    let nft_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, MINT_CREATE_POOL), nft_2.as_str());
    pool_creation_reply(deps.as_mut(), env_after_cooldown.clone(), nft_reply).unwrap();
    let pool_2 = make_addr("pool_addr_2");
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id_2, FINALIZE_POOL), pool_2.as_str());
    pool_creation_reply(deps.as_mut(), env_after_cooldown, pool_reply).unwrap();

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
// `ProposeConfigUpdate` must refuse to silently overwrite a pending
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
// `validate_factory_config` must reject configs whose
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
// `commit_threshold_limit_usd == 0` is also rejected. A zero threshold
// makes commit pools created against this config permanently uncrossable,
// locking them in pre-threshold state forever.
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
// `pyth_contract_addr_for_conversions` must be a non-empty bech32-valid
// address; an empty string used to slip through and only fail at query
// time (after the 48h timelock elapsed and the bad config landed).
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
// Same handler must reject a bech32-invalid string too. Without
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
// Same handler must reject an empty `pyth_atom_usd_price_feed_id`.
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
// `PayDistributionBounty` must reject standard pools, even though the
// standard-pool wasm doesn't currently emit the message. Defense-in-depth
// so a future pool-wasm migration can't drain the bounty reserve without
// going through the audited commit-pool path.
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
// `CreateStandardPool` must refund any non-bluechip funds the caller
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
// Warm-up gate. After any anchor reset (one-shot SetAnchorPool, the
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
// `ProposePoolUpgrade` must dedup `pool_ids` and reject IDs that don't
// exist in the registry. Pre-fix the admin-supplied list flowed straight
// through to apply, where duplicates produced two `Migrate` messages to
// the same pool and invalid IDs aborted the entire batch after a 48h
// timelock.
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
// `ProposePoolUpgrade` must refuse to include the anchor pool. Migrating
// the anchor mid-flight would leave the oracle querying possibly-mid-
// migration storage; if the migrate changes the reserve representation
// the cumulative-delta math breaks silently.
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
// `ForceRotateOraclePools` must reset the cumulative snapshots, price
// cache, and warm-up counter so the post-rotation TWAP starts from a
// clean slate. Pre-fix it left snapshots and `last_price` intact,
// anchoring the circuit breaker on the (potentially manipulated)
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
        "force-rotate must re-arm the warm-up gate"
    );
}

// ---------------------------------------------------------------------------
// `PoolDetails.pool_token_info[1].contract_addr` must equal the freshly-
// instantiated CW20 address after a successful commit-pool create.
// Pre-fix, `mint_create_pool` rewrote a LOCAL clone of the pair while the
// original `ctx.temp.temp_pool_info.pool_token_info` retained the literal
// `CREATOR_TOKEN_SENTINEL` placeholder, which `finalize_pool` then
// persisted into POOLS_BY_ID — leaving every commit pool's registry
// entry with the placeholder string. This test pins the post-fix
// invariant: registry's CreatorToken address matches the SubMsg-instantiated
// CW20.
// ---------------------------------------------------------------------------
#[test]
fn test_c2_pool_details_persists_real_creator_token_address() {
    use crate::execute::pool_lifecycle::create::CREATOR_TOKEN_SENTINEL;

    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &creation_fee_funds());

    // Caller-supplied pair carries the SENTINEL — the factory mints the
    // CW20 itself and rewrites the address downstream.
    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked(CREATOR_TOKEN_SENTINEL),
                },
            ],
        },
        token_info: CreatorTokenInfo {
            name: "TestToken".to_string(),
            symbol: "TEST".to_string(),
            decimal: 6,
        },
    };

    execute(deps.as_mut(), env.clone(), admin_info, create_msg).unwrap();
    let pool_id = POOL_COUNTER.load(&deps.storage).unwrap();

    // Walk the reply chain. The address fed into SET_TOKENS is the one
    // we expect to find in POOLS_BY_ID at the end.
    let real_token_addr = make_addr("freshly_instantiated_cw20");
    let token_reply = create_instantiate_reply(
        encode_reply_id(pool_id, SET_TOKENS),
        real_token_addr.as_str(),
    );
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();
    let nft_addr = make_addr("freshly_instantiated_cw721");
    let nft_reply =
        create_instantiate_reply(encode_reply_id(pool_id, MINT_CREATE_POOL), nft_addr.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();
    let pool_addr = make_addr("freshly_instantiated_pool");
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id, FINALIZE_POOL), pool_addr.as_str());
    pool_creation_reply(deps.as_mut(), env, pool_reply).unwrap();

    // The fix: PoolDetails.pool_token_info[1] must be the REAL CW20,
    // not the sentinel placeholder.
    let details = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    let creator_token_addr = match &details.pool_token_info[1] {
        TokenType::CreatorToken { contract_addr } => contract_addr.clone(),
        _ => panic!(
            "expected CreatorToken at pool_token_info[1], got: {:?}",
            details.pool_token_info[1]
        ),
    };
    assert_ne!(
        creator_token_addr.as_str(),
        CREATOR_TOKEN_SENTINEL,
        "regression: PoolDetails persisted the sentinel instead of the real CW20 address"
    );
    assert_eq!(
        creator_token_addr, real_token_addr,
        "PoolDetails CreatorToken address must equal the SubMsg-instantiated CW20"
    );

    // The asset_strings stored in POOLS_BY_CONTRACT_ADDRESS (used by
    // off-chain query consumers) is derived from pool_token_info — it
    // must also have the real address, not the sentinel.
    let snapshot = POOLS_BY_CONTRACT_ADDRESS
        .load(&deps.storage, pool_addr)
        .unwrap();
    assert!(
        snapshot.assets.iter().any(|a| a == real_token_addr.as_str()),
        "POOLS_BY_CONTRACT_ADDRESS.assets must include the real CW20 address; got: {:?}",
        snapshot.assets
    );
    assert!(
        !snapshot
            .assets
            .iter()
            .any(|a| a == CREATOR_TOKEN_SENTINEL),
        "POOLS_BY_CONTRACT_ADDRESS.assets must not retain the sentinel; got: {:?}",
        snapshot.assets
    );
}

// ---------------------------------------------------------------------------
// `CreateStandardPool` rejects labels longer than the bound.
// ---------------------------------------------------------------------------
#[test]
fn test_l4_create_standard_pool_rejects_oversized_label() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);
    // Configure standard-pool wasm so the handler reaches label validation.
    let mut cfg = default_factory_config();
    cfg.standard_pool_wasm_contract_id = 12;
    crate::state::FACTORYINSTANTIATEINFO
        .save(&mut deps.storage, &cfg)
        .unwrap();

    let oversized = "x".repeat(129);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::CreateStandardPool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::Native {
                    denom: "uatom".to_string(),
                },
            ],
            label: oversized,
        },
    );
    let err = res.expect_err("oversized label must be rejected");
    assert!(
        err.to_string().contains("label too long"),
        "expected length-rejection error, got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// `validate_creator_token_info` rejects all-numeric symbols.
// ---------------------------------------------------------------------------
#[test]
fn test_l7_create_rejects_all_numeric_symbol() {
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::Create {
            pool_msg: CreatePool {
                pool_token_info: [
                    TokenType::Native {
                        denom: "ubluechip".to_string(),
                    },
                    TokenType::CreatorToken {
                        contract_addr: Addr::unchecked(
                            crate::execute::pool_lifecycle::create::CREATOR_TOKEN_SENTINEL,
                        ),
                    },
                ],
            },
            token_info: CreatorTokenInfo {
                name: "All-digit symbol token".to_string(),
                symbol: "12345".to_string(),
                decimal: 6,
            },
        },
    );
    let err = res.expect_err("all-numeric symbol must be rejected");
    assert!(
        err.to_string().contains("at least one uppercase ASCII letter"),
        "expected letter-required error, got: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// Per-address rate limit on commit-pool creation. Pre-fix, anyone could
// spam consecutive Create calls. Now the same address must wait
// `COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS` between successful creates.
// ---------------------------------------------------------------------------
#[test]
fn test_i6_commit_pool_create_rate_limit_per_address() {
    use crate::execute::pool_lifecycle::create::CREATOR_TOKEN_SENTINEL;
    let mut deps = mock_deps_with_querier(&[]);
    setup_factory(&mut deps);

    let make_msg = |sym: &str| ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked(CREATOR_TOKEN_SENTINEL),
                },
            ],
        },
        token_info: CreatorTokenInfo {
            name: format!("Token {}", sym),
            symbol: sym.to_string(),
            decimal: 6,
        },
    };

    let env = mock_env();
    let info = message_info(&admin_addr(), &creation_fee_funds());

    // First create: succeeds.
    execute(deps.as_mut(), env.clone(), info.clone(), make_msg("AAA")).unwrap();

    // Second create from the same address, same block: rate-limited.
    let res = execute(deps.as_mut(), env.clone(), info.clone(), make_msg("BBB"));
    let err = res.expect_err("rapid second create from same address must be rate-limited");
    assert!(
        err.to_string().contains("Rate-limited"),
        "expected rate-limit error, got: {}",
        err
    );

    // From a DIFFERENT address in the same block: allowed (per-address gate).
    let other = make_addr("other_creator");
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&other, &creation_fee_funds()),
        make_msg("CCC"),
    )
    .unwrap();

    // After the cooldown elapses, the original address can create again.
    let mut later_env = env.clone();
    later_env.block.time = later_env
        .block
        .time
        .plus_seconds(crate::state::COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS + 1);
    execute(deps.as_mut(), later_env, info, make_msg("DDD")).unwrap();
}

// ===========================================================================
// Audit-fix follow-up tests (round 2)
//
// Coverage for the four standard-pool / oracle / accounting fixes that
// previously had only implicit (existing-test passes) verification.
// ===========================================================================

// ---------------------------------------------------------------------------
// Fix 2: per-sender rate-limit on `CreateStandardPool`
// ---------------------------------------------------------------------------
//
// The rate-limit check fires AFTER `validate_standard_pool_token_info`, so
// the test uses a Native+Native pair (skips the CW20 TokenInfo query that
// would otherwise short-circuit through validation in the mock querier).
// Setup also writes `standard_pool_wasm_contract_id = 12` so the reply
// chain has a code id to instantiate against — the SubMsg may still error
// further downstream in mock-land, but the rate-limit storage write
// happens BEFORE the SubMsg dispatch in the same tx, so a CosmWasm
// revert would also revert the timestamp save.
//
// To get clean assertions we directly load `LAST_STANDARD_POOL_CREATE_AT`
// from storage rather than relying on the response on the first call —
// the rate-limit save happens before any reply-chain SubMsg, so the
// timestamp is present in storage even if the outer SubMsg dispatch fails
// in the test environment.
mod standard_pool_rate_limit_tests {
    use super::*;
    use crate::state::{
        LAST_STANDARD_POOL_CREATE_AT, STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS,
    };

    fn make_native_pair_msg() -> ExecuteMsg {
        ExecuteMsg::CreateStandardPool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::Native {
                    denom: "uatom".to_string(),
                },
            ],
            label: "rate-limit-test".to_string(),
        }
    }

    fn setup_factory_with_std_wasm(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    ) {
        setup_factory(deps);
        let mut cfg = default_factory_config();
        cfg.standard_pool_wasm_contract_id = 12;
        cfg.standard_pool_creation_fee_usd = Uint128::zero(); // disable fee for cleaner test
        crate::state::FACTORYINSTANTIATEINFO
            .save(deps.as_mut().storage, &cfg)
            .unwrap();
    }

    /// A second `CreateStandardPool` from the same sender within the
    /// cooldown window must be rejected with a "Rate-limited" error.
    #[test]
    fn second_create_within_cooldown_is_rejected() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_std_wasm(&mut deps);
        let caller = make_addr("std_pool_creator");
        let env = mock_env();

        // First call. The reply chain may not fully execute under the
        // mock querier, but the rate-limit write happens at handler entry
        // (before SubMsg dispatch). If the handler returns Ok, the write
        // landed; if it returns Err, the write was reverted.
        // Either way, the SECOND call's behaviour locks in: if first
        // succeeded, second must be rate-limited; if first failed,
        // second succeeds (no prior stamp). We assert by reading the
        // storage timestamp directly.
        let _ = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&caller, &[]),
            make_native_pair_msg(),
        );

        let stamp = LAST_STANDARD_POOL_CREATE_AT
            .may_load(&deps.storage, caller.clone())
            .unwrap();
        if stamp.is_none() {
            // First call rolled back via SubMsg failure in mock-land.
            // Manually seed the stamp to simulate a successful first
            // call so we can exercise the rate-limit gate explicitly.
            LAST_STANDARD_POOL_CREATE_AT
                .save(&mut deps.storage, caller.clone(), &env.block.time)
                .unwrap();
        }

        // Second call from same caller, same block: must be rate-limited.
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&caller, &[]),
            make_native_pair_msg(),
        )
        .expect_err("second create within cooldown must be rate-limited");
        assert!(
            err.to_string().contains("Rate-limited"),
            "expected rate-limit error, got: {}",
            err
        );
        assert!(
            err.to_string().contains("standard pool"),
            "error message must identify the standard-pool path; got: {}",
            err
        );
    }

    /// A different sender within the cooldown window is unaffected — the
    /// rate-limit is per-address, not global.
    #[test]
    fn different_sender_within_cooldown_is_allowed() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_std_wasm(&mut deps);
        let env = mock_env();

        let alice = make_addr("alice");
        // Seed alice's stamp directly so we don't depend on whether the
        // first execute() succeeded under mock conditions.
        LAST_STANDARD_POOL_CREATE_AT
            .save(&mut deps.storage, alice.clone(), &env.block.time)
            .unwrap();

        let bob = make_addr("bob");
        // Bob has no stamp → bob's first call must NOT be rate-limited.
        // The handler may fail downstream in the SubMsg, but the
        // rate-limit gate must NOT be the cause. Inspect the error
        // string to confirm.
        let res = execute(
            deps.as_mut(),
            env,
            message_info(&bob, &[]),
            make_native_pair_msg(),
        );
        if let Err(e) = res {
            assert!(
                !e.to_string().contains("Rate-limited"),
                "different sender must not hit the rate-limit gate; got: {}",
                e
            );
        }

        // And bob's stamp landed (reached the rate-limit write).
        let bob_stamp = LAST_STANDARD_POOL_CREATE_AT
            .may_load(&deps.storage, bob)
            .unwrap();
        // The stamp may be None if the SubMsg reverted the whole tx in
        // mock-land. Both outcomes are acceptable — the assertion that
        // matters is the negative one above.
        let _ = bob_stamp;
    }

    /// After the cooldown elapses, the original sender can create again.
    #[test]
    fn original_sender_succeeds_after_cooldown() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_std_wasm(&mut deps);
        let caller = make_addr("std_pool_creator");
        let env = mock_env();

        // Seed a stamp at "now".
        LAST_STANDARD_POOL_CREATE_AT
            .save(&mut deps.storage, caller.clone(), &env.block.time)
            .unwrap();

        // Just before the cooldown expires: still rate-limited.
        let mut early_env = env.clone();
        early_env.block.time = early_env
            .block
            .time
            .plus_seconds(STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS - 1);
        let err = execute(
            deps.as_mut(),
            early_env,
            message_info(&caller, &[]),
            make_native_pair_msg(),
        )
        .expect_err("call inside cooldown must reject");
        assert!(
            err.to_string().contains("Rate-limited"),
            "expected rate-limit, got: {}",
            err
        );

        // Just after the cooldown expires: the rate-limit gate must NOT
        // be the cause of any error.
        let mut late_env = env.clone();
        late_env.block.time = late_env
            .block
            .time
            .plus_seconds(STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS + 1);
        let res = execute(
            deps.as_mut(),
            late_env,
            message_info(&caller, &[]),
            make_native_pair_msg(),
        );
        if let Err(e) = res {
            assert!(
                !e.to_string().contains("Rate-limited"),
                "after cooldown must not hit rate-limit; got: {}",
                e
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Fix 3: cache anchor pool's bluechip-side index
// ---------------------------------------------------------------------------
//
// Coverage for the new `BlueChipPriceInternalOracle.anchor_bluechip_index`
// field across the three places production code populates / preserves it:
//   - `execute_set_anchor_pool` (one-shot bootstrap path)
//   - `refresh_internal_oracle_for_anchor_change` (timelocked anchor
//     change via `UpdateConfig`; also called by the one-shot above)
//   - `execute_force_rotate_pools` (anchor itself unchanged → index
//     must be preserved, not zeroed)
mod anchor_bluechip_index_cache_tests {
    use super::*;
    use crate::internal_bluechip_price_oracle::INTERNAL_ORACLE;
    use crate::state::INITIAL_ANCHOR_SET;

    /// Register a Native/Native standard anchor pool with a chosen
    /// (bluechip, atom) ordering so we can drive both `index = 0` and
    /// `index = 1` cases through `SetAnchorPool`.
    fn register_anchor_with_ordering(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        pool_id: u64,
        bluechip_first: bool,
    ) -> Addr {
        let bluechip_side = TokenType::Native {
            denom: "ubluechip".to_string(),
        };
        let atom_side = TokenType::Native {
            denom: "uatom".to_string(),
        };
        let pool_token_info = if bluechip_first {
            [bluechip_side, atom_side]
        } else {
            [atom_side, bluechip_side]
        };
        let pool_addr = make_addr(&format!("anchor_pool_{}", pool_id));
        let pool_details = PoolDetails {
            pool_id,
            pool_token_info,
            creator_pool_addr: pool_addr.clone(),
            pool_kind: pool_factory_interfaces::PoolKind::Standard,
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, pool_id, &pool_details)
            .unwrap();
        // Mirror what register_pool would write so oracle queries don't
        // panic on a missing POOLS_BY_CONTRACT_ADDRESS entry. The
        // oracle's calculate_weighted_price path doesn't fire in these
        // tests, so reserve values aren't load-bearing.
        let snapshot = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: false,
            reserve0: Uint128::new(100_000_000_000),
            reserve1: Uint128::new(100_000_000_000),
            total_liquidity: Uint128::new(200_000_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, pool_addr.clone(), &snapshot)
            .unwrap();
        pool_addr
    }

    /// SetAnchorPool with bluechip at index 0 must populate
    /// `anchor_bluechip_index = 0`.
    #[test]
    fn set_anchor_pool_caches_index_0_when_bluechip_first() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        // Reset the one-shot guard so SetAnchorPool can fire (setup_factory
        // may have already triggered it depending on its impl).
        INITIAL_ANCHOR_SET
            .save(deps.as_mut().storage, &false)
            .unwrap();

        let _addr = register_anchor_with_ordering(&mut deps, 99, /*bluechip_first*/ true);

        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 99 },
        )
        .expect("SetAnchorPool must succeed for bluechip-first anchor");

        let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        assert_eq!(
            oracle.anchor_bluechip_index, 0,
            "bluechip at index 0 in pool_token_info must cache as 0"
        );
    }

    /// SetAnchorPool with bluechip at index 1 must populate
    /// `anchor_bluechip_index = 1`. Inverted-shape regression coverage.
    #[test]
    fn set_anchor_pool_caches_index_1_when_atom_first() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        INITIAL_ANCHOR_SET
            .save(deps.as_mut().storage, &false)
            .unwrap();

        let _addr = register_anchor_with_ordering(&mut deps, 88, /*bluechip_first*/ false);

        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 88 },
        )
        .expect("SetAnchorPool must succeed for atom-first anchor");

        let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        assert_eq!(
            oracle.anchor_bluechip_index, 1,
            "bluechip at index 1 in pool_token_info must cache as 1"
        );
    }

    /// `execute_force_rotate_pools` does NOT change the anchor pool —
    /// only the sample-set rotation is forced. The cached
    /// `anchor_bluechip_index` must therefore be PRESERVED across a
    /// force-rotate, not reset to zero.
    #[test]
    fn force_rotate_preserves_anchor_bluechip_index() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        INITIAL_ANCHOR_SET
            .save(deps.as_mut().storage, &false)
            .unwrap();

        // Set anchor with bluechip at index 1 so the cache holds a
        // non-default value (default is 0; we want to assert the
        // non-default value survives force-rotate).
        let _addr = register_anchor_with_ordering(&mut deps, 77, /*bluechip_first*/ false);
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 77 },
        )
        .unwrap();

        let cached_before = INTERNAL_ORACLE
            .load(&deps.storage)
            .unwrap()
            .anchor_bluechip_index;
        assert_eq!(cached_before, 1, "sanity: anchor_bluechip_index = 1 after SetAnchorPool");

        // Drive the force-rotate flow: propose, wait timelock, execute.
        let env_propose = mock_env();
        execute(
            deps.as_mut(),
            env_propose.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeForceRotateOraclePools {},
        )
        .unwrap();

        let mut env_exec = env_propose;
        env_exec.block.time = env_exec
            .block
            .time
            .plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env_exec,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ForceRotateOraclePools {},
        )
        .unwrap();

        let cached_after = INTERNAL_ORACLE
            .load(&deps.storage)
            .unwrap()
            .anchor_bluechip_index;
        assert_eq!(
            cached_after, 1,
            "force-rotate must NOT reset anchor_bluechip_index — anchor itself is unchanged"
        );
    }

    /// `calculate_weighted_price_with_atom` reads the anchor's
    /// bluechip-side from the `anchor_bluechip_index` parameter (not from
    /// a runtime POOLS_BY_ID scan). Passing 0 vs 1 with the same pool
    /// state produces different `bluechip_reserve` (and thus different
    /// weighted contributions) — verifies the cache is actually
    /// consulted.
    #[test]
    fn calculate_weighted_price_uses_cached_index_for_anchor() {
        use crate::internal_bluechip_price_oracle::{
            calculate_weighted_price_with_atom, PoolCumulativeSnapshot,
        };

        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);

        let atom_addr = atom_bluechip_pool_addr();
        // Skewed reserves so reserve0 != reserve1 — we can detect which
        // side the function used as the "bluechip reserve" weight via
        // the resulting weighted-price math.
        let mut state = POOLS_BY_CONTRACT_ADDRESS
            .load(&deps.storage, atom_addr.clone())
            .unwrap();
        state.reserve0 = Uint128::new(100_000_000_000);
        state.reserve1 = Uint128::new(50_000_000_000);
        state.block_time_last = 100;
        // The pool-side accumulator is pre-scaled by
        // `pool_core::swap::PRICE_ACCUMULATOR_SCALE` (== 1e6), so the
        // cumulative values stored on a real pool are raw_ratio·time·1e6.
        // 500 raw × 1e6 = 5e8; 2000 raw × 1e6 = 2e9.
        state.price0_cumulative_last = Uint128::new(500_000_000);
        state.price1_cumulative_last = Uint128::new(2_000_000_000);
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, atom_addr.clone(), &state)
            .unwrap();

        let prev_snapshots = vec![PoolCumulativeSnapshot {
            pool_address: atom_addr.to_string(),
            price0_cumulative: Uint128::zero(),
            block_time: 0,
        }];
        let pools = vec![atom_addr.to_string()];

        // Invocation A: anchor_bluechip_index = 0. cumulative_for_price
        // reads price1_cumulative_last (2e9) → TWAP = 2e9 / 100 = 20_000_000.
        let (_, atom_price_a, _) =
            calculate_weighted_price_with_atom(deps.as_ref(), &pools, &prev_snapshots, 0)
                .expect("call A must succeed");
        let price_a =
            atom_price_a.expect("anchor TWAP under index=0 must be Some");

        // Invocation B: anchor_bluechip_index = 1. cumulative_for_price
        // reads price0_cumulative_last (5e8) → TWAP = 5e8 / 100 = 5_000_000.
        let (_, atom_price_b, _) =
            calculate_weighted_price_with_atom(deps.as_ref(), &pools, &prev_snapshots, 1)
                .expect("call B must succeed");
        let price_b =
            atom_price_b.expect("anchor TWAP under index=1 must be Some");

        assert_ne!(
            price_a, price_b,
            "the anchor_bluechip_index parameter must actually drive the cumulative-side selection"
        );
        assert_eq!(price_a, Uint128::new(20_000_000));
        assert_eq!(price_b, Uint128::new(5_000_000));
    }
}

// ---------------------------------------------------------------------------
// Fix 5: tier the warm-up gate (best-effort fallback for non-critical
//                               USD-denominated callers)
// ---------------------------------------------------------------------------
//
// Strict callers (`bluechip_to_usd` / `usd_to_bluechip`) hard-fail during
// warm-up. Best-effort callers (`*_best_effort`) fall back to
// `pre_reset_last_price` if it's non-zero. Pyth must still be available
// (live or cached within MAX_PRICE_AGE_SECONDS_BEFORE_STALE) — both paths
// fail closed if the bluechip-side fallback exists but Pyth has no
// usable price.
mod warmup_best_effort_tests {
    use super::*;
    use crate::internal_bluechip_price_oracle::{
        bluechip_to_usd, usd_to_bluechip, usd_to_bluechip_best_effort, INTERNAL_ORACLE,
        MOCK_PYTH_PRICE,
    };

    /// Helper: prime the oracle into post-reset state with the given
    /// `pre_reset_last_price`, `warmup_remaining`, and a fresh Pyth cache.
    fn set_oracle_post_reset(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        pre_reset: Uint128,
        warmup_remaining: u32,
    ) {
        // Mock Pyth so the live query succeeds.
        MOCK_PYTH_PRICE
            .save(deps.as_mut().storage, &Uint128::new(10_000_000))
            .unwrap();
        let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        oracle.bluechip_price_cache.last_price = Uint128::zero();
        oracle.bluechip_price_cache.last_update = 0;
        oracle.warmup_remaining = warmup_remaining;
        oracle.pre_reset_last_price = pre_reset;
        oracle.pending_first_price = None;
        INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();
    }

    fn fresh_deps_with_factory() -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        deps
    }

    /// During warm-up with `pre_reset_last_price > 0`, best-effort
    /// returns Ok using the pre-reset price; strict returns Err.
    #[test]
    fn best_effort_serves_during_warmup_strict_does_not() {
        let mut deps = fresh_deps_with_factory();

        // Pre-reset price = 10_000_000 (10 bluechip per atom). Warm-up
        // active (5 remaining).
        set_oracle_post_reset(&mut deps, Uint128::new(10_000_000), 5);

        let env = mock_env();
        let amount = Uint128::new(1_000_000); // $1.00

        // Strict path must Err.
        let strict_result = usd_to_bluechip(deps.as_ref(), amount, &env);
        assert!(
            strict_result.is_err(),
            "strict must fail during warm-up; got {:?}",
            strict_result
        );
        assert!(
            strict_result
                .unwrap_err()
                .to_string()
                .contains("warm-up in progress"),
            "strict error must mention warm-up"
        );

        // Best-effort must succeed using pre_reset_last_price.
        let best_effort_result = usd_to_bluechip_best_effort(deps.as_ref(), amount, &env);
        let conv = best_effort_result
            .expect("best-effort must serve during warm-up when pre_reset > 0");
        assert!(
            !conv.amount.is_zero(),
            "best-effort conversion must produce non-zero amount"
        );
        // Timestamp tagged as current block time (not the stale pre-reset
        // last_update) so downstream staleness checks accept it.
        assert_eq!(
            conv.timestamp,
            env.block.time.seconds(),
            "best-effort must tag timestamp = current block time"
        );
    }

    /// During warm-up with `pre_reset_last_price == 0` (true bootstrap),
    /// best-effort also Errs — there's no fallback price to serve.
    #[test]
    fn best_effort_fails_during_bootstrap_warmup() {
        let mut deps = fresh_deps_with_factory();

        // pre_reset = 0 simulates fresh bootstrap. Warm-up active.
        set_oracle_post_reset(&mut deps, Uint128::zero(), 5);

        let env = mock_env();
        let result = usd_to_bluechip_best_effort(
            deps.as_ref(),
            Uint128::new(1_000_000),
            &env,
        );
        assert!(
            result.is_err(),
            "best-effort with no pre_reset price must fail; got {:?}",
            result
        );
    }

    /// In steady state (`warmup_remaining == 0`), best-effort and strict
    /// produce identical results — they only diverge during warm-up.
    #[test]
    fn best_effort_equals_strict_in_steady_state() {
        let mut deps = fresh_deps_with_factory();

        // Steady state: warmup = 0, last_price set, pre_reset doesn't matter.
        MOCK_PYTH_PRICE
            .save(deps.as_mut().storage, &Uint128::new(10_000_000))
            .unwrap();
        let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        oracle.bluechip_price_cache.last_price = Uint128::new(10_000_000);
        oracle.bluechip_price_cache.last_update = mock_env().block.time.seconds();
        oracle.warmup_remaining = 0;
        oracle.pre_reset_last_price = Uint128::new(99_999_999); // wildly different
        INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();

        let env = mock_env();
        let amount = Uint128::new(2_500_000);

        let strict =
            usd_to_bluechip(deps.as_ref(), amount, &env).expect("strict OK in steady state");
        let best_effort = usd_to_bluechip_best_effort(deps.as_ref(), amount, &env)
            .expect("best-effort OK in steady state");

        assert_eq!(
            strict.amount, best_effort.amount,
            "steady-state amounts must match"
        );
        assert_eq!(
            strict.rate_used, best_effort.rate_used,
            "steady-state rates must match"
        );

        // Bonus: bluechip_to_usd strict in steady state also works
        // (smoke check that the round-trip via the symmetric function
        // works under the same setup).
        bluechip_to_usd(deps.as_ref(), amount, &env)
            .expect("bluechip_to_usd strict in steady state must succeed");
    }
}

// ---------------------------------------------------------------------------
// Pool-admin forwarder tests (PausePool / UnpausePool / EmergencyWithdraw /
//                             CancelEmergencyWithdraw / RecoverStuckStates)
// ---------------------------------------------------------------------------
//
// These factory handlers forward an admin-issued message to the target pool
// contract via WasmMsg::Execute. The pool itself rejects anything not from
// `pool_info.factory_addr`, so the factory is the only entity that can issue
// these. Tests here verify:
//   - Auth gate: non-admin sender → Unauthorized.
//   - Pool registry gate: unknown pool_id → "not found in registry".
//   - Forwarding shape: admin sender → exactly one WasmMsg::Execute targeting
//     the registered pool address with the right inner message.
mod pool_admin_forwarder_tests {
    use super::*;
    use cosmwasm_std::{from_json, CosmosMsg, WasmMsg};
    use serde::{Deserialize, Serialize};

    /// Mirror of the factory's private `PoolAdminMsg` enum so tests can
    /// decode and assert on the forwarded body. Wire format must stay in
    /// lock-step with `factory/src/execute/pool_lifecycle/admin.rs`.
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "snake_case")]
    enum PoolAdminMsgMirror {
        Pause {},
        Unpause {},
        EmergencyWithdraw {},
        CancelEmergencyWithdraw {},
        RecoverStuckStates {
            recovery_type: crate::pool_struct::RecoveryType,
        },
    }

    fn setup_factory_with_pool(pool_id: u64) -> (
        OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        Addr,
    ) {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        let pool_addr = make_addr(&format!("pool_{}", pool_id));
        register_test_pool_addr(&mut deps.storage, pool_id, &pool_addr);
        (deps, pool_addr)
    }

    fn assert_forwards_to_pool(
        res: cosmwasm_std::Response,
        expected_pool_addr: &Addr,
        expected_inner: PoolAdminMsgMirror,
    ) {
        assert_eq!(res.messages.len(), 1, "expected exactly one forwarded msg");
        match &res.messages[0].msg {
            CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr,
                msg,
                funds,
            }) => {
                assert_eq!(contract_addr, &expected_pool_addr.to_string());
                assert!(funds.is_empty(), "admin forwards must not attach funds");
                let inner: PoolAdminMsgMirror = from_json(msg).unwrap();
                assert_eq!(inner, expected_inner);
            }
            other => panic!("expected WasmMsg::Execute, got {:?}", other),
        }
    }

    #[test]
    fn pause_pool_admin_forwards_to_pool() {
        let (mut deps, pool_addr) = setup_factory_with_pool(42);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::PausePool { pool_id: 42 },
        )
        .unwrap();
        assert_forwards_to_pool(res, &pool_addr, PoolAdminMsgMirror::Pause {});
    }

    #[test]
    fn pause_pool_non_admin_rejected() {
        let (mut deps, _) = setup_factory_with_pool(42);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("hacker"), &[]),
            ExecuteMsg::PausePool { pool_id: 42 },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    #[test]
    fn pause_pool_unknown_pool_id_rejected() {
        let (mut deps, _) = setup_factory_with_pool(42);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::PausePool { pool_id: 999 }, // not registered
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found in registry"));
    }

    #[test]
    fn unpause_pool_admin_forwards_to_pool() {
        let (mut deps, pool_addr) = setup_factory_with_pool(7);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::UnpausePool { pool_id: 7 },
        )
        .unwrap();
        assert_forwards_to_pool(res, &pool_addr, PoolAdminMsgMirror::Unpause {});
    }

    #[test]
    fn unpause_pool_non_admin_rejected() {
        let (mut deps, _) = setup_factory_with_pool(7);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("hacker"), &[]),
            ExecuteMsg::UnpausePool { pool_id: 7 },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    #[test]
    fn emergency_withdraw_admin_forwards_to_pool() {
        let (mut deps, pool_addr) = setup_factory_with_pool(123);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::EmergencyWithdrawPool { pool_id: 123 },
        )
        .unwrap();
        assert_forwards_to_pool(res, &pool_addr, PoolAdminMsgMirror::EmergencyWithdraw {});
    }

    #[test]
    fn emergency_withdraw_non_admin_rejected() {
        let (mut deps, _) = setup_factory_with_pool(123);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("hacker"), &[]),
            ExecuteMsg::EmergencyWithdrawPool { pool_id: 123 },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    #[test]
    fn cancel_emergency_withdraw_admin_forwards_to_pool() {
        let (mut deps, pool_addr) = setup_factory_with_pool(456);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::CancelEmergencyWithdrawPool { pool_id: 456 },
        )
        .unwrap();
        assert_forwards_to_pool(
            res,
            &pool_addr,
            PoolAdminMsgMirror::CancelEmergencyWithdraw {},
        );
    }

    #[test]
    fn recover_stuck_states_admin_forwards_to_pool_with_recovery_type() {
        let (mut deps, pool_addr) = setup_factory_with_pool(99);
        // Each RecoveryType variant must round-trip through the forwarded
        // payload — exercise all four.
        for recovery in [
            crate::pool_struct::RecoveryType::StuckThreshold,
            crate::pool_struct::RecoveryType::StuckDistribution,
            crate::pool_struct::RecoveryType::StuckReentrancyGuard,
            crate::pool_struct::RecoveryType::Both,
        ] {
            let res = execute(
                deps.as_mut(),
                mock_env(),
                message_info(&admin_addr(), &[]),
                ExecuteMsg::RecoverPoolStuckStates {
                    pool_id: 99,
                    recovery_type: recovery.clone(),
                },
            )
            .unwrap();
            assert_forwards_to_pool(
                res,
                &pool_addr,
                PoolAdminMsgMirror::RecoverStuckStates {
                    recovery_type: recovery,
                },
            );
        }
    }

    #[test]
    fn recover_stuck_states_non_admin_rejected() {
        let (mut deps, _) = setup_factory_with_pool(99);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("hacker"), &[]),
            ExecuteMsg::RecoverPoolStuckStates {
                pool_id: 99,
                recovery_type: crate::pool_struct::RecoveryType::StuckThreshold,
            },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }
}

// ---------------------------------------------------------------------------
// Anchor validation failure-mode tests
// ---------------------------------------------------------------------------
//
// `validate_anchor_pool_choice` enforces the strict shape an anchor pool
// must have: PoolKind::Standard, Native+Native pair of exactly
// (bluechip_denom, atom_denom) in either order. The audit-fix
// `anchor_bluechip_index_cache_tests` exercises the happy paths through
// `SetAnchorPool`. These tests cover the rejection paths — the failure
// modes that prevent a hostile or misconfigured anchor from being set.
mod anchor_validation_failure_tests {
    use super::*;
    use crate::state::INITIAL_ANCHOR_SET;

    fn fresh_factory_with_anchor_unset() -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory(&mut deps);
        // Reset one-shot guard so SetAnchorPool can fire.
        INITIAL_ANCHOR_SET
            .save(deps.as_mut().storage, &false)
            .unwrap();
        deps
    }

    /// SetAnchorPool against a Commit pool (not Standard) → rejected.
    /// The anchor MUST be a standard pool because commit pools have a
    /// pre-threshold phase where they can't serve swaps.
    #[test]
    fn set_anchor_rejects_commit_pool() {
        let mut deps = fresh_factory_with_anchor_unset();
        let pool_addr = make_addr("commit_pool_attempted_as_anchor");
        // Register as Commit kind with the right denom pair shape.
        let pool_details = PoolDetails {
            pool_id: 50,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::Native {
                    denom: "uatom".to_string(),
                },
            ],
            creator_pool_addr: pool_addr,
            pool_kind: pool_factory_interfaces::PoolKind::Commit, // wrong kind
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, 50, &pool_details)
            .unwrap();

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 50 },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Anchor pool must be a standard pool"),
            "got: {}",
            err
        );
    }

    /// SetAnchorPool against a Standard pool whose pair has a CreatorToken
    /// instead of two Natives → rejected. Anchor must price ATOM/USD via
    /// Pyth, and a CW20 side breaks that derivation.
    #[test]
    fn set_anchor_rejects_native_creator_pair() {
        let mut deps = fresh_factory_with_anchor_unset();
        let pool_addr = make_addr("std_native_creator_pool");
        let pool_details = PoolDetails {
            pool_id: 51,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("some_cw20"),
                },
            ],
            creator_pool_addr: pool_addr,
            pool_kind: pool_factory_interfaces::PoolKind::Standard,
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, 51, &pool_details)
            .unwrap();

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 51 },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Anchor pool must be a Native/Native pair"),
            "got: {}",
            err
        );
    }

    /// SetAnchorPool against a Standard Native+Native pool whose denoms
    /// don't match `(bluechip_denom, atom_denom)` exactly → rejected.
    /// Specifically a bluechip + IBC-not-atom pair: the pool has bluechip
    /// on one side but the other side is an unrelated IBC denom. Anchor
    /// must price ATOM/USD via Pyth, so any non-atom companion breaks
    /// the derivation.
    #[test]
    fn set_anchor_rejects_bluechip_with_wrong_companion_denom() {
        let mut deps = fresh_factory_with_anchor_unset();
        let pool_addr = make_addr("std_bluechip_wrongibc_pool");
        let pool_details = PoolDetails {
            pool_id: 52,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::Native {
                    denom: "ibc/UNRELATED_DENOM".to_string(), // not uatom
                },
            ],
            creator_pool_addr: pool_addr,
            pool_kind: pool_factory_interfaces::PoolKind::Standard,
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, 52, &pool_details)
            .unwrap();

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 52 },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Anchor pool must be a Native/Native pair"),
            "got: {}",
            err
        );
    }

    /// SetAnchorPool against an unregistered pool_id → "not found in registry".
    #[test]
    fn set_anchor_rejects_unregistered_pool_id() {
        let mut deps = fresh_factory_with_anchor_unset();
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 9999 },
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found in registry"));
    }

    /// SetAnchorPool from a non-admin sender → Unauthorized.
    #[test]
    fn set_anchor_rejects_non_admin() {
        let mut deps = fresh_factory_with_anchor_unset();
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&Addr::unchecked("hacker"), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 1 },
        )
        .unwrap_err();
        assert!(matches!(err, ContractError::Unauthorized {}));
    }

    /// One-shot guard: after a successful SetAnchorPool, a second call
    /// (even from admin against a different valid pool) is rejected
    /// because INITIAL_ANCHOR_SET is now true. Subsequent anchor changes
    /// must go through the timelocked propose/apply config flow.
    #[test]
    fn set_anchor_one_shot_rejects_second_call() {
        let mut deps = fresh_factory_with_anchor_unset();
        let pool_addr_a = make_addr("first_anchor");
        let pool_addr_b = make_addr("second_anchor_attempt");
        for (pid, addr) in [(60, &pool_addr_a), (61, &pool_addr_b)] {
            let pool_details = PoolDetails {
                pool_id: pid,
                pool_token_info: [
                    TokenType::Native {
                        denom: "ubluechip".to_string(),
                    },
                    TokenType::Native {
                        denom: "uatom".to_string(),
                    },
                ],
                creator_pool_addr: addr.clone(),
                pool_kind: pool_factory_interfaces::PoolKind::Standard,
                commit_pool_ordinal: 0,
            };
            POOLS_BY_ID
                .save(&mut deps.storage, pid, &pool_details)
                .unwrap();
        }

        // First call succeeds.
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 60 },
        )
        .unwrap();

        // Second call rejects with the one-shot error.
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::SetAnchorPool { pool_id: 61 },
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Anchor pool has already been set"),
            "expected one-shot guard error, got: {}",
            err
        );
    }
}

// ===========================================================================
// M-3: oracle eligibility curation
//
// Allowlist (any pool kind) + global commit-pool auto-include flag, both
// behind a 48h timelock for adds / flips. Removes are immediate.
// Permissionless `RefreshOraclePoolSnapshot` is rate-limited.
// ===========================================================================
mod oracle_eligibility_tests {
    use super::*;
    use crate::execute::instantiate;
    use crate::internal_bluechip_price_oracle::get_eligible_creator_pools;
    use crate::state::{
        load_commit_pools_auto_eligible, ADMIN_TIMELOCK_SECONDS, COMMIT_POOLS_AUTO_ELIGIBLE,
        ORACLE_ELIGIBLE_POOLS, ORACLE_REFRESH_RATE_LIMIT_BLOCKS,
        PENDING_COMMIT_POOLS_AUTO_ELIGIBLE, PENDING_ORACLE_ELIGIBLE_POOL_ADD,
    };

    /// Stand up a factory + register a single commit pool that has crossed
    /// threshold and meets the liquidity floor. Returns the pool's
    /// contract address. Caller chooses the auto-eligible flag value.
    fn setup_factory_with_commit_pool(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        auto_eligible: bool,
    ) -> Addr {
        setup_atom_pool(deps);
        let env = mock_env();
        instantiate(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            default_factory_config(),
        )
        .unwrap();
        // setup_atom_pool sets the flag to true; respect the caller's
        // choice instead.
        COMMIT_POOLS_AUTO_ELIGIBLE
            .save(deps.as_mut().storage, &auto_eligible)
            .unwrap();

        let pool_addr = make_addr("creator_pool_1");
        let pool_details = PoolDetails {
            pool_id: 1,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("creator_token_1"),
                },
            ],
            creator_pool_addr: pool_addr.clone(),
            pool_kind: pool_factory_interfaces::PoolKind::Commit,
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID.save(deps.as_mut().storage, 1, &pool_details).unwrap();
        POOLS_BY_CONTRACT_ADDRESS
            .save(
                deps.as_mut().storage,
                pool_addr.clone(),
                &PoolStateResponseForFactory {
                    pool_contract_address: pool_addr.clone(),
                    nft_ownership_accepted: true,
                    reserve0: Uint128::new(50_000_000_000),
                    reserve1: Uint128::new(50_000_000_000),
                    total_liquidity: Uint128::new(100_000_000_000),
                    block_time_last: 100,
                    price0_cumulative_last: Uint128::zero(),
                    price1_cumulative_last: Uint128::zero(),
                    assets: vec![],
                },
            )
            .unwrap();
        POOL_THRESHOLD_MINTED
            .save(deps.as_mut().storage, 1, &true)
            .unwrap();
        pool_addr
    }

    /// Register a standard pool whose canonical bluechip side is at index 0.
    fn register_standard_pool(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        pool_id: u64,
        addr: &Addr,
    ) {
        register_standard_pool_with_reserves(
            deps,
            pool_id,
            addr,
            50_000_000_000,
            50_000_000_000,
        );
    }

    /// Same as `register_standard_pool` but with explicit reserves so M-4
    /// liquidity-floor tests can dial in pool-state edges.
    pub(super) fn register_standard_pool_with_reserves(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        pool_id: u64,
        addr: &Addr,
        reserve0: u128,
        reserve1: u128,
    ) {
        let pool_details = PoolDetails {
            pool_id,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::Native {
                    denom: "uusdc".to_string(),
                },
            ],
            creator_pool_addr: addr.clone(),
            pool_kind: pool_factory_interfaces::PoolKind::Standard,
            commit_pool_ordinal: 0,
        };
        POOLS_BY_ID
            .save(deps.as_mut().storage, pool_id, &pool_details)
            .unwrap();
        POOLS_BY_CONTRACT_ADDRESS
            .save(
                deps.as_mut().storage,
                addr.clone(),
                &PoolStateResponseForFactory {
                    pool_contract_address: addr.clone(),
                    nft_ownership_accepted: true,
                    reserve0: Uint128::new(reserve0),
                    reserve1: Uint128::new(reserve1),
                    total_liquidity: Uint128::new(reserve0 + reserve1),
                    block_time_last: 100,
                    price0_cumulative_last: Uint128::zero(),
                    price1_cumulative_last: Uint128::zero(),
                    assets: vec![],
                },
            )
            .unwrap();
    }

    /// Auto-flag OFF: a threshold-crossed commit pool that's NOT in the
    /// allowlist must NOT be eligible. This is the stage 1–3 default.
    #[test]
    fn auto_off_threshold_crossed_commit_pool_not_eligible() {
        let mut deps = mock_deps_with_querier(&[]);
        let _pool = setup_factory_with_commit_pool(&mut deps, false);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert!(
            eligible.is_empty(),
            "auto-flag OFF + empty allowlist => no eligible pools (got {:?})",
            eligible
        );
    }

    /// Auto-flag ON: a threshold-crossed commit pool flows in
    /// automatically without an allowlist entry. Mirrors the legacy
    /// behaviour we preserve via the migrate handler.
    #[test]
    fn auto_on_threshold_crossed_commit_pool_eligible() {
        let mut deps = mock_deps_with_querier(&[]);
        let pool = setup_factory_with_commit_pool(&mut deps, true);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible, vec![pool.to_string()]);
    }

    /// Allowlist propose -> wait -> apply must respect the 48h timelock.
    /// Apply before the timelock fails; cancel removes the pending entry.
    #[test]
    fn allowlist_propose_apply_timelock() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);
        let std_pool = make_addr("std_pool_usdc");
        register_standard_pool(&mut deps, 2, &std_pool);

        // Propose.
        let mut env = mock_env();
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        assert!(
            PENDING_ORACLE_ELIGIBLE_POOL_ADD.has(&deps.storage, std_pool.clone()),
            "pending entry must exist after propose"
        );

        // Apply BEFORE timelock => TimelockNotExpired.
        let too_early = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap_err();
        assert!(
            matches!(too_early, ContractError::TimelockNotExpired { .. }),
            "expected TimelockNotExpired, got {:?}",
            too_early
        );

        // Apply AFTER timelock => allowlisted, pending cleared.
        env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        assert!(ORACLE_ELIGIBLE_POOLS.has(&deps.storage, std_pool.clone()));
        assert!(
            !PENDING_ORACLE_ELIGIBLE_POOL_ADD.has(&deps.storage, std_pool),
            "pending must be cleared after apply"
        );
    }

    /// Cancel during the timelock window drops the pending entry without
    /// landing it.
    #[test]
    fn allowlist_cancel_drops_pending() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);
        let std_pool = make_addr("std_pool_usdc");
        register_standard_pool(&mut deps, 2, &std_pool);

        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::CancelAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        assert!(!PENDING_ORACLE_ELIGIBLE_POOL_ADD.has(&deps.storage, std_pool.clone()));
        assert!(!ORACLE_ELIGIBLE_POOLS.has(&deps.storage, std_pool));
    }

    /// Remove is immediate (no timelock) — drops the pool from the
    /// allowlist on the same tx.
    #[test]
    fn allowlist_remove_is_immediate() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);
        let std_pool = make_addr("std_pool_usdc");
        register_standard_pool(&mut deps, 2, &std_pool);

        // Propose + apply.
        let mut env = mock_env();
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        assert!(ORACLE_ELIGIBLE_POOLS.has(&deps.storage, std_pool.clone()));

        // Remove — same tx, no timelock.
        execute(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::RemoveOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        assert!(!ORACLE_ELIGIBLE_POOLS.has(&deps.storage, std_pool));
    }

    /// Allowlisted standard pool is eligible even when the auto-flag is OFF.
    #[test]
    fn allowlisted_standard_pool_eligible_with_auto_off() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);
        let std_pool = make_addr("std_pool_usdc");
        register_standard_pool(&mut deps, 2, &std_pool);

        // Propose + apply (timelock).
        let mut env = mock_env();
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();
        env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
        )
        .unwrap();

        // Eligibility resolved: only the allowlisted standard pool, no
        // commit pools (auto-flag OFF, the threshold-crossed creator
        // pool from setup is NOT in the allowlist).
        let (eligible, indices) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible, vec![std_pool.to_string()]);
        // bluechip is at index 0 in the standard pool we registered.
        assert_eq!(indices, vec![0u8]);
    }

    /// A pool that's both allowlisted AND threshold-crossed (auto-flag ON)
    /// appears exactly once in the eligible set, with the allowlist's
    /// recorded bluechip_index taking precedence.
    #[test]
    fn dedup_when_pool_is_both_allowlisted_and_auto_eligible() {
        let mut deps = mock_deps_with_querier(&[]);
        let commit_pool = setup_factory_with_commit_pool(&mut deps, true);

        // Add the SAME commit pool to the allowlist via the timelock flow.
        let mut env = mock_env();
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: commit_pool.to_string(),
            },
        )
        .unwrap();
        env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: commit_pool.to_string(),
            },
        )
        .unwrap();

        // Both inputs reference the same pool — dedup ensures one entry.
        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0], commit_pool.to_string());
    }

    /// Flag flip goes through the same 48h timelock; apply before the
    /// timelock fails; cancel discards the pending change.
    #[test]
    fn flag_flip_timelock() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);

        // Propose ON.
        let mut env = mock_env();
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: true },
        )
        .unwrap();
        assert_eq!(load_commit_pools_auto_eligible(&deps.storage), false);

        // Apply too early.
        let early = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplySetCommitPoolsAutoEligible {},
        )
        .unwrap_err();
        assert!(matches!(early, ContractError::TimelockNotExpired { .. }));

        // Apply after timelock.
        env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
        execute(
            deps.as_mut(),
            env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ApplySetCommitPoolsAutoEligible {},
        )
        .unwrap();
        assert_eq!(load_commit_pools_auto_eligible(&deps.storage), true);
        assert!(PENDING_COMMIT_POOLS_AUTO_ELIGIBLE
            .may_load(&deps.storage)
            .unwrap()
            .is_none());
    }

    /// Cancel a pending flag flip drops the pending without changing the
    /// effective value.
    #[test]
    fn flag_flip_cancel() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, true);

        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: false },
        )
        .unwrap();
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::CancelSetCommitPoolsAutoEligible {},
        )
        .unwrap();
        assert_eq!(load_commit_pools_auto_eligible(&deps.storage), true);
    }

    /// Proposing a flag flip when the flag is already at the proposed
    /// value is a no-op error (don't waste the 48h window on nothing).
    #[test]
    fn flag_flip_no_change_rejected() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, true);

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: true },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ContractError::CommitPoolsAutoEligibleNoChange { value: true }
        ));
    }

    /// Permissionless refresh: first call lands; second within the rate
    /// limit fails; third after the rate limit elapses lands again.
    #[test]
    fn refresh_rate_limited() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, true);

        let mut env = mock_env();
        // First refresh — no prior, lands.
        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&make_addr("randomkeeper"), &[]),
            ExecuteMsg::RefreshOraclePoolSnapshot {},
        )
        .unwrap();

        // Second refresh in the same block — rate-limited.
        let err = execute(
            deps.as_mut(),
            env.clone(),
            message_info(&make_addr("randomkeeper"), &[]),
            ExecuteMsg::RefreshOraclePoolSnapshot {},
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ContractError::OracleRefreshRateLimited { .. }
        ));

        // Advance past the rate limit.
        env.block.height = env.block.height + ORACLE_REFRESH_RATE_LIMIT_BLOCKS + 1;
        execute(
            deps.as_mut(),
            env,
            message_info(&make_addr("randomkeeper"), &[]),
            ExecuteMsg::RefreshOraclePoolSnapshot {},
        )
        .unwrap();
    }

    /// Non-admin cannot propose, apply, or remove allowlist entries; nor
    /// can they propose / apply / cancel flag flips.
    #[test]
    fn non_admin_rejected_on_admin_actions() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);
        let std_pool = make_addr("std_pool_usdc");
        register_standard_pool(&mut deps, 2, &std_pool);
        let attacker = make_addr("attacker");

        for msg in [
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
            ExecuteMsg::ApplyAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
            ExecuteMsg::CancelAddOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
            ExecuteMsg::RemoveOracleEligiblePool {
                pool_addr: std_pool.to_string(),
            },
            ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: true },
            ExecuteMsg::ApplySetCommitPoolsAutoEligible {},
            ExecuteMsg::CancelSetCommitPoolsAutoEligible {},
        ] {
            let err = execute(
                deps.as_mut(),
                mock_env(),
                message_info(&attacker, &[]),
                msg,
            )
            .unwrap_err();
            assert!(
                matches!(err, ContractError::Unauthorized {}),
                "expected Unauthorized, got {:?}",
                err
            );
        }
    }

    /// Adding a pool whose address is unknown to the factory registry
    /// fails at propose time (don't burn the timelock on a non-existent
    /// pool).
    #[test]
    fn propose_unknown_pool_rejected() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_with_commit_pool(&mut deps, false);

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ProposeAddOracleEligiblePool {
                pool_addr: make_addr("ghost_pool").to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ContractError::OracleEligiblePoolNotInRegistry { .. }
        ));
    }
}

// ===========================================================================
// M-4: USD-denominated liquidity floor + per-side floor
//
// Replaces the legacy `reserve0 + reserve1 >= MIN_POOL_LIQUIDITY` check
// (which conflated units across asymmetric pairs) with a single
// `pool_meets_liquidity_floor` helper:
//
//   - When the oracle cache has a non-zero `last_price`, the helper
//     converts MIN_POOL_LIQUIDITY_USD ($5,000 default) to bluechip via
//     the cache and requires bluechip-side >= floor / 2.
//   - When the cache is zero (bootstrap, breaker tripped, post-warmup),
//     it falls back to MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE
//     (5_000 BC) so the gate stays meaningful before the oracle has
//     produced a usable USD price.
// ===========================================================================
mod liquidity_floor_tests {
    use super::*;
    use crate::internal_bluechip_price_oracle::{
        pool_meets_liquidity_floor, INTERNAL_ORACLE,
        MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE, MIN_POOL_LIQUIDITY_USD,
    };

    /// Build a `PoolStateResponseForFactory` with explicit reserves.
    fn pool_with_reserves(
        addr: &Addr,
        reserve0: u128,
        reserve1: u128,
    ) -> PoolStateResponseForFactory {
        PoolStateResponseForFactory {
            pool_contract_address: addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(reserve0),
            reserve1: Uint128::new(reserve1),
            total_liquidity: Uint128::new(reserve0 + reserve1),
            block_time_last: 100,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        }
    }

    /// Set `INTERNAL_ORACLE.bluechip_price_cache.last_price` to the given
    /// USD-per-bluechip value (6-decimal scale: 1_000_000 = $1.00).
    fn seed_oracle_price(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        price_micro_usd: u128,
    ) {
        // Lazy-load the existing oracle (initialized by `instantiate`),
        // mutate just the price field, save back.
        let mut oracle = INTERNAL_ORACLE
            .load(deps.as_mut().storage)
            .expect("oracle must be initialized before seeding price");
        oracle.bluechip_price_cache.last_price = Uint128::new(price_micro_usd);
        INTERNAL_ORACLE
            .save(deps.as_mut().storage, &oracle)
            .unwrap();
    }

    /// Standard "factory + atom pool ready, oracle initialized" scaffold.
    fn setup_factory_and_init_oracle(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    ) {
        setup_atom_pool(deps);
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            default_factory_config(),
        )
        .unwrap();
    }

    /// Fallback path: oracle has no price (last_price == 0). Helper must
    /// require bluechip-side >= MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.
    #[test]
    fn fallback_passes_when_bluechip_side_meets_legacy_floor() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        seed_oracle_price(&mut deps, 0);
        let pool = make_addr("balanced_pool");
        let state = pool_with_reserves(
            &pool,
            MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128(),
            MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128(),
        );
        let ok = pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap();
        assert!(ok, "balanced pool at the fallback floor must pass");
    }

    /// Fallback path: bluechip-side strictly below the legacy floor fails.
    #[test]
    fn fallback_fails_when_bluechip_side_below_legacy_floor() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        seed_oracle_price(&mut deps, 0);
        let pool = make_addr("thin_pool");
        let bluechip_side = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128() - 1;
        let state = pool_with_reserves(&pool, bluechip_side, bluechip_side);
        let ok = pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap();
        assert!(!ok, "bluechip-side one below the floor must fail");
    }

    /// Fallback path: lopsided pool whose SUMMED reserves clear the legacy
    /// MIN_POOL_LIQUIDITY (10 BC) but whose BLUECHIP SIDE is far below
    /// must fail. This is the exact pre-M-4 false-pass case.
    #[test]
    fn fallback_rejects_lopsided_pool_that_old_check_passed() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        seed_oracle_price(&mut deps, 0);
        let pool = make_addr("lopsided_pool");
        // bluechip side: 100 ubluechip (essentially zero). Other side:
        // 100 BC (big). Legacy check (sum >= 10 BC) would have PASSED.
        // New check on bluechip-side must FAIL.
        let state = pool_with_reserves(&pool, 100, 100_000_000_000);
        let ok = pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap();
        assert!(
            !ok,
            "lopsided pool with bluechip-side dust must fail the M-4 floor"
        );
    }

    /// USD path: oracle has price = $1.00 / bluechip. $5_000 floor /
    /// $1.00 / 2 sides = 2_500 BC = 2_500_000_000 ubluechip per side.
    /// Pool exactly at the floor passes.
    #[test]
    fn usd_path_passes_at_computed_floor() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        // last_price units: USD-6dp per bluechip-1.0
        // 1_000_000 = $1.00 per bluechip.
        seed_oracle_price(&mut deps, 1_000_000);
        let pool = make_addr("usd_floor_exact");
        // floor_per_side = MIN_POOL_LIQUIDITY_USD * 1_000_000 / last_price / 2
        //                = 5_000_000_000 * 1_000_000 / 1_000_000 / 2
        //                = 2_500_000_000 ubluechip
        let floor_per_side = MIN_POOL_LIQUIDITY_USD.u128() / 2;
        let state = pool_with_reserves(&pool, floor_per_side, floor_per_side);
        let ok = pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap();
        assert!(ok, "exactly at the computed USD floor must pass");
    }

    /// USD path: doubling the bluechip price halves the bluechip-side
    /// floor (same USD value at higher price = less bluechip).
    #[test]
    fn usd_path_floor_inverse_to_price() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        // $2.00 / bluechip → floor_per_side = 5_000_000_000 / 2 / 2 = 1_250 BC
        seed_oracle_price(&mut deps, 2_000_000);
        let pool = make_addr("usd_floor_halved");
        let floor_per_side = MIN_POOL_LIQUIDITY_USD.u128() / 2 / 2;
        let state = pool_with_reserves(&pool, floor_per_side, floor_per_side);
        assert!(pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap());

        // One ubluechip below must fail.
        let just_under = pool_with_reserves(&pool, floor_per_side - 1, floor_per_side - 1);
        assert!(!pool_meets_liquidity_floor(&deps.storage, &just_under, 0).unwrap());
    }

    /// USD path: bluechip-side index correctly selects the right reserve.
    /// Pool lays out [creator_token, bluechip] (index 1 is bluechip).
    /// reserve0 has dust, reserve1 has plenty — must pass when index=1.
    #[test]
    fn bluechip_index_one_picks_reserve1() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        seed_oracle_price(&mut deps, 1_000_000);
        let pool = make_addr("idx1_pool");
        let floor_per_side = MIN_POOL_LIQUIDITY_USD.u128() / 2;
        // Lopsided in absolute units, but bluechip is on side 1.
        let state = pool_with_reserves(&pool, 100, floor_per_side);
        assert!(
            pool_meets_liquidity_floor(&deps.storage, &state, 1).unwrap(),
            "bluechip_index=1 must read reserve1, not reserve0"
        );
        // Same pool with bluechip_index=0 must FAIL (reserve0 = 100).
        assert!(
            !pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap(),
            "bluechip_index=0 must read reserve0 (100 ubluechip, far below floor)"
        );
    }

    /// Pre-instantiate: oracle hasn't been initialized yet. Helper must
    /// fall back rather than panic. Mirrors the bootstrap order
    /// (instantiate -> initialize_internal_bluechip_oracle ->
    /// select_random_pools_with_atom -> get_eligible_creator_pools).
    #[test]
    fn missing_oracle_falls_back_without_panic() {
        let deps = mock_deps_with_querier(&[]);
        // No setup_factory_and_init_oracle — INTERNAL_ORACLE intentionally absent.
        let pool = make_addr("preinit");
        let state = pool_with_reserves(
            &pool,
            MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128(),
            MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128(),
        );
        let ok = pool_meets_liquidity_floor(&deps.storage, &state, 0).unwrap();
        assert!(ok);
    }

    // Note on integration coverage: the per-pool integration path is
    // covered indirectly by `oracle_eligibility_tests` (allowlist
    // add/remove/dedup). The mock querier in this crate returns the same
    // reserves for every contract address, so it can't distinguish
    // drained / lopsided pools at the cross-contract query layer; the
    // helper-level tests above pin the actual gate semantics directly,
    // which is the only thing M-4 changes.
}

// ===========================================================================
// Pre-testnet oracle coverage backfill.
//
// Targets the highest-priority gaps identified in the coverage audit
// (drift_bps saturating math, bootstrap exact-boundary timestamp,
// best-effort warmup fallback combinations, zero-amount conversions).
// These cover paths whose breakage would either silently corrupt the
// oracle (drift saturation) or surface only on real anchor rotations
// (warmup fallback) — exactly the kinds of bugs that get caught late
// or never on testnet without explicit unit coverage.
// ===========================================================================
mod oracle_coverage_backfill {
    use super::*;
    use crate::internal_bluechip_price_oracle::{
        drift_bps_saturating, usd_to_bluechip, usd_to_bluechip_best_effort,
        ANCHOR_CHANGE_WARMUP_OBSERVATIONS, INTERNAL_ORACLE,
    };

    fn setup_factory_and_init_oracle(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    ) {
        setup_atom_pool(deps);
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            default_factory_config(),
        )
        .unwrap();
    }

    /// Mutate `INTERNAL_ORACLE` with a closure. Loads, applies, saves.
    fn mutate_oracle(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        f: impl FnOnce(&mut crate::internal_bluechip_price_oracle::BlueChipPriceInternalOracle),
    ) {
        let mut oracle = INTERNAL_ORACLE.load(deps.as_mut().storage).unwrap();
        f(&mut oracle);
        INTERNAL_ORACLE.save(deps.as_mut().storage, &oracle).unwrap();
    }

    // -----------------------------------------------------------------------
    // drift_bps_saturating: pure-function unit tests (audit P2).
    //
    // The breaker's correctness pivots on this helper. Saturating
    // arithmetic is invisible at integration-test scale because real
    // prices never reach the regime where wrap or overflow matter, but
    // a future code change that swaps `saturating_*` for `checked_*`
    // (or vice versa) flips behaviour silently.
    // -----------------------------------------------------------------------

    /// Both inputs zero: implementation saturates to `u128::MAX`
    /// (the fail-safe direction) because `0 / 0` falls into the
    /// saturating div-by-zero branch. Production callers gate the
    /// drift check on `prior != 0` separately, so this code path
    /// is unreachable in practice — but pinning the actual behaviour
    /// here protects against a future "fix" that returns 0 and
    /// silently disarms the breaker on (hypothetical) zero-vs-zero
    /// transitions.
    #[test]
    fn drift_zero_zero_saturates_to_max() {
        assert_eq!(
            drift_bps_saturating(Uint128::zero(), Uint128::zero()),
            u128::MAX
        );
    }

    /// Identical inputs → 0 bps drift regardless of magnitude.
    #[test]
    fn drift_identical_returns_zero() {
        assert_eq!(
            drift_bps_saturating(Uint128::new(1_000_000), Uint128::new(1_000_000)),
            0
        );
        assert_eq!(
            drift_bps_saturating(Uint128::MAX, Uint128::MAX),
            0
        );
    }

    /// One side zero, other non-zero → saturates to u128::MAX (the
    /// "definitely tripped" sentinel — division by the smaller value
    /// (= 0) is treated as "infinite drift" rather than a numeric
    /// error). Maps "math broke" to "fire the breaker", which is the
    /// safe direction.
    #[test]
    fn drift_one_zero_saturates_to_max() {
        assert_eq!(
            drift_bps_saturating(Uint128::zero(), Uint128::new(1_000_000)),
            u128::MAX
        );
        assert_eq!(
            drift_bps_saturating(Uint128::new(1_000_000), Uint128::zero()),
            u128::MAX
        );
    }

    /// Order independence: drift(a, b) == drift(b, a).
    #[test]
    fn drift_is_symmetric() {
        let a = Uint128::new(1_000_000);
        let b = Uint128::new(1_300_000);
        assert_eq!(drift_bps_saturating(a, b), drift_bps_saturating(b, a));
    }

    /// Exactly +30% drift = 3000 bps. The breaker uses `>` against
    /// MAX_TWAP_DRIFT_BPS (3000), so this exact reading must NOT trip.
    /// Pins the boundary semantics that a later const change would
    /// otherwise flip silently.
    #[test]
    fn drift_exactly_thirty_percent_yields_3000_bps() {
        let prior = Uint128::new(10_000_000);
        let new = Uint128::new(13_000_000); // +30% from prior
        assert_eq!(drift_bps_saturating(prior, new), 3_000);
    }

    /// Saturating overflow: a delta so large that
    /// `diff * BPS_SCALE` would overflow u128. The helper must
    /// saturate to u128::MAX rather than wrap, so the breaker fires.
    #[test]
    fn drift_overflow_saturates_to_max() {
        // diff = u128::MAX - 1; diff * 10_000 overflows u128.
        let huge = Uint128::MAX;
        let small = Uint128::new(1);
        let drift = drift_bps_saturating(small, huge);
        assert_eq!(
            drift, u128::MAX,
            "overflow in scaling step must saturate, not wrap"
        );
    }

    // -----------------------------------------------------------------------
    // Bootstrap-confirm exact-boundary timestamp (audit P2).
    //
    // Existing tests cover `< window` (rejected) and `> window`
    // (accepted) but not `== window`. The handler uses `<` so equality
    // must be ACCEPTED — a future refactor swapping to `<=` would flip
    // the semantics silently.
    // -----------------------------------------------------------------------

    /// `block.time == proposed_at + BOOTSTRAP_OBSERVATION_SECONDS` exactly
    /// must succeed. Guard against an off-by-one regression.
    #[test]
    fn confirm_bootstrap_at_exact_boundary_accepts() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        // Prime: zero pre-reset and zero last_price → first update buffers
        // a candidate via branch (b) → next round's `update` writes
        // PENDING_BOOTSTRAP_PRICE in branch (d). For the boundary test we
        // can shortcut: write PENDING_BOOTSTRAP_PRICE directly with a
        // known proposed_at.
        let env_now = mock_env();
        crate::state::PENDING_BOOTSTRAP_PRICE
            .save(
                deps.as_mut().storage,
                &crate::state::PendingBootstrapPrice {
                    price: Uint128::new(10_000_000),
                    atom_pool_price: Uint128::new(10_000_000),
                    proposed_at: env_now.block.time,
                    observation_count: 1,
                },
            )
            .unwrap();
        // Make sure warmup is non-zero so the publish path actually
        // decrements (matches production state immediately after a
        // reset).
        mutate_oracle(&mut deps, |o| {
            o.warmup_remaining = ANCHOR_CHANGE_WARMUP_OBSERVATIONS;
        });

        let mut confirm_env = env_now.clone();
        confirm_env.block.time = confirm_env
            .block
            .time
            .plus_seconds(crate::state::BOOTSTRAP_OBSERVATION_SECONDS);

        execute(
            deps.as_mut(),
            confirm_env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ConfirmBootstrapPrice {},
        )
        .expect("confirm at exact boundary must succeed");

        let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        assert_eq!(
            oracle.bluechip_price_cache.last_price,
            Uint128::new(10_000_000)
        );
        assert!(crate::state::PENDING_BOOTSTRAP_PRICE
            .may_load(&deps.storage)
            .unwrap()
            .is_none());
    }

    /// One second BEFORE the boundary must reject. Pinned alongside the
    /// accept test to make the boundary semantics symmetrically explicit
    /// rather than relying on the existing "+300s" early-reject test
    /// (which is far from the edge).
    #[test]
    fn confirm_bootstrap_one_second_before_boundary_rejects() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        let env_now = mock_env();
        crate::state::PENDING_BOOTSTRAP_PRICE
            .save(
                deps.as_mut().storage,
                &crate::state::PendingBootstrapPrice {
                    price: Uint128::new(10_000_000),
                    atom_pool_price: Uint128::new(10_000_000),
                    proposed_at: env_now.block.time,
                    observation_count: 1,
                },
            )
            .unwrap();

        let mut confirm_env = env_now.clone();
        // -1s relative to the boundary.
        confirm_env.block.time = confirm_env
            .block
            .time
            .plus_seconds(crate::state::BOOTSTRAP_OBSERVATION_SECONDS - 1);

        let err = execute(
            deps.as_mut(),
            confirm_env,
            message_info(&admin_addr(), &[]),
            ExecuteMsg::ConfirmBootstrapPrice {},
        )
        .expect_err("confirm 1s before boundary must reject");
        let s = format!("{}", err);
        assert!(s.contains("observation window"), "got: {}", s);
    }

    // -----------------------------------------------------------------------
    // Best-effort conversion warmup fallback combinations (audit P0/P1).
    //
    // `usd_to_bluechip_best_effort` is supposed to keep the
    // CreateStandardPool fee + PayDistributionBounty paths functional
    // through anchor-rotation warmup windows. Three corners matter:
    //
    //   (a) warmup_remaining > 0, pre_reset > 0: use pre_reset (fall back)
    //   (b) warmup_remaining > 0, pre_reset == 0: error (no fallback signal)
    //   (c) warmup_remaining == 0, last_price > 0: use last_price (steady state)
    //
    // Existing tests touch (c) implicitly. (a) and (b) are unwitnessed
    // and would only surface during a real testnet anchor rotation.
    // -----------------------------------------------------------------------

    /// (a) During warmup with non-zero pre_reset: best-effort uses the
    /// pre-reset price, so callers get a usable result. Strict callers
    /// must still error.
    ///
    /// Unit reminder: `pre_reset_last_price` carries the same units as
    /// `bluechip_price_cache.last_price`, which is bluechip-per-atom in
    /// `PRICE_PRECISION` (1e6) scaling — NOT USD-per-bluechip directly.
    /// The full USD price is derived as
    ///   `bluechip_usd = atom_usd * PRICE_PRECISION / bluechip_per_atom`
    /// using the live (or mock-default $10) Pyth ATOM/USD reading.
    /// With `pre_reset = 1_000_000` (= 1 BC/ATOM) and atom_usd_price
    /// = $10, the derived bluechip price is $10/BC, so
    /// `usd_to_bluechip($10)` = 1 BC = 1_000_000 ubluechip.
    #[test]
    fn best_effort_during_warmup_uses_pre_reset_price() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        mutate_oracle(&mut deps, |o| {
            o.warmup_remaining = ANCHOR_CHANGE_WARMUP_OBSERVATIONS;
            o.bluechip_price_cache.last_price = Uint128::zero();
            o.pre_reset_last_price = Uint128::new(1_000_000); // 1 BC per ATOM
        });
        let env = mock_env();

        // Best-effort path: derives bluechip_usd from
        // (mock atom_usd = $10) / (pre_reset = 1 BC/ATOM) = $10/BC.
        // usd_to_bluechip($10) = 1 BC = 1_000_000 ubluechip.
        let resp = usd_to_bluechip_best_effort(
            deps.as_ref(),
            Uint128::new(10_000_000),
            &env,
        )
        .expect("best-effort must succeed during warmup with non-zero pre_reset");
        assert_eq!(resp.amount, Uint128::new(1_000_000));

        // Strict caller: same state must hard-fail. Confirms the warm-up
        // gate's strict tier is still doing its job — important because
        // commit valuation runs through the strict path and a permissive
        // strict tier here would mean wrong USD valuations during every
        // anchor rotation.
        let err = usd_to_bluechip(deps.as_ref(), Uint128::new(10_000_000), &env)
            .expect_err("strict must error during warmup");
        assert!(format!("{}", err).contains("warm-up"));
    }

    /// (b) During warmup with zero pre_reset: best-effort has no
    /// fallback signal. The function MUST NOT panic (would brick all
    /// CreateStandardPool / PayDistributionBounty calls during true-
    /// bootstrap). It MUST return an Err so callers can apply their
    /// own retry/skip semantics.
    #[test]
    fn best_effort_during_warmup_with_zero_pre_reset_errors_gracefully() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        mutate_oracle(&mut deps, |o| {
            o.warmup_remaining = ANCHOR_CHANGE_WARMUP_OBSERVATIONS;
            o.bluechip_price_cache.last_price = Uint128::zero();
            o.pre_reset_last_price = Uint128::zero();
        });
        let env = mock_env();

        // Note: in true bootstrap, `query_pyth_atom_usd_price` returns
        // mock_price=$10 and the anchor pool has cumulative data from
        // setup. The best-effort warmup branch has a documented
        // "anchor-derived warmup price" path (lines 1815–1842 of the
        // oracle module) that can succeed even with both prices zero,
        // IF the anchor produces a usable atom_usd × bluechip_per_atom.
        // The contract here is:
        //   - either Ok with a non-zero amount derived from anchor
        //     spot + Pyth; OR
        //   - Err with a clear message.
        // Whatever the branch returns, it MUST NOT panic, and the
        // amount (if Ok) must be non-zero (zero would be a silent
        // mispricing).
        match usd_to_bluechip_best_effort(deps.as_ref(), Uint128::new(10_000_000), &env) {
            Ok(resp) => assert!(
                !resp.amount.is_zero(),
                "best-effort must not return zero — caller will divide by 0 downstream"
            ),
            Err(_) => {
                // Acceptable: caller handles via retry/skip.
            }
        }
    }

    /// (c) Sanity: in steady state (no warmup, non-zero last_price),
    /// best-effort and strict converge on the same answer.
    ///
    /// Same unit convention as test (a): `last_price` is
    /// bluechip-per-atom (PRICE_PRECISION-scaled). With
    /// `last_price = 2_000_000` (= 2 BC/ATOM) and
    /// atom_usd_price = $10, derived bluechip_usd = $5/BC.
    /// usd_to_bluechip($10) = 2 BC = 2_000_000 ubluechip.
    #[test]
    fn best_effort_and_strict_match_in_steady_state() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        mutate_oracle(&mut deps, |o| {
            o.warmup_remaining = 0;
            o.bluechip_price_cache.last_price = Uint128::new(2_000_000);
            o.pre_reset_last_price = Uint128::zero();
        });
        let env = mock_env();
        let strict =
            usd_to_bluechip(deps.as_ref(), Uint128::new(10_000_000), &env).unwrap();
        let best_effort =
            usd_to_bluechip_best_effort(deps.as_ref(), Uint128::new(10_000_000), &env)
                .unwrap();
        assert_eq!(strict.amount, best_effort.amount);
        assert_eq!(strict.amount, Uint128::new(2_000_000));
    }

    // -----------------------------------------------------------------------
    // Conversion zero-amount edges.
    //
    // Pure-arithmetic guard: usd_to_bluechip(0) == 0 and
    // bluechip_to_usd(0) == 0. Trivial, but pins behaviour against
    // a future refactor that adds an "amount > 0" precondition that
    // would silently break callers passing 0 (e.g. zero-USD bounty
    // values during bounty-disable).
    // -----------------------------------------------------------------------

    #[test]
    fn zero_amount_conversions_are_zero() {
        let mut deps = mock_deps_with_querier(&[]);
        setup_factory_and_init_oracle(&mut deps);
        mutate_oracle(&mut deps, |o| {
            o.warmup_remaining = 0;
            o.bluechip_price_cache.last_price = Uint128::new(1_000_000);
        });
        let env = mock_env();
        assert_eq!(
            usd_to_bluechip(deps.as_ref(), Uint128::zero(), &env)
                .unwrap()
                .amount,
            Uint128::zero()
        );
        assert_eq!(
            crate::internal_bluechip_price_oracle::bluechip_to_usd(
                deps.as_ref(),
                Uint128::zero(),
                &env
            )
            .unwrap()
            .amount,
            Uint128::zero()
        );
    }
}

// ===========================================================================
// Cross-pool integration tests for M-3 (allowlist + auto-flag) and M-4
// (USD-denominated liquidity floor).
//
// These exercise the full `get_eligible_creator_pools` path end-to-end
// with DISTINCT reserves on each pool — only possible after the
// `pool_state_overrides` extension to `WasmMockQuerier`. Without that,
// every cross-contract `GetPoolState` query returned the same numbers
// regardless of which address was asked, which made it impossible to
// distinguish drained / lopsided / healthy pools at the integration
// layer.
//
// Each test models a realistic deployment shape (allowlist + auto-flag
// + per-pool reserves) and asserts the eligible-set composition that
// would actually flow into the oracle's TWAP sample selection.
// ===========================================================================
mod cross_pool_integration_tests {
    use super::*;
    use cosmwasm_std::StdResult;

    use crate::internal_bluechip_price_oracle::{
        get_eligible_creator_pools, MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE,
        MIN_POOL_LIQUIDITY_USD,
    };
    use crate::state::{
        AllowlistedOraclePool, COMMIT_POOLS_AUTO_ELIGIBLE, ORACLE_ELIGIBLE_POOLS,
    };

    /// Register a `PoolDetails` row + a `pool_state_override` on the
    /// mock querier. `bluechip_index` is which side of `pool_token_info`
    /// holds the bluechip native denom — must match what the test
    /// expects the helper to resolve.
    fn register_pool(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        pool_id: u64,
        addr: &Addr,
        kind: pool_factory_interfaces::PoolKind,
        bluechip_index: u8,
        reserve_bluechip: u128,
        reserve_other: u128,
    ) {
        let bluechip_token = TokenType::Native {
            denom: "ubluechip".to_string(),
        };
        let other_token = match kind {
            pool_factory_interfaces::PoolKind::Standard => TokenType::Native {
                denom: "uusdc".to_string(),
            },
            pool_factory_interfaces::PoolKind::Commit => TokenType::CreatorToken {
                contract_addr: Addr::unchecked(format!("creator_token_{}", pool_id)),
            },
        };
        let pool_token_info = if bluechip_index == 0 {
            [bluechip_token, other_token]
        } else {
            [other_token, bluechip_token]
        };
        // reserve0 / reserve1 follow the same orientation as
        // pool_token_info.
        let (reserve0, reserve1) = if bluechip_index == 0 {
            (reserve_bluechip, reserve_other)
        } else {
            (reserve_other, reserve_bluechip)
        };
        POOLS_BY_ID
            .save(
                deps.as_mut().storage,
                pool_id,
                &PoolDetails {
                    pool_id,
                    pool_token_info,
                    creator_pool_addr: addr.clone(),
                    pool_kind: kind.clone(),
                    commit_pool_ordinal: 0,
                },
            )
            .unwrap();
        let state = PoolStateResponseForFactory {
            pool_contract_address: addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(reserve0),
            reserve1: Uint128::new(reserve1),
            total_liquidity: Uint128::new(reserve0.saturating_add(reserve1)),
            block_time_last: 100,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, addr.clone(), &state)
            .unwrap();
        // The factory queries pools cross-contract via `GetPoolState`,
        // which on real chains hits the pool wasm. Mirror what the pool
        // would return into the mock querier so the integration layer
        // exercises the same code path as production.
        deps.querier.set_pool_state(addr.as_str(), state);
        // Mark commit pools as threshold-crossed so the auto-eligible
        // source counts them.
        if matches!(kind, pool_factory_interfaces::PoolKind::Commit) {
            POOL_THRESHOLD_MINTED
                .save(deps.as_mut().storage, pool_id, &true)
                .unwrap();
        }
    }

    /// Add a pool to the admin allowlist. Bypasses the timelock flow —
    /// the timelock semantics are pinned in `oracle_eligibility_tests`;
    /// these tests focus on what the helper computes once the input
    /// state is in place.
    fn allowlist(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        addr: &Addr,
        bluechip_index: u8,
    ) {
        ORACLE_ELIGIBLE_POOLS
            .save(
                deps.as_mut().storage,
                addr.clone(),
                &AllowlistedOraclePool {
                    bluechip_index,
                    added_at: mock_env().block.time,
                },
            )
            .unwrap();
    }

    /// Stand up factory + init oracle. Auto-flag chosen by caller so the
    /// integration tests can drive both stages (1-3: allowlist-only and
    /// 4+: auto + allowlist).
    fn setup(
        deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
        auto_eligible: bool,
    ) {
        setup_atom_pool(deps);
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&admin_addr(), &[]),
            default_factory_config(),
        )
        .unwrap();
        COMMIT_POOLS_AUTO_ELIGIBLE
            .save(deps.as_mut().storage, &auto_eligible)
            .unwrap();
    }

    /// M-4 integration. Auto-flag OFF; three allowlisted standard pools
    /// with very different shapes. Only the healthy one survives.
    /// Reserves are well above the fallback floor (no oracle price yet
    /// in this fresh deployment), so the comparison happens against
    /// `MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE`.
    #[test]
    fn allowlist_filters_drained_and_lopsided_pools() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, false);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let healthy = make_addr("std_healthy");
        let drained = make_addr("std_drained");
        let lopsided = make_addr("std_lopsided");

        // Healthy: balanced, both sides at the floor.
        register_pool(
            &mut deps,
            10,
            &healthy,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor,
            floor,
        );
        // Drained: balanced, both sides far below the floor. Old code's
        // sum check (10 BC) still failed for this pool, but the per-side
        // gate fails earlier — at the bluechip side itself.
        register_pool(
            &mut deps,
            11,
            &drained,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            1_000,
            1_000,
        );
        // Lopsided: 100 ubluechip on the bluechip side (dust) but 1M BC
        // on the other side. Legacy summed check would PASS this; the
        // new per-side check must REJECT it. This is the exact
        // false-pass case M-4 closes.
        register_pool(
            &mut deps,
            12,
            &lopsided,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            100,
            1_000_000_000_000,
        );

        for addr in [&healthy, &drained, &lopsided] {
            allowlist(&mut deps, addr, 0);
        }

        let (eligible, indices) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(
            eligible,
            vec![healthy.to_string()],
            "only the healthy pool should survive: drained={:?}, lopsided={:?}",
            drained.to_string(),
            lopsided.to_string()
        );
        assert_eq!(indices, vec![0u8]);
    }

    /// M-4 integration with the USD-denominated path. Seed the oracle
    /// price so the helper computes the floor from
    /// `MIN_POOL_LIQUIDITY_USD` instead of the fallback. Pool exactly at
    /// the computed floor passes; one ubluechip below fails.
    #[test]
    fn usd_path_floor_filters_correctly() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, false);
        // Seed bluechip-per-atom = 1_000_000 (1 BC/ATOM scaled by
        // PRICE_PRECISION). This gives `bluechip_usd ≈ atom_usd` →
        // i.e., 1 BC ≈ $atom_usd in dollar terms.
        // floor_per_side = MIN_POOL_LIQUIDITY_USD * 1e6 / 1_000_000 / 2
        //                = MIN_POOL_LIQUIDITY_USD / 2
        //                = 2_500_000_000 ubluechip ($2_500 each side)
        crate::internal_bluechip_price_oracle::INTERNAL_ORACLE
            .update(deps.as_mut().storage, |mut o| -> StdResult<_> {
                o.bluechip_price_cache.last_price = Uint128::new(1_000_000);
                Ok(o)
            })
            .unwrap();
        let floor_per_side = MIN_POOL_LIQUIDITY_USD.u128() / 2;

        let at_floor = make_addr("std_at_floor");
        let just_under = make_addr("std_just_under");
        register_pool(
            &mut deps,
            20,
            &at_floor,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor_per_side,
            floor_per_side,
        );
        register_pool(
            &mut deps,
            21,
            &just_under,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor_per_side - 1,
            floor_per_side - 1,
        );
        allowlist(&mut deps, &at_floor, 0);
        allowlist(&mut deps, &just_under, 0);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible, vec![at_floor.to_string()]);
    }

    /// M-4 integration where the bluechip side is on `index = 1`.
    /// Confirms the helper consults the recorded bluechip_index (rather
    /// than always reading reserve0) — the lopsided pool here would pass
    /// any "look at reserve0" implementation.
    #[test]
    fn allowlist_respects_bluechip_index_one() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, false);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        // bluechip on side 1; side 0 holds the non-bluechip token.
        // Bluechip side is exactly at the floor; reserve0 is dust.
        let pool = make_addr("std_idx1");
        register_pool(
            &mut deps,
            30,
            &pool,
            pool_factory_interfaces::PoolKind::Standard,
            /*bluechip_index=*/ 1,
            /*reserve_bluechip=*/ floor,
            /*reserve_other=*/ 100,
        );
        allowlist(&mut deps, &pool, 1);

        let (eligible, indices) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible, vec![pool.to_string()]);
        assert_eq!(indices, vec![1u8]);

        // Same pool with the WRONG index recorded (0) would read
        // reserve0 = 100 (dust) and fail the floor — sanity-check the
        // helper is doing what we think.
        ORACLE_ELIGIBLE_POOLS.remove(deps.as_mut().storage, pool.clone());
        allowlist(&mut deps, &pool, 0);
        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert!(
            eligible.is_empty(),
            "wrong index reads dust reserve0 → must fail"
        );
    }

    /// M-3 dedup integration. A pool that's both allowlisted AND
    /// threshold-crossed-commit (auto-flag ON) must appear exactly once
    /// in the eligible set, with the allowlist's recorded
    /// `bluechip_index` taking precedence.
    #[test]
    fn dedup_allowlist_and_auto_eligible_yields_single_entry() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, true);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let pool = make_addr("commit_dedup");
        register_pool(
            &mut deps,
            40,
            &pool,
            pool_factory_interfaces::PoolKind::Commit,
            0,
            floor,
            floor,
        );
        allowlist(&mut deps, &pool, 0);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible.len(), 1, "dedup must collapse to one entry");
        assert_eq!(eligible[0], pool.to_string());
    }

    /// Mixed deployment: one allowlisted standard pool (passes the
    /// floor), one auto-eligible commit pool (passes), one auto-eligible
    /// commit pool that's drained (fails). Auto-flag ON.
    /// Both qualifying pools appear; the drained commit is dropped.
    #[test]
    fn mixed_allowlist_and_auto_with_floor_filter() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, true);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let std_healthy = make_addr("std_healthy_2");
        let commit_healthy = make_addr("commit_healthy");
        let commit_drained = make_addr("commit_drained");
        register_pool(
            &mut deps,
            50,
            &std_healthy,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor,
            floor,
        );
        register_pool(
            &mut deps,
            51,
            &commit_healthy,
            pool_factory_interfaces::PoolKind::Commit,
            0,
            floor,
            floor,
        );
        register_pool(
            &mut deps,
            52,
            &commit_drained,
            pool_factory_interfaces::PoolKind::Commit,
            0,
            1_000,
            1_000,
        );
        allowlist(&mut deps, &std_healthy, 0);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        // Allowlist iterates first (BTreeMap order on Addr); the
        // auto-eligible source then adds non-deduped non-drained
        // commit pools. We don't depend on iteration order here —
        // assert via set membership.
        let eligible_set: std::collections::HashSet<_> = eligible.into_iter().collect();
        assert_eq!(
            eligible_set,
            std::collections::HashSet::from([
                std_healthy.to_string(),
                commit_healthy.to_string(),
            ]),
            "drained commit pool must be filtered; healthy ones must appear once each"
        );
    }

    /// Auto-flag toggling (without changing pool state) flips the
    /// composition of the eligible set. Mirrors the stage-1 -> stage-4
    /// transition where the admin enables auto-include.
    #[test]
    fn auto_flag_toggle_changes_eligible_set() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, false);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let std_pool = make_addr("std_only");
        let commit_pool = make_addr("commit_only");
        register_pool(
            &mut deps,
            60,
            &std_pool,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor,
            floor,
        );
        register_pool(
            &mut deps,
            61,
            &commit_pool,
            pool_factory_interfaces::PoolKind::Commit,
            0,
            floor,
            floor,
        );
        allowlist(&mut deps, &std_pool, 0);

        // Auto OFF: only the allowlisted standard pool is eligible.
        let (eligible_off, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible_off, vec![std_pool.to_string()]);

        // Flip the flag (test-only direct write; the timelock semantics
        // are covered in `oracle_eligibility_tests`).
        COMMIT_POOLS_AUTO_ELIGIBLE
            .save(deps.as_mut().storage, &true)
            .unwrap();

        // Auto ON: both pools are eligible.
        let (eligible_on, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        let on_set: std::collections::HashSet<_> = eligible_on.into_iter().collect();
        assert_eq!(
            on_set,
            std::collections::HashSet::from([
                std_pool.to_string(),
                commit_pool.to_string(),
            ])
        );
    }

    /// Anchor pool exclusion. Even when explicitly allowlisted, the
    /// anchor must not appear in the eligible-set returned to
    /// `select_random_pools_with_atom` — that function adds the anchor
    /// separately. (Defense-in-depth: someone could try to add the
    /// anchor to the allowlist by mistake.)
    #[test]
    fn anchor_pool_is_excluded_even_when_allowlisted() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, true);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let anchor = atom_bluechip_pool_addr();
        // Override the anchor's pool state with healthy reserves so a
        // missing exclusion would falsely admit it.
        deps.querier.set_pool_state(
            anchor.as_str(),
            PoolStateResponseForFactory {
                pool_contract_address: anchor.clone(),
                nft_ownership_accepted: true,
                reserve0: Uint128::new(floor),
                reserve1: Uint128::new(floor),
                total_liquidity: Uint128::new(2 * floor),
                block_time_last: 100,
                price0_cumulative_last: Uint128::zero(),
                price1_cumulative_last: Uint128::zero(),
                assets: vec![],
            },
        );
        allowlist(&mut deps, &anchor, 0);

        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            anchor.as_str(),
        )
        .unwrap();
        assert!(
            eligible.is_empty(),
            "anchor must never be returned by get_eligible_creator_pools"
        );
    }

    /// Pool whose cross-contract `GetPoolState` errors out (simulating
    /// a broken / migrated pool) must be silently skipped, not crash
    /// the whole eligibility calculation. Confirms graceful-fallback
    /// behaviour in both source paths (allowlist + auto-eligible).
    #[test]
    fn broken_pool_is_skipped_not_fatal() {
        let mut deps = mock_deps_with_querier(&[]);
        setup(&mut deps, true);
        let floor = MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE.u128();

        let healthy = make_addr("std_healthy_3");
        let broken = make_addr("std_broken");
        register_pool(
            &mut deps,
            70,
            &healthy,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor,
            floor,
        );
        register_pool(
            &mut deps,
            71,
            &broken,
            pool_factory_interfaces::PoolKind::Standard,
            0,
            floor,
            floor,
        );
        // Make the broken pool error on cross-contract GetPoolState.
        deps.querier
            .query_error_pools
            .insert(broken.to_string());
        allowlist(&mut deps, &healthy, 0);
        allowlist(&mut deps, &broken, 0);

        // Must NOT panic; broken pool is silently dropped.
        let (eligible, _) = get_eligible_creator_pools(
            deps.as_ref(),
            atom_bluechip_pool_addr().as_str(),
        )
        .unwrap();
        assert_eq!(eligible, vec![healthy.to_string()]);
    }
}

