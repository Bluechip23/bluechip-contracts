use cosmwasm_std::testing::{
    message_info, mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage,
    MOCK_CONTRACT_ADDR,
};
use cosmwasm_std::{
    to_json_binary, Addr, Coin, CosmosMsg, Decimal, Empty, OwnedDeps, Uint128, WasmMsg,
};

use crate::asset::TokenType;
use crate::error::ContractError;
use crate::execute::{execute, instantiate};
use crate::mock_querier::WasmMockQuerier;
use crate::msg::ExecuteMsg;
use crate::pool_struct::{PoolConfigUpdate, PoolDetails};
use crate::state::{FactoryInstantiate, PENDING_CONFIG, PENDING_POOL_UPGRADE, POOLS_BY_ID};
use crate::testing::tests::{register_test_pool_addr, setup_atom_pool};

fn make_addr(label: &str) -> Addr {
    MockApi::default().addr_make(label)
}

fn admin_addr() -> Addr {
    make_addr("admin")
}

fn atom_bluechip_pool_addr() -> Addr {
    make_addr("atom_bluechip_pool")
}

pub fn mock_dependencies_2(
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

#[test]
fn test_propose_and_execute_update_config() {
    let mut deps = mock_dependencies_2(&[]);
    setup_atom_pool(&mut deps);
    let the_admin = make_addr("addr0000");
    let msg = FactoryInstantiate {
        cw721_nft_contract_id: 58,
        factory_admin_address: the_admin.clone(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: MockApi::default().addr_make("oracle0000").to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: make_addr("ubluechip"),
        commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
        commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
        threshold_payout_amounts: Default::default(),
        emergency_withdraw_delay_seconds: 86_400,
    };

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    instantiate(deps.as_mut(), env.clone(), info.clone(), msg.clone()).unwrap();

    let unauthorized_info = message_info(&Addr::unchecked("unauthorized"), &[]);
    let new_config = FactoryInstantiate {
        factory_admin_address: the_admin.clone(),
        ..msg.clone()
    };
    let propose_msg = ExecuteMsg::ProposeConfigUpdate {
        config: new_config.clone(),
    };

    let err = execute(
        deps.as_mut(),
        env.clone(),
        unauthorized_info,
        propose_msg.clone(),
    )
    .unwrap_err();
    let _ = the_admin; // ensure binding stays in scope across edits
    assert_eq!(err.to_string(), "Unauthorized");

    let res = execute(deps.as_mut(), env.clone(), info.clone(), propose_msg).unwrap();
    assert_eq!(res.attributes[0], ("action", "propose_config_update"));

    // Check pending config exists
    let pending = PENDING_CONFIG.load(&deps.storage).unwrap();
    assert_eq!(pending.new_config.cw721_nft_contract_id, 58);
    assert!(pending.effective_after.seconds() > env.block.time.seconds());

    let early_update_msg = ExecuteMsg::UpdateConfig {};
    let err = execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
        early_update_msg.clone(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not yet effective"),
        "Unexpected error: {}",
        err
    );

    let mut later_env = env.clone();
    later_env.block.time = pending.effective_after.plus_seconds(1);

    let res = execute(deps.as_mut(), later_env, info.clone(), early_update_msg).unwrap();
    assert_eq!(res.attributes[0], ("action", "execute_update_config"));

    // Pending config should now be cleared
    assert!(PENDING_CONFIG.may_load(&deps.storage).unwrap().is_none());
}

#[test]
fn test_pool_registry_population() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);
    let pool_id = 1u64;
    let pool_address = Addr::unchecked("pool_1");
    register_test_pool_addr(&mut deps.storage, pool_id, &pool_address);

    let loaded = POOLS_BY_ID
        .load(&deps.storage, pool_id)
        .unwrap()
        .creator_pool_addr;
    assert_eq!(loaded, pool_address);
}

#[test]
fn test_upgrade_pools_with_registry() {
    // Uses the custom querier so the IsPaused gate inside
    // build_upgrade_batch can resolve (default = not paused). The
    // plain MockQuerier doesn't know how to answer wasm-smart queries,
    // so under the new fail-closed semantics it would revert the
    // apply — which is the correct behaviour, just not what this test
    // is exercising.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=5 {
        let pool_addr = Addr::unchecked(format!("pool_{}", i));
        register_test_pool_addr(&mut deps.storage, i, &pool_addr);
    }

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    let upgrade_msg = ExecuteMsg::UpgradePools {
        new_code_id: 200,
        pool_ids: None,
        migrate_msg: to_json_binary(&Empty {}).unwrap(),
    };

    // Step 1: Propose — no migrations yet, just saves pending upgrade
    let res = execute(deps.as_mut(), env.clone(), admin_info.clone(), upgrade_msg).unwrap();
    assert_eq!(res.messages.len(), 0); // No migrate messages on proposal
    assert_eq!(res.attributes[0], ("action", "propose_pool_upgrade"));

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(pending.pools_to_upgrade.len(), 5);
    assert_eq!(pending.new_code_id, 200);
    assert!(pending.effective_after.seconds() > env.block.time.seconds());

    // Step 2: Try to execute before timelock — should fail
    let err = execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not yet effective"),
        "Unexpected error: {}",
        err
    );

    // Step 3: Execute after timelock — migrations happen
    let mut later_env = env.clone();
    later_env.block.time = pending.effective_after.plus_seconds(1);

    let res = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();

    assert_eq!(res.messages.len(), 5);

    for (i, msg) in res.messages.iter().enumerate() {
        match &msg.msg {
            CosmosMsg::Wasm(WasmMsg::Migrate {
                contract_addr,
                new_code_id,
                ..
            }) => {
                assert_eq!(contract_addr.as_str(), &format!("pool_{}", i + 1));
                assert_eq!(*new_code_id, 200);
            }
            _ => panic!("Expected migrate message"),
        }
    }

    // Pending upgrade should be cleaned up (all pools fit in one batch)
    assert!(PENDING_POOL_UPGRADE
        .may_load(&deps.storage)
        .unwrap()
        .is_none());
}

#[test]
fn test_update_specific_pool_from_registry() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    let pool_id = 3u64;
    let pool_addr = Addr::unchecked("pool_3_address");

    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("token"),
            },
        ],
        creator_pool_addr: pool_addr.clone(),
        pool_kind: pool_factory_interfaces::PoolKind::Commit,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID
        .save(&mut deps.storage, pool_id, &pool_details)
        .unwrap();

    let admin_info = message_info(&admin_addr(), &[]);
    let pool_config = PoolConfigUpdate {
        lp_fee: None,
        min_commit_interval: None,
        ..Default::default()
    };

    let update_msg = ExecuteMsg::ProposePoolConfigUpdate {
        pool_id,
        pool_config,
    };

    let res = execute(deps.as_mut(), mock_env(), admin_info.clone(), update_msg).unwrap();

    // Propose should NOT send a message yet — just store the pending update
    assert_eq!(res.messages.len(), 0);

    // Advance time past 48-hour timelock
    let mut future_env = mock_env();
    future_env.block.time = future_env
    .block
    .time
    .plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);

    let apply_msg = ExecuteMsg::ExecutePoolConfigUpdate { pool_id };
    let res = execute(deps.as_mut(), future_env, admin_info, apply_msg).unwrap();

    assert_eq!(res.messages.len(), 1);
    match &res.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) => {
            assert_eq!(contract_addr.as_str(), "pool_3_address");
        }
        _ => panic!("Expected execute message"),
    }
}
#[test]
fn test_migration_with_large_pool_count() {
    // Custom querier so IsPaused resolves to default=false. Plain
    // MockQuerier would now fail-closed.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=25 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    let upgrade_msg = ExecuteMsg::UpgradePools {
        new_code_id: 300,
        pool_ids: None,
        migrate_msg: to_json_binary(&Empty {}).unwrap(),
    };

    // Step 1: Propose — no messages, just saves pending
    let res = execute(deps.as_mut(), env.clone(), admin_info.clone(), upgrade_msg).unwrap();
    assert_eq!(res.messages.len(), 0);

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(pending.pools_to_upgrade.len(), 25);
    assert_eq!(pending.upgraded_count, 0);

    // Step 2: Execute after timelock — first batch of 10, NO self-continuation.
    // Admin must call ContinuePoolUpgrade explicitly for the remaining 15.
    let mut later_env = env.clone();
    later_env.block.time = pending.effective_after.plus_seconds(1);

    let res = execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();

    // 10 migrate messages only — the self-dispatched ContinuePoolUpgrade
    // has been removed to prevent gas-limit blowouts on large fleets.
    assert_eq!(res.messages.len(), 10);
    for m in &res.messages {
        assert!(
            matches!(m.msg, CosmosMsg::Wasm(WasmMsg::Migrate { .. })),
            "expected only Migrate messages, got: {:?}",
            m.msg
        );
    }

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(pending.pools_to_upgrade.len(), 25);
    assert_eq!(pending.upgraded_count, 10);

    // Admin calls ContinuePoolUpgrade explicitly — processes next 10.
    let res = execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();
    assert_eq!(res.messages.len(), 10);
    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(pending.upgraded_count, 20);

    // Final batch — processes the last 5 and clears pending state.
    let res = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();
    assert_eq!(res.messages.len(), 5);
    assert!(
        PENDING_POOL_UPGRADE.may_load(&deps.storage).unwrap().is_none(),
        "PENDING_POOL_UPGRADE should be cleared after final batch"
    );
}

#[test]
fn test_upgrade_skips_paused_pools() {
    // Confirms that pools reporting paused=true are deferred to
    // `pending_retry` rather than silently counted-and-dropped. The
    // first pass migrates the unpaused pools and leaves PENDING_POOL_UPGRADE
    // around (with pool_2 in pending_retry) so a future
    // ContinuePoolUpgrade can retry it once the admin has unpaused.
    // Uses mock_dependencies_2 so we can mark specific pools as paused
    // on the custom querier.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=3 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }

    // Mark pool_2 as paused via the mock querier.
    deps.querier.paused_pools.insert("pool_2".to_string());

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 400,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();

    let mut later_env = env.clone();
    later_env.block.time = pending.effective_after.plus_seconds(1);

    let res = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();

    // Only pool_1 and pool_3 get migrate messages; pool_2 is skipped.
    assert_eq!(res.messages.len(), 2);
    let migrated_addrs: Vec<String> = res
        .messages
        .iter()
        .filter_map(|sm| match &sm.msg {
            CosmosMsg::Wasm(WasmMsg::Migrate { contract_addr, .. }) => Some(contract_addr.clone()),
            _ => None,
        })
        .collect();
    assert!(migrated_addrs.contains(&"pool_1".to_string()));
    assert!(migrated_addrs.contains(&"pool_3".to_string()));
    assert!(!migrated_addrs.contains(&"pool_2".to_string()));

    // Skipped attribute must name pool_2.
    let skipped = res
        .attributes
        .iter()
        .find(|a| a.key == "skipped_paused")
        .map(|a| a.value.clone())
        .unwrap_or_default();
    assert_eq!(skipped, "2");

    // Pending state is RETAINED so the admin can retry pool_2 after
    // unpausing via ContinuePoolUpgrade. pending_retry holds the
    // skipped id.
    let pending_after = PENDING_POOL_UPGRADE
        .may_load(&deps.storage)
        .unwrap()
        .expect("PENDING_POOL_UPGRADE should remain while pending_retry is non-empty");
    assert_eq!(pending_after.pending_retry, vec![2u64]);
    // First-pass cursor reached the end (all 3 pools decided on).
    assert_eq!(pending_after.upgraded_count, 3);
    assert_eq!(pending_after.pools_to_upgrade.len(), 3);
}

#[test]
fn test_upgrade_first_pass_fails_closed_on_query_error() {
    // on the FIRST pass, an IsPaused query error
    // propagates and reverts the apply. Treating "query unreachable"
    // as "not paused" would be a fail-open — we don't know whether
    // the pool is in a sensitive state, and committing to a Migrate
    // on that basis is the wrong direction.
    //
    // The retry pass tolerates the same error (separate test below)
    // because we're revisiting pools we already deferred; a single
    // transiently-broken pool shouldn't veto progress on the others.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=2 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }

    deps.querier
        .query_error_pools
        .insert("pool_1".to_string());

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 500,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();
    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();

    let mut later_env = env;
    later_env.block.time = pending.effective_after.plus_seconds(1);

    let err = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("IsPaused query failed"),
        "expected fail-closed error mentioning IsPaused, got: {}",
        msg
    );

    // No pool migrated, PENDING_POOL_UPGRADE retained (admin must Cancel
    // and either fix pool_1 or re-propose without it).
    let still_pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(still_pending.upgraded_count, 0);
    assert!(still_pending.pending_retry.is_empty());
}

#[test]
fn test_upgrade_retry_path_migrates_unpaused_pool() {
    // a pool paused on first pass lands in
    // pending_retry. Once the admin has unpaused it (test-side: drop
    // from `paused_pools`), the next ContinuePoolUpgrade migrates it
    // in-place — no Cancel + re-Propose required.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=3 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }
    deps.querier.paused_pools.insert("pool_2".to_string());

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 600,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();
    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();

    let mut later_env = env;
    later_env.block.time = pending.effective_after.plus_seconds(1);

    // First pass: pool_1 and pool_3 migrate, pool_2 lands in pending_retry.
    let res = execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();
    assert_eq!(res.messages.len(), 2);
    let mid = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(mid.pending_retry, vec![2u64]);
    assert_eq!(mid.upgraded_count, 3);

    // Admin unpauses pool_2 (test-side: drop from paused set), calls
    // Continue. Without fix, this required Cancel + re-Propose
    // + wait 48h.
    deps.querier.paused_pools.remove("pool_2");

    let res = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();

    // pool_2 migrated, mode=retry, pending_retry drained, PENDING removed.
    assert_eq!(res.messages.len(), 1);
    let migrated_addrs: Vec<String> = res
        .messages
        .iter()
        .filter_map(|sm| match &sm.msg {
            CosmosMsg::Wasm(WasmMsg::Migrate { contract_addr, .. }) => Some(contract_addr.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(migrated_addrs, vec!["pool_2".to_string()]);

    let mode = res
        .attributes
        .iter()
        .find(|a| a.key == "mode")
        .map(|a| a.value.clone())
        .unwrap();
    assert_eq!(mode, "retry");

    assert!(
        PENDING_POOL_UPGRADE.may_load(&deps.storage).unwrap().is_none(),
        "PENDING_POOL_UPGRADE should be cleared once first pass is done AND pending_retry is empty"
    );
}

#[test]
fn test_upgrade_retry_path_keeps_still_paused_pool() {
    // If a pool remains paused on retry, it stays in pending_retry
    // for the next Continue call. The admin can Cancel to abandon.
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    for i in 1..=2 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }
    deps.querier.paused_pools.insert("pool_2".to_string());

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 700,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();
    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();

    let mut later_env = env;
    later_env.block.time = pending.effective_after.plus_seconds(1);

    // First pass.
    execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();
    assert_eq!(
        PENDING_POOL_UPGRADE.load(&deps.storage).unwrap().pending_retry,
        vec![2u64]
    );

    // Retry while pool_2 is still paused: no migrate, pool stays on the queue.
    let res = execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();
    assert_eq!(res.messages.len(), 0);
    let after = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(after.pending_retry, vec![2u64]);
}

#[test]
fn test_continue_upgrade_unauthorized() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    // After removing self-dispatch, ContinuePoolUpgrade is admin-only.
    // A random caller must be rejected.
    let info = message_info(&Addr::unchecked("hacker"), &[]);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap_err();
    // assert_correct_factory_address returns a Std error containing "admin"
    assert!(
        format!("{}", err).contains("admin") || matches!(err, ContractError::Unauthorized {}),
        "expected admin rejection, got: {}",
        err
    );
}

#[test]
fn test_cancel_pool_upgrade() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    for i in 1..=3 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
    }

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    // Propose upgrade
    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 999,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();

    assert!(PENDING_POOL_UPGRADE
        .may_load(&deps.storage)
        .unwrap()
        .is_some());

    // Unauthorized cancel should fail
    let err = execute(
        deps.as_mut(),
        env.clone(),
        message_info(&Addr::unchecked("hacker"), &[]),
        ExecuteMsg::CancelPoolUpgrade {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    // Admin cancel should succeed
    let res = execute(
        deps.as_mut(),
        env,
        admin_info,
        ExecuteMsg::CancelPoolUpgrade {},
    )
    .unwrap();
    assert_eq!(res.attributes[0], ("action", "cancel_pool_upgrade"));
    assert!(PENDING_POOL_UPGRADE
        .may_load(&deps.storage)
        .unwrap()
        .is_none());
}

fn setup_factory(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    let msg = default_factory_instantiate_msg();
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&make_addr("deployer"), &[]),
        msg,
    )
    .unwrap();
}

fn setup_factory_custom(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>) {
    let msg = default_factory_instantiate_msg();
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&make_addr("deployer"), &[]),
        msg,
    )
    .unwrap();
}

fn default_factory_instantiate_msg() -> FactoryInstantiate {
    FactoryInstantiate {
        factory_admin_address: admin_addr(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: MockApi::default().addr_make("oracle").to_string(),
        pyth_atom_usd_price_feed_id: "feed".to_string(),
        cw20_token_contract_id: 10,
        cw721_nft_contract_id: 20,
        create_pool_wasm_contract_id: 30,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: make_addr("ubluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
        threshold_payout_amounts: Default::default(),
        emergency_withdraw_delay_seconds: 86_400,
    }
}

/// Regression for audit-followup §2.2: when the retry batch (first 10
/// entries of `pending_retry`) all remain paused, the queue rotates so
/// later entries get their turn on subsequent retry calls. Pre-fix the
/// head 10 stayed at positions 0-9 forever and tail entries never got
/// processed without an admin Cancel + re-Propose.
#[test]
fn test_upgrade_retry_queue_rotates_when_head_stays_paused() {
    let mut deps = mock_dependencies_2(&[]);
    setup_factory_custom(&mut deps);

    // 11 pools — one more than `batch_size = 10` so the head/tail
    // distinction is meaningful. All start paused.
    for i in 1..=11u64 {
        register_test_pool_addr(&mut deps.storage, i, &Addr::unchecked(format!("pool_{}", i)));
        deps.querier.paused_pools.insert(format!("pool_{}", i));
    }

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::UpgradePools {
            new_code_id: 700,
            pool_ids: None,
            migrate_msg: to_json_binary(&Empty {}).unwrap(),
        },
    )
    .unwrap();
    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    let mut later_env = env;
    later_env.block.time = pending.effective_after.plus_seconds(1);

    // First pass: processes pools 1-10 (all paused), they go into pending_retry.
    execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ExecutePoolUpgrade {},
    )
    .unwrap();

    // Continue first pass: processes pool 11, also paused.
    execute(
        deps.as_mut(),
        later_env.clone(),
        admin_info.clone(),
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();

    let pre_retry = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(
        pre_retry.pending_retry,
        (1u64..=11u64).collect::<Vec<u64>>(),
        "all 11 paused pools should be in pending_retry in original order"
    );

    // Retry call: batch = [1..10], all still paused, no migrations.
    // Pre-fix: pending_retry unchanged → [1..11].
    // Post-fix: rotation moves the 10 skipped entries to the back,
    //           giving [11, 1, 2, ..., 10].
    execute(
        deps.as_mut(),
        later_env,
        admin_info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap();

    let post_retry = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    let expected_rotated: Vec<u64> = std::iter::once(11u64)
        .chain(1u64..=10u64)
        .collect();
    assert_eq!(
        post_retry.pending_retry, expected_rotated,
        "after retry with all head pools still paused, queue must rotate so pool 11 (previously at the back) moves to the front"
    );
}
