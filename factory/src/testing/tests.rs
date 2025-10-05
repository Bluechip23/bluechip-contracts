use crate::state::{
    CreationStatus, FactoryInstantiate, PoolCreationState, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID,
    POOL_CREATION_STATES, SETCOMMIT, TEMPCREATORTOKENADDR, TEMPCREATORWALLETADDR, TEMPNFTADDR,
    TEMPPOOLID, TEMPPOOLINFO,
};
use cosmwasm_std::{
    Addr, Binary, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse, SubMsgResult, Uint128,
};

use crate::asset::{TokenInfo, TokenType};
use crate::execute::{
    execute, instantiate, pool_creation_reply, FINALIZE_POOL, MINT_CREATE_POOL, SET_TOKENS,
};
use crate::internal_bluechip_price_oracle::{ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS, INTERNAL_ORACLE};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CommitFeeInfo, CreatePool};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage};
use pool_factory_interfaces::PoolStateResponseForFactory;

const ADMIN: &str = "admin";

fn create_default_instantiate_msg() -> FactoryInstantiate {
    FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    }
}

// Helper function to set up ATOM pool in storage before instantiation
fn setup_atom_pool(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>) {
    let atom_pool_addr = Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS);
    let atom_pool_state = PoolStateResponseForFactory {
        pool_contract_address: atom_pool_addr.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000_000_000), // 1M bluechip with 6 decimals
        reserve1: Uint128::new(100_000_000_000),   // 100k ATOM with 6 decimals
        total_liquidity: Uint128::new(100_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };

    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, atom_pool_addr, &atom_pool_state)
        .unwrap();
}

#[test]
fn proper_initialization() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool first
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(addr.as_str(), &[]);

    let res = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();

    // Verify oracle was initialized
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(
        !oracle.selected_pools.is_empty(),
        "Oracle should have at least ATOM pool"
    );
    assert_eq!(
        oracle.atom_pool_contract_address,
        Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS),
        "ATOM pool address should be set correctly"
    );
    assert!(
        oracle
            .selected_pools
            .contains(&ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()),
        "Selected pools should include ATOM pool"
    );

    // Verify response attributes
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "init_contract"));

    // Test multiple instantiations with fresh dependencies
    let mut deps2 = mock_dependencies(&[]);
    setup_atom_pool(&mut deps2);

    let env = mock_env();
    let addr = Addr::unchecked("addr0001");
    let info = mock_info(&addr.as_str(), &[]);

    let _res1 = instantiate(deps2.as_mut(), env.clone(), info, msg.clone()).unwrap();

    let mut deps3 = mock_dependencies(&[]);
    setup_atom_pool(&mut deps3);

    let env = mock_env();
    let addr = Addr::unchecked("addr0002");
    let info = mock_info(&addr.as_str(), &[]);

    instantiate(deps3.as_mut(), env.clone(), info, msg.clone()).unwrap();
}

#[test]
fn test_oracle_initialization_with_no_other_pools() {
    let mut deps = mock_dependencies(&[]);

    // Only set up ATOM pool, no other creator pools
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);

    let res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Verify oracle initialized with just ATOM pool
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert_eq!(
        oracle.selected_pools.len(),
        1,
        "Should have only ATOM pool when no other pools exist"
    );
    assert_eq!(
        oracle.selected_pools[0],
        ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS
    );

    // Verify cache is initialized
    assert_eq!(oracle.bluechip_price_cache.last_price, Uint128::zero());
    assert_eq!(oracle.bluechip_price_cache.last_update, 0);
    assert!(oracle.bluechip_price_cache.twap_observations.is_empty());
}

#[test]
fn test_oracle_initialization_with_multiple_pools() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    // Add 5 more creator pools with sufficient liquidity
    for i in 1..=5 {
        let pool_addr = Addr::unchecked(format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000), // 50k bluechip
            reserve1: Uint128::new(10_000_000_000), // 10k creator token
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Verify oracle selected multiple pools (ATOM + up to 3 random = 4 total max)
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(
        oracle.selected_pools.len() >= 1,
        "Should have at least ATOM pool"
    );
    assert!(
        oracle.selected_pools.len() <= 5,
        "Should not exceed ORACLE_POOL_COUNT (5)"
    );
    assert!(
        oracle
            .selected_pools
            .contains(&ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()),
        "Should always include ATOM pool"
    );
}

#[test]
fn create_pair() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("addr0000"),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let pool_token_info = [
        TokenType::Bluechip {
            denom: "bluechip".to_string(),
        },
        TokenType::CreatorToken {
            contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
    ];

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::Create {
            pool_msg: CreatePool {
                pool_token_info: pool_token_info.clone(),
                cw20_token_contract_id: 10,
                factory_to_create_pool_addr: Addr::unchecked("factory"),
                threshold_payout: None,
                commit_fee_info: CommitFeeInfo {
                    bluechip_wallet_address: Addr::unchecked("bluechip"),
                    creator_wallet_address: Addr::unchecked("creator"),
                    commit_fee_bluechip: Decimal::percent(1),
                    commit_fee_creator: Decimal::percent(5),
                },
                commit_amount_for_threshold: Uint128::zero(),
                commit_limit_usd: Uint128::new(25_000_000_000),
                pyth_contract_addr_for_conversions: "oracle0000".to_string(),
                pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
                creator_token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            },
            token_info: CreatorTokenInfo {
                token_name: "Test Token".to_string(),
                ticker: "TEST".to_string(),
                decimal: 6,
            },
        },
    )
    .unwrap();

    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "create"));
    assert!(res.attributes.iter().any(|attr| attr.key == "creator"));
    assert!(res.attributes.iter().any(|attr| attr.key == "pool_id"));
}

#[test]
fn test_create_pair_with_custom_params() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    let custom_params = Binary::from(b"custom_pool_params");

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            cw20_token_contract_id: 10,
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            threshold_payout: Some(custom_params),
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("bluechip"),
                creator_wallet_address: Addr::unchecked(ADMIN),
                commit_fee_bluechip: Decimal::percent(1),
                commit_fee_creator: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(25_000_000_000),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
            creator_token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
        token_info: CreatorTokenInfo {
            token_name: "Custom Token".to_string(),
            ticker: "CUSTOM".to_string(),
            decimal: 6,
        },
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    let res = execute(deps.as_mut(), env, info, create_msg).unwrap();

    assert_eq!(res.messages.len(), 1);
}

fn create_pool_msg(token_name: &str) -> ExecuteMsg {
    ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            cw20_token_contract_id: 10,
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            threshold_payout: None,
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("bluechip"),
                creator_wallet_address: Addr::unchecked("creator"),
                commit_fee_bluechip: Decimal::percent(1),
                commit_fee_creator: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(25_000_000_000),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
            creator_token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
        token_info: CreatorTokenInfo {
            token_name: token_name.to_string(),
            ticker: token_name.to_string(),
            decimal: 6,
        },
    }
}

fn simulate_complete_reply_chain(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    env: Env,
    pool_id: u64,
) {
    let token_reply = create_instantiate_reply(SET_TOKENS, &format!("token_address_{}", pool_id));
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    let nft_reply = create_instantiate_reply(MINT_CREATE_POOL, &format!("nft_address_{}", pool_id));
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    let pool_reply = create_instantiate_reply(FINALIZE_POOL, &format!("pool_address_{}", pool_id));
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();
}

#[test]
fn test_asset_info() {
    let bluechip_info = TokenType::Bluechip {
        denom: "bluechip".to_string(),
    };
    assert!(bluechip_info.is_bluechip_token());

    let token_info = TokenType::CreatorToken {
        contract_addr: Addr::unchecked("bluechip..."),
    };
    assert!(!token_info.is_bluechip_token());

    assert!(bluechip_info.equal(&TokenType::Bluechip {
        denom: "bluechip".to_string(),
    }));
    assert!(!bluechip_info.equal(&token_info));
}

fn create_instantiate_reply(id: u64, contract_addr: &str) -> Reply {
    Reply {
        id,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![
                Event::new("instantiate").add_attribute("_contract_address", contract_addr)
            ],
            data: None,
        }),
    }
}

#[test]
fn test_multiple_pool_creation() {
    let mut deps = mock_dependencies(&[]);
    // Set up ATOM pool
    setup_atom_pool(&mut deps);
    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
    // Create 3 pools and verify they're created with unique random IDs
    let mut created_pool_ids = Vec::new();
    for i in 1u64..=3u64 {
        // Create pool
        let create_msg = create_pool_msg(&format!("Token{}", i));
        let info = mock_info(ADMIN, &[]);
        let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();
        assert!(
            res.attributes.iter().any(|attr| attr.key == "pool_id"),
            "Response should contain pool_id attribute"
        );
        // Get the actual pool ID (it will be random, not sequential)
        let pool_id = TEMPPOOLID.load(&deps.storage).unwrap();
        // Verify this is a new unique ID
        assert!(
            !created_pool_ids.contains(&pool_id),
            "Pool ID {} should be unique", pool_id
        );
        created_pool_ids.push(pool_id);
        // SET UP CREATION STATE
        let creator = TEMPCREATORWALLETADDR.load(&deps.storage).unwrap();
        let creation_state = PoolCreationState {
            pool_id,
            creator: creator.clone(),
            creator_token_address: None,
            mint_new_position_nft_address: None,
            pool_address: None,
            creation_time: env.block.time,
            status: CreationStatus::Started,
            retry_count: 0,
        };
        POOL_CREATION_STATES
            .save(deps.as_mut().storage, pool_id, &creation_state)
            .unwrap();
        // Simulate complete reply chain with the actual pool_id
        simulate_complete_reply_chain(&mut deps, env.clone(), pool_id);
        // Verify pool was created successfully
        assert!(POOLS_BY_ID.load(&deps.storage, pool_id).is_ok(), "Pool should be stored by ID");
        // Verify creation state shows completed
        let final_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
        assert_eq!(final_state.status, CreationStatus::Completed);
    }
    
    // Verify we created 3 unique pools
    assert_eq!(created_pool_ids.len(), 3, "Should have created 3 pools");
}
#[test]
fn test_complete_pool_creation_flow() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("token0000"),
                },
            ],
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            cw20_token_contract_id: 10,
            threshold_payout: None,
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: Addr::unchecked("bluechip"),
                creator_wallet_address: Addr::unchecked("addr0000"),
                commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
                commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(100),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "ORCL".to_string(),
            creator_token_address: Addr::unchecked("token0000"),
        },
        token_info: CreatorTokenInfo {
            token_name: "Test Token".to_string(),
            ticker: "TEST".to_string(),
            decimal: 6,
        },
    };

    let info = mock_info(ADMIN, &[]);
    let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

    assert!(
        !res.attributes.is_empty(),
        "Should have response attributes"
    );
    assert_eq!(
        res.messages.len(),
        1,
        "Should have exactly one submessage for token instantiation"
    );

    assert!(TEMPPOOLID.load(&deps.storage).is_ok());
    assert!(TEMPPOOLINFO.load(&deps.storage).is_ok());
    assert!(TEMPCREATORWALLETADDR.load(&deps.storage).is_ok());

    let pool_id = TEMPPOOLID.load(&deps.storage).unwrap();
    let creator = TEMPCREATORWALLETADDR.load(&deps.storage).unwrap();

    let creation_state = PoolCreationState {
        pool_id,
        creator: creator.clone(),
        creator_token_address: None,
        mint_new_position_nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    POOL_CREATION_STATES
        .save(deps.as_mut().storage, pool_id, &creation_state)
        .unwrap();

    let token_reply = create_instantiate_reply(SET_TOKENS, "token_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    assert_eq!(
        TEMPCREATORTOKENADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("token_address")
    );
    assert_eq!(res.messages.len(), 1);

    let updated_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked("token_address"))
    );

    let nft_reply = create_instantiate_reply(MINT_CREATE_POOL, "nft_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    assert_eq!(
        TEMPNFTADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("nft_address")
    );
    assert_eq!(res.messages.len(), 1);

    let updated_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::NftCreated);
    assert_eq!(
        updated_state.mint_new_position_nft_address,
        Some(Addr::unchecked("nft_address"))
    );

    let pool_reply = create_instantiate_reply(FINALIZE_POOL, "pool_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    let creator = Addr::unchecked(ADMIN);
    let commit_info = SETCOMMIT.load(&deps.storage, &creator.to_string()).unwrap();
    assert_eq!(commit_info.pool_id, pool_id);
    assert_eq!(
        commit_info.creator_pool_addr,
        Addr::unchecked("pool_address")
    );

    let pool_by_id = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    assert_eq!(
        pool_by_id.creator_pool_addr,
        Addr::unchecked("pool_address")
    );

    assert!(TEMPPOOLID.load(&deps.storage).is_err());
    assert!(TEMPPOOLINFO.load(&deps.storage).is_err());
    assert!(TEMPCREATORWALLETADDR.load(&deps.storage).is_err());
    assert!(TEMPCREATORTOKENADDR.load(&deps.storage).is_err());
    assert!(TEMPNFTADDR.load(&deps.storage).is_err());

    let final_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(final_state.status, CreationStatus::Completed);
    assert_eq!(
        final_state.pool_address,
        Some(Addr::unchecked("pool_address"))
    );

    assert_eq!(res.messages.len(), 2);
}

#[test]
fn test_asset() {
    let bluechip_asset = TokenInfo {
        info: TokenType::Bluechip {
            denom: "bluechip".to_string(),
        },
        amount: Uint128::new(100),
    };

    let token_asset = TokenInfo {
        info: TokenType::CreatorToken {
            contract_addr: Addr::unchecked("bluechip..."),
        },
        amount: Uint128::new(100),
    };

    assert!(bluechip_asset.is_bluechip_token());
    assert!(!token_asset.is_bluechip_token());
}

#[test]
fn test_config() {
    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("admin1..."),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 1,
        create_pool_wasm_contract_id: 1,
        bluechip_wallet_address: Addr::unchecked("bluechip1..."),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
    };

    assert_eq!(config.factory_admin_address, Addr::unchecked("admin1..."));
    assert_eq!(config.cw20_token_contract_id, 1);
    assert_eq!(config.create_pool_wasm_contract_id, 1);
    assert_eq!(
        config.bluechip_wallet_address,
        Addr::unchecked("bluechip1...")
    );
    assert_eq!(config.commit_fee_bluechip, Decimal::percent(10));
    assert_eq!(config.commit_fee_creator, Decimal::percent(10));
}

#[test]
fn test_update_config() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
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
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    // Try updating with non-admin
    let unauthorized_info = mock_info("unauthorized", &[]);
    let update_msg = ExecuteMsg::UpdateConfig {
        config: FactoryInstantiate {
            factory_admin_address: Addr::unchecked("addr0000"),
            cw721_nft_contract_id: 58,
            commit_amount_for_threshold_bluechip: Uint128::zero(),
            commit_threshold_limit_usd: Uint128::new(100),
            pyth_contract_addr_for_conversions: "oracle0000".to_string(),
            pyth_atom_usd_price_feed_id: "ORCL".to_string(),
            cw20_token_contract_id: 10,
            create_pool_wasm_contract_id: 11,
            bluechip_wallet_address: Addr::unchecked("bluechip"),
            commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
            commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let err = execute(
        deps.as_mut(),
        env.clone(),
        unauthorized_info,
        update_msg.clone(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Generic error: Only the admin can execute this function. Admin: addr0000, Sender: unauthorized"
    );

    // Update config successfully
    let res = execute(deps.as_mut(), env.clone(), info, update_msg).unwrap();
    assert_eq!(1, res.attributes.len());
    assert_eq!(("action", "update_config"), res.attributes[0]);
}

#[test]
fn test_reply_handling() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("addr0000"),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
        commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let pool_id = 1u64;
    TEMPPOOLID.save(deps.as_mut().storage, &pool_id).unwrap();

    let creation_state = PoolCreationState {
        pool_id,
        creator: addr.clone(),
        creator_token_address: None,
        mint_new_position_nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    POOL_CREATION_STATES
        .save(deps.as_mut().storage, pool_id, &creation_state)
        .unwrap();

    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("token0000"),
            },
        ],
        factory_to_create_pool_addr: Addr::unchecked("factory"),
        cw20_token_contract_id: 10,
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip"),
            creator_wallet_address: Addr::unchecked("addr0000"),
            commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
            commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        },
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        creator_token_address: Addr::unchecked("token0000"),
    };

    TEMPPOOLINFO.save(deps.as_mut().storage, &pool_msg).unwrap();
    TEMPCREATORWALLETADDR
        .save(deps.as_mut().storage, &addr)
        .unwrap();

    let contract_addr = "token_contract_address";

    let reply_msg = Reply {
        id: SET_TOKENS,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![
                Event::new("instantiate").add_attribute("_contract_address", contract_addr)
            ],
            data: None,
        }),
    };

    let res = pool_creation_reply(deps.as_mut(), env.clone(), reply_msg).unwrap();

    assert_eq!(res.attributes.len(), 3);
    assert_eq!(res.attributes[0], ("action", "token_created_successfully"));
    assert_eq!(res.attributes[1], ("token_address", contract_addr));
    assert_eq!(res.attributes[2], ("pool_id", "1"));

    let updated_state = POOL_CREATION_STATES
        .load(deps.as_ref().storage, pool_id)
        .unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked(contract_addr))
    );

    let temp_token = TEMPCREATORTOKENADDR.load(deps.as_ref().storage).unwrap();
    assert_eq!(temp_token, Addr::unchecked(contract_addr));
}

#[test]
fn test_oracle_execute_update_price() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool and some creator pools
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = Addr::unchecked(format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Manually set the oracle's last_update to current time to simulate a recent update
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_update = env.block.time.seconds();
    INTERNAL_ORACLE.save(deps.as_mut().storage, &oracle).unwrap();

    // Try to update price immediately (should fail - too soon)
    let update_msg = ExecuteMsg::UpdateOraclePrice {};
    let info = mock_info(ADMIN, &[]);
    let result = execute(deps.as_mut(), env.clone(), info.clone(), update_msg.clone());

    // Should fail because not enough time has passed
    assert!(result.is_err());

    // Fast forward time by 6 minutes (UPDATE_INTERVAL is 5 minutes)
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);

    // Now it should succeed
    let result = execute(deps.as_mut(), future_env.clone(), info, update_msg);
    assert!(result.is_ok());

    let res = result.unwrap();
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "update_oracle"));
    assert!(res.attributes.iter().any(|attr| attr.key == "twap_price"));
    assert!(res.attributes.iter().any(|attr| attr.key == "pools_used"));

    // Verify oracle state was updated
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(oracle.bluechip_price_cache.last_update > 0);
    assert!(!oracle.bluechip_price_cache.twap_observations.is_empty());
}
#[test]
fn test_oracle_force_rotate_pools() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool and multiple creator pools
    setup_atom_pool(&mut deps);

    for i in 1..=10 {
        let pool_addr = Addr::unchecked(format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Store initial pool selection
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let initial_pools = oracle.selected_pools.clone();

    // Try to force rotate as non-admin (should fail)
    let unauthorized_info = mock_info("unauthorized", &[]);
    let rotate_msg = ExecuteMsg::ForceRotateOraclePools {};
    let result = execute(
        deps.as_mut(),
        env.clone(),
        unauthorized_info,
        rotate_msg.clone(),
    );
    assert!(result.is_err());

    // Force rotate as admin (should succeed)
    let admin_info = mock_info(ADMIN, &[]);
    let result = execute(deps.as_mut(), env.clone(), admin_info, rotate_msg);
    assert!(result.is_ok());

    let res = result.unwrap();
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "force_rotate_pools"));
    assert!(res.attributes.iter().any(|attr| attr.key == "pools_count"));

    // Verify pools were rotated
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let new_pools = oracle.selected_pools.clone();

    // ATOM pool should always be present
    assert!(new_pools.contains(&ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()));

    // With 10 creator pools, rotation should potentially select different pools
    // (though there's a chance they're the same due to randomness)
    assert_eq!(new_pools.len(), initial_pools.len());
}
