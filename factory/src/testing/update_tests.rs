use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
};
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Coin, CosmosMsg, Decimal, Empty, OwnedDeps, Uint128, WasmMsg,
};

use crate::asset::TokenType;
use crate::error::ContractError;
use crate::execute::{execute, instantiate};
use crate::mock_querier::WasmMockQuerier;
use crate::msg::ExecuteMsg;
use crate::pool_struct::{CommitFeeInfo, PoolConfigUpdate, PoolDetails};
use crate::state::{
    FactoryInstantiate, PENDING_CONFIG, PENDING_POOL_UPGRADE, POOLS_BY_ID, POOL_REGISTRY,
};
use crate::testing::tests::setup_atom_pool;

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

    let msg = FactoryInstantiate {
        cw721_nft_contract_id: 58,
        factory_admin_address: Addr::unchecked("addr0000"),
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
        commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    let env = mock_env();
    let info = mock_info("addr0000", &[]);

    instantiate(deps.as_mut(), env.clone(), info.clone(), msg.clone()).unwrap();

    let unauthorized_info = mock_info("unauthorized", &[]);
    let new_config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("addr0000"),
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
    assert_eq!(
        err.to_string(),
        "Generic error: Only the admin can execute this function. Admin: addr0000, Sender: unauthorized"
    );

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
    POOL_REGISTRY
        .save(&mut deps.storage, pool_id, &pool_address)
        .unwrap();

    let loaded = POOL_REGISTRY.load(&deps.storage, pool_id).unwrap();
    assert_eq!(loaded, pool_address);
}

#[test]
fn test_upgrade_pools_with_registry() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    for i in 1..=5 {
        let pool_addr = Addr::unchecked(format!("pool_{}", i));
        POOL_REGISTRY
            .save(&mut deps.storage, i, &pool_addr)
            .unwrap();
    }

    let admin_info = mock_info("admin", &[]);
    let upgrade_msg = ExecuteMsg::UpgradePools {
        new_code_id: 200,
        pool_ids: None,
        migrate_msg: to_json_binary(&Empty {}).unwrap(),
    };

    let res = execute(deps.as_mut(), mock_env(), admin_info, upgrade_msg).unwrap();

    assert_eq!(res.messages.len(), 5);

    for (i, msg) in res.messages.iter().enumerate() {
        match &msg.msg {
            CosmosMsg::Wasm(WasmMsg::Migrate {
                contract_addr,
                new_code_id,
                ..
            }) => {
                assert_eq!(contract_addr, &format!("pool_{}", i + 1));
                assert_eq!(*new_code_id, 200);
            }
            _ => panic!("Expected migrate message"),
        }
    }
}

#[test]
fn test_update_specific_pool_from_registry() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    let pool_id = 3u64;
    let pool_addr = Addr::unchecked("pool_3_address");
    POOL_REGISTRY
        .save(&mut deps.storage, pool_id, &pool_addr)
        .unwrap();

    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("token"),
            },
        ],
        creator_pool_addr: pool_addr.clone(),
    };
    POOLS_BY_ID
        .save(&mut deps.storage, pool_id, &pool_details)
        .unwrap();

    let admin_info = mock_info("admin", &[]);
    let pool_config = PoolConfigUpdate {
        commit_fee_info: Some(CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip"),
            creator_wallet_address: Addr::unchecked("creator"),
            commit_fee_bluechip: Decimal::percent(2), // Changed
            commit_fee_creator: Decimal::percent(10), // Changed
        }),
        commit_limit_usd: Some(Uint128::new(30_000_000_000)),
        pyth_contract_addr_for_conversions: None,
        pyth_atom_usd_price_feed_id: None,
        commit_amount_for_threshold: Some(Uint128::new(30_000_000_000)), // Changed
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
    };

    let update_msg = ExecuteMsg::UpdatePoolConfig {
        pool_id,
        pool_config,
    };

    let res = execute(deps.as_mut(), mock_env(), admin_info, update_msg).unwrap();

    assert_eq!(res.messages.len(), 1);
    match &res.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) => {
            assert_eq!(contract_addr, "pool_3_address");
        }
        _ => panic!("Expected execute message"),
    }
}
#[test]
fn test_migration_with_large_pool_count() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    for i in 1..=25 {
        POOL_REGISTRY
            .save(
                &mut deps.storage,
                i,
                &Addr::unchecked(format!("pool_{}", i)),
            )
            .unwrap();
    }

    let admin_info = mock_info("admin", &[]);
    let upgrade_msg = ExecuteMsg::UpgradePools {
        new_code_id: 300,
        pool_ids: None,
        migrate_msg: to_json_binary(&Empty {}).unwrap(),
    };

    let res = execute(deps.as_mut(), mock_env(), admin_info, upgrade_msg).unwrap();

    assert_eq!(res.messages.len(), 11);

    match &res.messages[10].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
            let exec_msg: ExecuteMsg = from_json(msg).unwrap();
            assert!(matches!(exec_msg, ExecuteMsg::ContinuePoolUpgrade {}));
        }
        _ => panic!("Expected continuation message"),
    }

    let pending = PENDING_POOL_UPGRADE.load(&deps.storage).unwrap();
    assert_eq!(pending.pools_to_upgrade.len(), 25);
    assert_eq!(pending.upgraded_count, 0);
}
#[test]
fn test_continue_upgrade_unauthorized() {
    let mut deps = mock_dependencies();
    setup_factory(&mut deps);

    let info = mock_info("hacker", &[]);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ContinuePoolUpgrade {},
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::Unauthorized {}));
}

fn setup_factory(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("admin"),
        commit_amount_for_threshold_bluechip: Uint128::new(25_000_000_000),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle".to_string(),
        pyth_atom_usd_price_feed_id: "feed".to_string(),
        cw20_token_contract_id: 10,
        cw721_nft_contract_id: 20,
        create_pool_wasm_contract_id: 30,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    instantiate(deps.as_mut(), mock_env(), mock_info("deployer", &[]), msg).unwrap();
}
