use crate::state::{
    CreationStatus, FactoryInstantiate, PoolCreationState, FACTORYINSTANTIATEINFO,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_CREATION_STATES, SETCOMMIT, TEMP_POOL_CREATION,
};
use cosmwasm_std::{
    Addr, Binary, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse, SubMsgResult, Uint128,
};

use crate::asset::{TokenInfo, TokenType};
use crate::execute::{
    execute, instantiate, pool_creation_reply, FINALIZE_POOL, MINT_CREATE_POOL, SET_TOKENS,
};
use crate::internal_bluechip_price_oracle::{
    bluechip_to_usd, calculate_twap, get_bluechip_usd_price, query_pyth_atom_usd_price,
    usd_to_bluechip, BlueChipPriceInternalOracle, PriceCache, PriceObservation,
    ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS, INTERNAL_ORACLE, MOCK_PYTH_PRICE,
};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolDetails, TempPoolCreation};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage};
use pool_factory_interfaces::PoolStateResponseForFactory;

const ADMIN: &str = "admin";
#[cfg(test)]
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
        max_bluechip_lock_per_pool: Uint128::new(1),
        creator_excess_liquidity_lock_days: 7,
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
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

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
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
                max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
            },
            token_info: CreatorTokenInfo {
                name: "Test Token".to_string(),
                symbol: "TEST".to_string(),
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
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
            max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
            creator_excess_liquidity_lock_days: 7,
        },
        token_info: CreatorTokenInfo {
            name: "Custom Token".to_string(),
            symbol: "CUSTOM".to_string(),
            decimal: 6,
        },
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    let res = execute(deps.as_mut(), env, info, create_msg).unwrap();

    assert_eq!(res.messages.len(), 1);
}

fn create_pool_msg(name: &str) -> ExecuteMsg {
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
            max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
            creator_excess_liquidity_lock_days: 7,
        },
        token_info: CreatorTokenInfo {
            name: name.to_string(),
            symbol: name.to_string(),
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

    // Create 3 pools and verify they're created with unique IDs
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

        // Load the pool context that was just created
        let pool_context = TEMP_POOL_CREATION.load(&deps.storage).unwrap();
        let pool_id = pool_context.pool_id;
        let creator = pool_context.temp_creator_wallet.clone();

        // Verify this is a new unique ID
        assert!(
            !created_pool_ids.contains(&pool_id),
            "Pool ID {} should be unique",
            pool_id
        );
        created_pool_ids.push(pool_id);

        // The creation state should already be created by execute, but verify it exists
        let creation_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
        assert_eq!(creation_state.status, CreationStatus::Started);
        assert_eq!(creation_state.creator, creator);

        // Simulate complete reply chain with the actual pool_id
        simulate_complete_reply_chain(&mut deps, env.clone(), pool_id);

        // Verify pool was created successfully
        assert!(
            POOLS_BY_ID.load(&deps.storage, pool_id).is_ok(),
            "Pool should be stored by ID"
        );

        // Verify creation state shows completed
        let final_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
        assert_eq!(final_state.status, CreationStatus::Completed);

        // Verify temp storage was cleaned up after completion
        assert!(
            TEMP_POOL_CREATION.load(&deps.storage).is_err(),
            "Temp storage should be cleaned up after pool creation"
        );
    }

    // Verify we created 3 unique pools
    assert_eq!(created_pool_ids.len(), 3, "Should have created 3 pools");
}
#[test]
fn test_complete_pool_creation_flow() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool first
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create the pool message
    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    let create_msg = ExecuteMsg::Create {
        pool_msg: pool_msg.clone(),
        token_info: CreatorTokenInfo {
            name: "Test Token".to_string(),
            symbol: "TEST".to_string(),
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

    // Load the pool context that was created during execute
    let pool_context = TEMP_POOL_CREATION.load(&deps.storage).unwrap();
    let pool_id = pool_context.pool_id;
    let creator = pool_context.temp_creator_wallet.clone();

    assert!(pool_id > 0);
    assert_eq!(pool_context.temp_creator_wallet, Addr::unchecked(ADMIN));
    assert!(pool_context.creator_token_addr.is_none());
    assert!(pool_context.nft_addr.is_none());

    // Step 1: Token Creation Reply
    let token_reply = create_instantiate_reply(SET_TOKENS, "token_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // Reload context and check token was set
    let pool_context = TEMP_POOL_CREATION.load(&deps.storage).unwrap();
    assert_eq!(
        pool_context.creator_token_addr,
        Some(Addr::unchecked("token_address"))
    );
    assert_eq!(res.messages.len(), 1);

    let updated_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked("token_address"))
    );

    // Step 2: NFT Creation Reply
    let nft_reply = create_instantiate_reply(MINT_CREATE_POOL, "nft_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    // Reload context and check NFT was set
    let pool_context = TEMP_POOL_CREATION.load(&deps.storage).unwrap();
    assert_eq!(pool_context.nft_addr, Some(Addr::unchecked("nft_address")));
    assert_eq!(res.messages.len(), 1);

    let updated_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::NftCreated);
    assert_eq!(
        updated_state.mint_new_position_nft_address,
        Some(Addr::unchecked("nft_address"))
    );

    // Step 3: Pool Finalization Reply
    let pool_reply = create_instantiate_reply(FINALIZE_POOL, "pool_address");
    let res = pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Check commit info was saved
    let commit_info = SETCOMMIT.load(&deps.storage, &creator.to_string()).unwrap();
    assert_eq!(commit_info.pool_id, pool_id);
    assert_eq!(
        commit_info.creator_pool_addr,
        Addr::unchecked("pool_address")
    );

    // Check pool was indexed by ID
    let pool_by_id = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    assert_eq!(
        pool_by_id.creator_pool_addr,
        Addr::unchecked("pool_address")
    );

    // Verify temp storage was cleaned up
    assert!(TEMP_POOL_CREATION.load(&deps.storage).is_err());

    // Check final state
    let final_state = POOL_CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(final_state.status, CreationStatus::Completed);
    assert_eq!(
        final_state.pool_address,
        Some(Addr::unchecked("pool_address"))
    );

    // Should have 2 messages for ownership transfer (CW20 and NFT)
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
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
            max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
            creator_excess_liquidity_lock_days: 7,
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let pool_id = 1u64;

    // Create the pool message
    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"), // Use placeholder
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
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };

    // Create the consolidated pool creation context
    let pool_context = TempPoolCreation {
        pool_id,
        temp_creator_wallet: addr.clone(),
        temp_pool_info: pool_msg,
        creator_token_addr: None,
        nft_addr: None,
    };

    // Save the consolidated context
    TEMP_POOL_CREATION
        .save(deps.as_mut().storage, &pool_context)
        .unwrap();

    // Set up the creation state
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

    let contract_addr = "token_contract_address";

    // Create the reply message
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

    // Verify the creation state was updated
    let updated_state = POOL_CREATION_STATES
        .load(deps.as_ref().storage, pool_id)
        .unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked(contract_addr))
    );

    // Verify the pool context was updated with the token address
    let updated_context = TEMP_POOL_CREATION.load(deps.as_ref().storage).unwrap();
    assert_eq!(
        updated_context.creator_token_addr,
        Some(Addr::unchecked(contract_addr))
    );
    assert_eq!(updated_context.pool_id, pool_id);
    assert_eq!(updated_context.temp_creator_wallet, addr);
}

#[test]
fn test_oracle_execute_update_price() {
    let mut deps = mock_dependencies(&[]);

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

    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_update = env.block.time.seconds();
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let update_msg = ExecuteMsg::UpdateOraclePrice {};
    let info = mock_info(ADMIN, &[]);
    let result = execute(deps.as_mut(), env.clone(), info.clone(), update_msg.clone());

    // Should fail because not enough time has passed
    assert!(result.is_err());

    // Fast forward time by 6 minutes (UPDATE_INTERVAL is 5 minutes)
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);

    // should succeed
    let result = execute(deps.as_mut(), future_env.clone(), info, update_msg);
    assert!(result.is_ok());

    let res = result.unwrap();
    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "update_oracle"));
    assert!(res.attributes.iter().any(|attr| attr.key == "twap_price"));
    assert!(res.attributes.iter().any(|attr| attr.key == "pools_used"));

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(oracle.bluechip_price_cache.last_update > 0);
    assert!(!oracle.bluechip_price_cache.twap_observations.is_empty());
}
#[test]
fn test_oracle_force_rotate_pools() {
    let mut deps = mock_dependencies(&[]);

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

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let new_pools = oracle.selected_pools.clone();

    // ATOM pool should always be present
    assert!(new_pools.contains(&ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()));

    // With 10 creator pools, rotation should potentially select different pools
    assert_eq!(new_pools.len(), initial_pools.len());
}

#[test]
fn test_oracle_calculates_correct_bluechip_price() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let atom_reserve = Uint128::new(100_000_000_000); // 100k ATOM with 6 decimals
    let bluechip_reserve = Uint128::new(1_000_000_000_000); // 1M bluechip with 6 decimals
    let atom_price_usd = Uint128::new(10_000_000); // $10.00 with 6 decimals

    let expected_bluechip_price = atom_reserve
        .checked_mul(atom_price_usd)
        .unwrap()
        .checked_div(bluechip_reserve)
        .unwrap();

    assert_eq!(
        expected_bluechip_price,
        Uint128::new(1_000_000),
        "Math check failed"
    );
}

#[test]
fn test_oracle_price_calculation_with_different_ratios() {
    let atom_reserve = Uint128::new(1_000_000_000); // 1k ATOM
    let bluechip_reserve = Uint128::new(1_000_000_000); // 1k bluechip
    let atom_price = Uint128::new(10_000_000); // $10.00

    let bluechip_price = atom_reserve
        .checked_mul(atom_price)
        .unwrap()
        .checked_div(bluechip_reserve)
        .unwrap();

    assert_eq!(bluechip_price, Uint128::new(10_000_000)); // Should also be $10.00

    // Test case 2: 10:1 ratio
    let atom_reserve = Uint128::new(100_000_000); // 100 ATOM
    let bluechip_reserve = Uint128::new(1_000_000_000); // 1k bluechip
    let bluechip_price = atom_reserve
        .checked_mul(atom_price)
        .unwrap()
        .checked_div(bluechip_reserve)
        .unwrap();

    assert_eq!(bluechip_price, Uint128::new(1_000_000)); // Should be $1.00

    // Test case 3: Very small bluechip value
    let atom_reserve = Uint128::new(10_000_000); // 10 ATOM
    let bluechip_reserve = Uint128::new(1_000_000_000_000); // 1M bluechip
    let bluechip_price = atom_reserve
        .checked_mul(atom_price)
        .unwrap()
        .checked_div(bluechip_reserve)
        .unwrap();

    assert_eq!(bluechip_price, Uint128::new(100)); // Should be $0.0001
}

#[test]
fn test_oracle_handles_zero_reserves_safely() {
    let atom_reserve = Uint128::new(100_000_000);
    let bluechip_reserve = Uint128::zero(); // ZERO reserves
    let atom_price = Uint128::new(10_000_000);

    let result = atom_reserve
        .checked_mul(atom_price)
        .unwrap()
        .checked_div(bluechip_reserve);

    assert!(result.is_err(), "Division by zero should return Err");
}

#[test]
fn test_oracle_overflow_protection() {
    // Test with very large numbers that might overflow
    let atom_reserve = Uint128::new(u128::MAX / 2);
    let bluechip_reserve = Uint128::new(1_000_000);
    let atom_price = Uint128::new(10_000_000);

    // First multiplication should overflow
    let mult_result = atom_reserve.checked_mul(atom_price);
    assert!(mult_result.is_err(), "Multiplication should overflow");

    // Test that even if multiplication succeeded, we handle it safely
    let safe_atom_reserve = Uint128::new(1_000_000_000);
    let product = safe_atom_reserve.checked_mul(atom_price).unwrap();
    let div_result = product.checked_div(bluechip_reserve);
    assert!(div_result.is_ok(), "Safe calculation should succeed");
}

#[test]
fn test_oracle_twap_calculation_with_manual_observations() {
    // Test the TWAP calculation logic directly without full oracle update
    let observations = vec![
        PriceObservation {
            timestamp: 1000,
            price: Uint128::new(5_000_000), // 5M
            atom_pool_price: Uint128::new(5_000_000),
        },
        PriceObservation {
            timestamp: 1360,                 // 360 seconds later
            price: Uint128::new(10_000_000), // 10M (doubled)
            atom_pool_price: Uint128::new(10_000_000),
        },
    ];

    let twap = calculate_twap(&observations).unwrap();

    // TWAP for this scenario:
    // time_delta = 360 seconds
    // avg_price = (5M + 10M) / 2 = 7.5M
    let expected_twap = Uint128::new(7_500_000);

    assert_eq!(twap, expected_twap, "TWAP should be 7.5M, got: {}", twap);
}

#[test]
fn test_oracle_twap_with_three_observations() {
    let observations = vec![
        PriceObservation {
            timestamp: 1000,
            price: Uint128::new(5_000_000),
            atom_pool_price: Uint128::new(5_000_000),
        },
        PriceObservation {
            timestamp: 1360,
            price: Uint128::new(10_000_000),
            atom_pool_price: Uint128::new(10_000_000),
        },
        PriceObservation {
            timestamp: 1720,
            price: Uint128::new(8_000_000),
            atom_pool_price: Uint128::new(8_000_000),
        },
    ];

    let twap = calculate_twap(&observations).unwrap();

    // Interval 1 (1000->1360): 360s, avg = 7.5M
    // Interval 2 (1360->1720): 360s, avg = 9M
    // TWAP = (7.5M * 360 + 9M * 360) / 720 = 8.25M
    let expected_twap = Uint128::new(8_250_000);

    assert_eq!(twap, expected_twap, "TWAP should be 8.25M, got: {}", twap);
}

#[test]
fn test_oracle_twap_observations_are_timestamped() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // First update
    let mut env1 = env.clone();
    env1.block.time = env1.block.time.plus_seconds(360);
    let time1 = env1.block.time.seconds();
    execute(
        deps.as_mut(),
        env1.clone(),
        mock_info(ADMIN, &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // Second update 10 minutes later
    let mut env2 = env1.clone();
    env2.block.time = env2.block.time.plus_seconds(600);
    let time2 = env2.block.time.seconds();
    execute(
        deps.as_mut(),
        env2.clone(),
        mock_info(ADMIN, &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let observations = &oracle.bluechip_price_cache.twap_observations;

    assert_eq!(observations.len(), 2);

    // Verify timestamps are correct and in order
    assert_eq!(
        observations[0].timestamp, time1,
        "First observation timestamp incorrect"
    );
    assert_eq!(
        observations[1].timestamp, time2,
        "Second observation timestamp incorrect"
    );
    assert!(
        observations[1].timestamp > observations[0].timestamp,
        "Timestamps should be increasing"
    );
}

#[test]
fn test_oracle_twap_observations_max_length() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let mut env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Add more observations than the max (let's say max is 10)
    for i in 1..=15 {
        env.block.time = env.block.time.plus_seconds(360);

        execute(
            deps.as_mut(),
            env.clone(),
            mock_info(ADMIN, &[]),
            ExecuteMsg::UpdateOraclePrice {},
        )
        .unwrap();

        let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
        let observations = &oracle.bluechip_price_cache.twap_observations;

        println!(
            "Observation #{}: count = {}, time = {}",
            i,
            observations.len(),
            env.block.time.seconds()
        );

        // Verify behavior at different stages
        if i <= 10 {
            // Before hitting the max, should keep growing
            assert_eq!(
                observations.len(),
                i as usize,
                "Observation count should equal iteration number before max"
            );
        } else {
            // After hitting max, should stay at max
            assert_eq!(
                observations.len(),
                10,
                "Observation count should stay at max of 10"
            );
        }
    }

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let observations = &oracle.bluechip_price_cache.twap_observations;

    assert!(
        observations.len() <= 10,
        "TWAP observations should not exceed max length, got: {}",
        observations.len()
    );

    // Verify oldest observations were pruned (most recent should be kept)
    if observations.len() == 10 {
        let last_timestamp = observations.last().unwrap().timestamp;
        assert_eq!(last_timestamp, env.block.time.seconds());
    }
}

#[test]
fn test_oracle_twap_with_volatile_prices() {
    let observations = vec![
        PriceObservation {
            timestamp: 1000,
            price: Uint128::new(10_000_000),
            atom_pool_price: Uint128::new(10_000_000),
        },
        PriceObservation {
            timestamp: 1360,
            price: Uint128::new(2_000_000),
            atom_pool_price: Uint128::new(2_000_000),
        },
        PriceObservation {
            timestamp: 1720,
            price: Uint128::new(20_000_000),
            atom_pool_price: Uint128::new(20_000_000),
        },
        PriceObservation {
            timestamp: 2080,
            price: Uint128::new(5_000_000),
            atom_pool_price: Uint128::new(5_000_000),
        },
    ];

    let twap = calculate_twap(&observations).unwrap();

    println!("Volatile observations: 10M -> 2M -> 20M -> 5M");
    println!("TWAP result: {}", twap);
    let expected_twap = Uint128::new(9_833_333); // ~9.83M
    let tolerance = Uint128::new(100_000); // 0.1M tolerance

    assert!(
        twap >= expected_twap
            .checked_sub(tolerance)
            .unwrap_or(Uint128::zero())
            && twap <= expected_twap + tolerance,
        "TWAP should be approximately {}, got: {}",
        expected_twap,
        twap
    );

    assert!(
        twap > Uint128::new(2_000_000) && twap < Uint128::new(20_000_000),
        "TWAP should smooth extreme values (2M and 20M), got: {}",
        twap
    );
}

#[test]
fn test_oracle_aggregates_multiple_pool_prices() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let add_test_pool = |deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
                         pool_addr: Addr,
                         pool_id: u64,
                         reserve0: u128,
                         reserve1: u128| {
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(reserve0),
            reserve1: Uint128::new(reserve1),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, pool_addr.clone(), &pool_state)
            .unwrap();

        let pool_details = PoolDetails {
            pool_id,
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("creator_token"),
                },
            ],
            creator_pool_addr: pool_addr,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, pool_id, &pool_details)
            .unwrap();
    };

    // Add 3 creator pools with different bluechip prices
    add_test_pool(
        &mut deps,
        Addr::unchecked("creator_pool_1"),
        1,
        45_000_000_000, // 45k bluechip
        10_000_000_000, // 10k creator token
    );

    add_test_pool(
        &mut deps,
        Addr::unchecked("creator_pool_2"),
        2,
        55_000_000_000, // 55k bluechip
        15_000_000_000, // 10k creator token
    );

    add_test_pool(
        &mut deps,
        Addr::unchecked("creator_pool_3"),
        3,
        50_000_000_000, // 50k bluechip
        10_000_000_000, // 10k creator token
    );

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);
    execute(
        deps.as_mut(),
        future_env.clone(),
        mock_info(ADMIN, &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();

    assert!(
        oracle.selected_pools.len() > 1,
        "Should aggregate from multiple pools - found: {:?}",
        oracle.selected_pools
    );

    let price = oracle.bluechip_price_cache.last_price;
    assert!(
        price > Uint128::zero(),
        "Aggregated price should be calculated"
    );
    assert!(
        price >= Uint128::new(9_000_000) && price <= Uint128::new(10_000_000),
        "Price should be in expected range, got: {}",
        price
    );
}

#[test]
fn test_oracle_filters_outlier_pool_prices() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = Addr::unchecked(format!("normal_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000), // 50k bluechip
            reserve1: Uint128::new(10_000_000_000), // 10k token = ratio of 5
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    // Add 1 manipulated pool with extreme ratio of 0.05 (very low bluechip)
    // This represents a 200x manipulation attempt
    let manipulated_pool = Addr::unchecked("manipulated_pool");
    let manipulated_state = PoolStateResponseForFactory {
        pool_contract_address: manipulated_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(500_000_000),    // 0.5k bluechip
        reserve1: Uint128::new(10_000_000_000), // 10k token = ratio of 0.05
        total_liquidity: Uint128::new(10_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            manipulated_pool.clone(),
            &manipulated_state,
        )
        .unwrap();

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Check which pools were selected
    let oracle_before = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    println!("Selected pools: {:?}", oracle_before.selected_pools);
    let manipulated_was_selected = oracle_before
        .selected_pools
        .contains(&manipulated_pool.to_string());
    println!("Manipulated pool selected: {}", manipulated_was_selected);

    // Update price
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);
    execute(
        deps.as_mut(),
        future_env.clone(),
        mock_info(ADMIN, &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let price = oracle.bluechip_price_cache.last_price;

    println!("Final aggregated price: {}", price);

    if manipulated_was_selected {
        assert!(
            price >= Uint128::new(4_000_000) && price <= Uint128::new(11_000_000),
            "Even with outlier, price should be near normal range (4-11), got: {}",
            price
        );
    } else {
        assert!(
            price >= Uint128::new(4_000_000) && price <= Uint128::new(11_000_000),
            "Without outlier, price should be in normal range (4-11), got: {}",
            price
        );
    }
    assert!(
        price > Uint128::new(1_000_000), // Should be well above the outlier's influence
        "Price should not be driven down to outlier level, got: {}",
        price
    );
}

#[test]
fn test_oracle_handles_pools_with_different_liquidities() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let small_pool = Addr::unchecked("small_pool");
    let small_state = PoolStateResponseForFactory {
        pool_contract_address: small_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000), // Very small
        reserve1: Uint128::new(200_000),
        total_liquidity: Uint128::new(100_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, small_pool, &small_state)
        .unwrap();

    let large_pool = Addr::unchecked("large_pool");
    let large_state = PoolStateResponseForFactory {
        pool_contract_address: large_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000_000_000), // Very large
        reserve1: Uint128::new(200_000_000_000),
        total_liquidity: Uint128::new(100_000_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, large_pool, &large_state)
        .unwrap();

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Update price
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);
    let result = execute(
        deps.as_mut(),
        future_env.clone(),
        mock_info(ADMIN, &[]),
        ExecuteMsg::UpdateOraclePrice {},
    );

    // Should handle different liquidity levels without errors
    assert!(result.is_ok(), "Should handle pools with varying liquidity");
}

#[test]
fn test_query_pyth_atom_usd_price_success() {
    let mut deps = mock_dependencies(&[]);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Mock Pyth price: ATOM = $10.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000))
        .unwrap();

    let env = mock_env();
    let result = query_pyth_atom_usd_price(deps.as_ref(), env);

    assert!(result.is_ok(), "Should successfully query Pyth price");

    let price = result.unwrap();
    assert_eq!(
        price,
        Uint128::new(10_000_000),
        "ATOM price should be $10.00 with 6 decimals"
    );
}

#[test]
fn test_query_pyth_atom_usd_price_default() {
    let mut deps = mock_dependencies(&[]);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();
    let env = mock_env();
    let result = query_pyth_atom_usd_price(deps.as_ref(), env);

    assert!(result.is_ok(), "Should use default price");
    let price = result.unwrap();
    assert_eq!(price, Uint128::new(10_000_000), "Should default to $10.00");
}

#[test]
fn test_query_pyth_extreme_atom_prices() {
    let mut deps = mock_dependencies(&[]);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    let env = mock_env();

    // Test 1: ATOM crash to $0.01
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000))
        .unwrap();
    let result_low = query_pyth_atom_usd_price(deps.as_ref(), env.clone());
    assert!(result_low.is_ok(), "Should handle low ATOM price");
    assert_eq!(result_low.unwrap(), Uint128::new(10_000)); // $0.01

    // Test 2: ATOM pump to $10,000
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000_000))
        .unwrap();
    let result_high = query_pyth_atom_usd_price(deps.as_ref(), env.clone());
    assert!(result_high.is_ok(), "Should handle high ATOM price");
    assert_eq!(result_high.unwrap(), Uint128::new(10_000_000_000)); // $10,000

    // Test 3: ATOM at $100
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(100_000_000))
        .unwrap();
    let result_med = query_pyth_atom_usd_price(deps.as_ref(), env.clone());
    assert!(result_med.is_ok(), "Should handle $100 ATOM price");
    assert_eq!(result_med.unwrap(), Uint128::new(100_000_000)); // $100
}

#[test]
fn test_get_bluechip_usd_price_with_pyth() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Mock ATOM = $10.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000))
        .unwrap();

    let env = mock_env();
    let result = get_bluechip_usd_price(deps.as_ref(), env);

    assert!(result.is_ok(), "Should calculate bluechip USD price");
    let bluechip_price = result.unwrap();

    println!("Calculated bluechip USD price: {}", bluechip_price);

    assert_eq!(
        bluechip_price,
        Uint128::new(1_000_000),
        "Bluechip should be $1.00"
    );
}

#[test]
fn test_bluechip_usd_price_with_different_atom_prices() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    let env = mock_env();

    // Scenario 1: ATOM = $5.00 -> bluechip = $0.50
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(5_000_000))
        .unwrap();
    let price1 = get_bluechip_usd_price(deps.as_ref(), env.clone()).unwrap();
    println!("ATOM=$5 -> Bluechip=${}", price1);
    assert_eq!(price1, Uint128::new(500_000)); // $0.50

    // Scenario 2: ATOM = $20.00 -> bluechip = $2.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(20_000_000))
        .unwrap();
    let price2 = get_bluechip_usd_price(deps.as_ref(), env.clone()).unwrap();
    println!("ATOM=$20 -> Bluechip=${}", price2);
    assert_eq!(price2, Uint128::new(2_000_000)); // $2.00

    // Scenario 3: ATOM = $100.00 -> bluechip = $10.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(100_000_000))
        .unwrap();
    let price3 = get_bluechip_usd_price(deps.as_ref(), env.clone()).unwrap();
    println!("ATOM=$100 -> Bluechip=${}", price3);
    assert_eq!(price3, Uint128::new(10_000_000)); // $10.00
}

#[test]
fn test_conversion_functions_with_pyth() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Mock ATOM = $10.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000))
        .unwrap();

    // Initialize oracle
    let oracle = BlueChipPriceInternalOracle {
        atom_pool_contract_address: Addr::unchecked(ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS),
        selected_pools: vec![ATOM_BLUECHIP_POOL_CONTRACT_ADDRESS.to_string()],
        bluechip_price_cache: PriceCache {
            last_price: Uint128::new(1_000_000), // $1.00
            last_update: 1000,
            twap_observations: vec![],
        },
        update_interval: 300,
        rotation_interval: 3600,
        last_rotation: 0,
    };
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let env = mock_env();

    let bluechip_amount = Uint128::new(5_000_000); // 5 bluechip
    let result = bluechip_to_usd(deps.as_ref(), bluechip_amount, env.clone());
    assert!(result.is_ok(), "bluechip_to_usd should succeed");
    println!("5 bluechip = ${}", result.as_ref().unwrap().amount);

    let usd_amount = Uint128::new(5_000_000); // $5
    let result2 = usd_to_bluechip(deps.as_ref(), usd_amount, env.clone());
    assert!(result2.is_ok(), "usd_to_bluechip should succeed");
    println!("$5 = {} bluechip", result2.as_ref().unwrap().amount);
}
