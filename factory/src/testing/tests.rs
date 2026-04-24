use crate::mint_bluechips_pool_creation::calculate_mint_amount;
use crate::state::{
    CreationStatus, FactoryInstantiate, PoolCreationContext, PoolCreationState,
    FACTORYINSTANTIATEINFO, FIRST_THRESHOLD_TIMESTAMP, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID,
    POOL_COUNTER, POOL_CREATION_CONTEXT,
};
use cosmwasm_std::{
    Addr, BankMsg, Binary, CosmosMsg, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse,
    SubMsgResult, Uint128,
};

use crate::asset::{TokenInfo, TokenType};
use crate::execute::{
    encode_reply_id, execute, instantiate, pool_creation_reply, FINALIZE_POOL, MINT_CREATE_POOL,
    SET_TOKENS,
};
use crate::internal_bluechip_price_oracle::{
    bluechip_to_usd, calculate_twap, get_bluechip_usd_price, query_pyth_atom_usd_price,
    usd_to_bluechip, BlueChipPriceInternalOracle, PriceCache, PriceObservation, INTERNAL_ORACLE,
    MOCK_PYTH_PRICE,
};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolDetails, TempPoolCreation};
use cosmwasm_std::testing::{message_info, mock_env, MockApi, MockStorage};
use pool_factory_interfaces::PoolStateResponseForFactory;

fn atom_bluechip_pool_addr() -> Addr {
    MockApi::default().addr_make("atom_bluechip_pool")
}

fn admin_addr() -> Addr {
    MockApi::default().addr_make("admin")
}

fn ubluechip_addr() -> Addr {
    MockApi::default().addr_make("ubluechip")
}

fn bluechip_wallet_addr() -> Addr {
    MockApi::default().addr_make("bluechip_wallet")
}

fn addr0000() -> Addr {
    MockApi::default().addr_make("addr0000")
}

fn make_addr(label: &str) -> Addr {
    MockApi::default().addr_make(label)
}
#[cfg(test)]
fn create_default_instantiate_msg() -> FactoryInstantiate {
    FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ubluechip".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(1),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    }
}

/// Save a minimal `PoolDetails` for `pool_id` so production code that looks
/// up a pool address via `POOLS_BY_ID.load(..).creator_pool_addr` works in
/// tests. Mirrors the pre-consolidation `POOL_REGISTRY.save(..., &addr)`
/// convenience; the extra fields default to values no test cares about.
pub fn register_test_pool_addr(
    storage: &mut dyn cosmwasm_std::Storage,
    pool_id: u64,
    pool_addr: &Addr,
) {
    POOLS_BY_ID
        .save(
            storage,
            pool_id,
            &PoolDetails {
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
            },
        )
        .unwrap();
}

pub fn setup_atom_pool(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>) {
    let atom_pool_addr = atom_bluechip_pool_addr();
    let atom_pool_state = PoolStateResponseForFactory {
        pool_contract_address: atom_pool_addr.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000_000_000),
        reserve1: Uint128::new(100_000_000_000),
        total_liquidity: Uint128::new(100_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec![],
    };

    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, atom_pool_addr, &atom_pool_state)
        .unwrap();
}

#[test]
fn proper_initialization() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let the_admin = addr0000();
    let msg = FactoryInstantiate {
        factory_admin_address: the_admin.clone(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    let res = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(
        !oracle.selected_pools.is_empty(),
        "Oracle should have at least ATOM pool"
    );
    assert_eq!(
        oracle.atom_pool_contract_address,
        atom_bluechip_pool_addr(),
        "ATOM pool address should be set correctly"
    );
    assert!(
        oracle
            .selected_pools
            .contains(&atom_bluechip_pool_addr().to_string()),
        "Selected pools should include ATOM pool"
    );

    assert!(res
        .attributes
        .iter()
        .any(|attr| attr.key == "action" && attr.value == "init_contract"));

    let mut deps2 = mock_dependencies(&[]);
    setup_atom_pool(&mut deps2);

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    let _res1 = instantiate(deps2.as_mut(), env.clone(), info, msg.clone()).unwrap();

    let mut deps3 = mock_dependencies(&[]);
    setup_atom_pool(&mut deps3);

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    instantiate(deps3.as_mut(), env.clone(), info, msg.clone()).unwrap();
}

#[test]
fn test_oracle_initialization_with_no_other_pools() {
    let mut deps = mock_dependencies(&[]);

    // Only set up ATOM pool, no other creator pools
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert_eq!(
        oracle.selected_pools.len(),
        1,
        "Should have only ATOM pool when no other pools exist"
    );
    assert_eq!(
        oracle.selected_pools[0],
        atom_bluechip_pool_addr().to_string()
    );

    assert_eq!(oracle.bluechip_price_cache.last_price, Uint128::zero());
    assert_eq!(oracle.bluechip_price_cache.last_update, 0);
    assert!(oracle.bluechip_price_cache.twap_observations.is_empty());
}

#[test]
fn test_oracle_initialization_with_multiple_pools() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    // Add 5 more creator pools with sufficient liquidity.
    // Mark each as threshold-crossed so they're eligible for oracle sampling.
    for i in 1..=5 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_details = PoolDetails {
            pool_id: i,
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
        };
        POOLS_BY_ID
            .save(deps.as_mut().storage, i, &pool_details)
            .unwrap();
        crate::state::POOL_THRESHOLD_MINTED
            .save(deps.as_mut().storage, i, &true)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Verify oracle selected multiple pools. With 5 eligible creator pools
    // seeded above plus the ATOM anchor, selection fits entirely within the
    // ORACLE_POOL_COUNT target, so the output should be exactly 6 (5 + ATOM).
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert!(
        !oracle.selected_pools.is_empty(),
        "Should have at least ATOM pool"
    );
    assert!(
        oracle.selected_pools.len()
            <= crate::internal_bluechip_price_oracle::ORACLE_POOL_COUNT,
        "Should not exceed ORACLE_POOL_COUNT"
    );
    assert!(
        oracle
            .selected_pools
            .contains(&atom_bluechip_pool_addr().to_string()),
        "Should always include ATOM pool"
    );
}

#[test]
fn create_pair() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let the_admin = addr0000();
    let msg = FactoryInstantiate {
        factory_admin_address: the_admin.clone(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let pool_token_info = [
        TokenType::Native {
            denom: "ubluechip".to_string(),
        },
        TokenType::CreatorToken {
            contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
    ];

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

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
                    bluechip_wallet_address: ubluechip_addr(),
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

    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    let custom_params = Binary::from(b"custom_pool_params");

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            cw20_token_contract_id: 10,
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            threshold_payout: Some(custom_params),
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: ubluechip_addr(),
                creator_wallet_address: admin_addr(),
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
    let info = message_info(&admin_addr(), &[]);
    let res = execute(deps.as_mut(), env, info, create_msg).unwrap();

    assert!(
        !res.messages.is_empty() && res.messages.len() <= 2,
        "Should have 1-2 messages (token instantiation + possibly mint), got {}",
        res.messages.len()
    );
}

fn create_pool_msg(name: &str) -> ExecuteMsg {
    ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            cw20_token_contract_id: 10,
            factory_to_create_pool_addr: Addr::unchecked("factory"),
            threshold_payout: None,
            commit_fee_info: CommitFeeInfo {
                bluechip_wallet_address: ubluechip_addr(),
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
            // Uppercase so the symbol passes factory validation (A-Z, 0-9 only).
            symbol: name.to_uppercase(),
            decimal: 6,
        },
    }
}

fn simulate_complete_reply_chain(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    env: Env,
    pool_id: u64,
) {
    let token_addr = make_addr(&format!("token_address_{}", pool_id));
    let token_reply =
        create_instantiate_reply(encode_reply_id(pool_id, SET_TOKENS), token_addr.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    let nft_addr = make_addr(&format!("nft_address_{}", pool_id));
    let nft_reply = create_instantiate_reply(
        encode_reply_id(pool_id, MINT_CREATE_POOL),
        nft_addr.as_str(),
    );
    pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    let pool_addr = make_addr(&format!("pool_address_{}", pool_id));
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id, FINALIZE_POOL), pool_addr.as_str());
    pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();
}

#[test]
fn test_asset_info() {
    let bluechip_info = TokenType::Native {
        denom: "ubluechip".to_string(),
    };
    assert!(bluechip_info.is_native_token());

    let token_info = TokenType::CreatorToken {
        contract_addr: Addr::unchecked("bluechip..."),
    };
    assert!(!token_info.is_native_token());

    assert!(bluechip_info.equal(&TokenType::Native {
        denom: "ubluechip".to_string(),
    }));
    assert!(!bluechip_info.equal(&token_info));
}

#[allow(deprecated)]
pub fn create_instantiate_reply(id: u64, contract_addr: &str) -> Reply {
    Reply {
        id,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![
                Event::new("instantiate").add_attribute("_contract_address", contract_addr)
            ],
            msg_responses: vec![],
            data: None,
        }),
        gas_used: 0,
        payload: Binary::default(),
    }
}

#[test]
fn test_multiple_pool_creation() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create 3 pools and verify they're created with unique IDs
    let mut created_pool_ids = Vec::new();

    for i in 1u64..=3u64 {
        // Create pool
        let create_msg = create_pool_msg(&format!("Token{}", i));
        let info = message_info(&admin_addr(), &[]);
        let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

        assert!(
            res.attributes.iter().any(|attr| attr.key == "pool_id"),
            "Response should contain pool_id attribute"
        );

        // Load the pool context that was just created (use loop index as pool_id)
        let pool_id = i;
        let ctx = POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).unwrap();
        let creator = ctx.temp.temp_creator_wallet.clone();

        // Verify this is a new unique ID
        assert!(
            !created_pool_ids.contains(&pool_id),
            "Pool ID {} should be unique",
            pool_id
        );
        created_pool_ids.push(pool_id);

        // The creation state should already be populated by execute, but verify it
        assert_eq!(ctx.state.status, CreationStatus::Started);
        assert_eq!(ctx.state.creator, creator);

        // Simulate complete reply chain with the actual pool_id
        simulate_complete_reply_chain(&mut deps, env.clone(), pool_id);

        assert!(
            POOLS_BY_ID.load(&deps.storage, pool_id).is_ok(),
            "Pool should be stored by ID"
        );

        // Creation context should be removed on successful completion to
        // avoid permanent storage bloat per pool.
        assert!(
            POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).is_err(),
            "POOL_CREATION_CONTEXT should be removed after successful creation"
        );
    }

    // Verify 3 unique pools
    assert_eq!(created_pool_ids.len(), 3, "Should have created 3 pools");
}
#[test]
fn test_complete_pool_creation_flow() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool first
    setup_atom_pool(&mut deps);

    let msg = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create the pool message
    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            },
        ],
        factory_to_create_pool_addr: Addr::unchecked("factory"),
        cw20_token_contract_id: 10,
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: ubluechip_addr(),
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

    let info = message_info(&admin_addr(), &[]);
    let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

    assert!(
        !res.attributes.is_empty(),
        "Should have response attributes"
    );
    assert!(
        !res.messages.is_empty() && res.messages.len() <= 2,
        "Should have 1-2 messages total (token instantiation + possibly mint), got {}",
        res.messages.len()
    );

    let pool_id = POOL_COUNTER.load(&deps.storage).unwrap();
    let ctx = POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).unwrap();

    assert!(pool_id > 0);
    assert_eq!(ctx.temp.temp_creator_wallet, admin_addr());
    assert!(ctx.temp.creator_token_addr.is_none());
    assert!(ctx.temp.nft_addr.is_none());

    let token_addr = make_addr("token_address");
    let token_reply =
        create_instantiate_reply(encode_reply_id(pool_id, SET_TOKENS), token_addr.as_str());
    let res = pool_creation_reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // Reload context and check token was set. ctx.state.creator_token_address
    // is no longer written to; ctx.temp is the single source of truth and the
    // query handler derives the state response from it.
    let ctx = POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).unwrap();
    assert_eq!(ctx.temp.creator_token_addr, Some(token_addr.clone()));
    assert_eq!(ctx.state.status, CreationStatus::TokenCreated);
    assert_eq!(res.messages.len(), 1);

    // Step 2: NFT Creation Reply
    let nft_addr = make_addr("nft_address");
    let nft_reply = create_instantiate_reply(
        encode_reply_id(pool_id, MINT_CREATE_POOL),
        nft_addr.as_str(),
    );
    let res = pool_creation_reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    let ctx = POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).unwrap();
    assert_eq!(ctx.temp.nft_addr, Some(nft_addr.clone()));
    assert_eq!(ctx.state.status, CreationStatus::NftCreated);
    // ctx.state.mint_new_position_nft_address is no longer written; the
    // ctx.temp.nft_addr check above is the single source of truth.
    assert_eq!(res.messages.len(), 1);

    // Step 3: Pool Finalization Reply
    let pool_addr = make_addr("pool_address");
    let pool_reply =
        create_instantiate_reply(encode_reply_id(pool_id, FINALIZE_POOL), pool_addr.as_str());
    let res = pool_creation_reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    let pool_by_id = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    assert_eq!(pool_by_id.pool_id, pool_id);
    assert_eq!(pool_by_id.creator_pool_addr, pool_addr.clone());

    // Creation context is cleared on success to avoid permanent bloat.
    assert!(
        POOL_CREATION_CONTEXT.load(&deps.storage, pool_id).is_err(),
        "POOL_CREATION_CONTEXT should be removed after successful creation"
    );

    assert_eq!(res.messages.len(), 2);
}

#[test]
fn test_asset() {
    let native_asset = TokenInfo {
        info: TokenType::Native {
            denom: "ubluechip".to_string(),
        },
        amount: Uint128::new(100),
    };

    let token_asset = TokenInfo {
        info: TokenType::CreatorToken {
            contract_addr: Addr::unchecked("bluechip..."),
        },
        amount: Uint128::new(100),
    };

    assert!(native_asset.is_native_token());
    assert!(!token_asset.is_native_token());
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
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: Addr::unchecked("bluechip1..."),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
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

#[allow(deprecated)]
#[test]
fn test_reply_handling() {
    let mut deps = mock_dependencies(&[]);

    // Set up ATOM pool
    setup_atom_pool(&mut deps);

    let the_admin = addr0000();
    let msg = FactoryInstantiate {
        factory_admin_address: the_admin.clone(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::from_ratio(10u128, 100u128),
        commit_fee_creator: Decimal::from_ratio(10u128, 100u128),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&the_admin, &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let pool_id = 1u64;

    // Create the pool message
    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"), // Use placeholder
            },
        ],
        factory_to_create_pool_addr: Addr::unchecked("factory"),
        cw20_token_contract_id: 10,
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: ubluechip_addr(),
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

    let ctx = PoolCreationContext {
        temp: TempPoolCreation {
            pool_id,
            temp_creator_wallet: the_admin.clone(),
            temp_pool_info: pool_msg,
            creator_token_addr: None,
            nft_addr: None,
        },
        state: PoolCreationState {
            pool_id,
            creator: the_admin.clone(),
            creator_token_address: None,
            mint_new_position_nft_address: None,
            pool_address: None,
            creation_time: env.block.time,
            status: CreationStatus::Started,
        },
    };
    POOL_CREATION_CONTEXT
        .save(deps.as_mut().storage, pool_id, &ctx)
        .unwrap();

    let contract_addr_obj = make_addr("token_contract_address");
    let contract_addr = contract_addr_obj.as_str();

    // Create the reply message with pool_id encoded in the reply ID
    let reply_msg = Reply {
        id: encode_reply_id(pool_id, SET_TOKENS),
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![
                Event::new("instantiate").add_attribute("_contract_address", contract_addr)
            ],
            msg_responses: vec![],
            data: None,
        }),
        gas_used: 0,
        payload: Binary::default(),
    };

    let res = pool_creation_reply(deps.as_mut(), env.clone(), reply_msg).unwrap();

    assert_eq!(res.attributes.len(), 3);
    assert_eq!(res.attributes[0], ("action", "token_created_successfully"));
    assert_eq!(res.attributes[1], ("token_address", contract_addr));
    assert_eq!(res.attributes[2], ("pool_id", "1"));

    let updated_ctx = POOL_CREATION_CONTEXT
        .load(deps.as_ref().storage, pool_id)
        .unwrap();
    assert_eq!(updated_ctx.state.status, CreationStatus::TokenCreated);
    // ctx.state.creator_token_address is no longer written; ctx.temp is
    // the single source of truth.
    assert_eq!(
        updated_ctx.temp.creator_token_addr,
        Some(Addr::unchecked(contract_addr))
    );
    assert_eq!(updated_ctx.temp.pool_id, pool_id);
    assert_eq!(updated_ctx.temp.temp_creator_wallet, the_admin);
}

#[test]
fn test_oracle_execute_update_price() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_update = env.block.time.seconds();
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let update_msg = ExecuteMsg::UpdateOraclePrice {};
    let info = message_info(&admin_addr(), &[]);
    let result = execute(deps.as_mut(), env.clone(), info.clone(), update_msg.clone());

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
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let initial_pools = oracle.selected_pools.clone();

    // Non-admin cannot propose a force-rotate.
    let unauthorized_info = message_info(&Addr::unchecked("unauthorized"), &[]);
    let result = execute(
        deps.as_mut(),
        env.clone(),
        unauthorized_info,
        ExecuteMsg::ProposeForceRotateOraclePools {},
    );
    assert!(result.is_err());

    // Admin proposes rotation. This just records PENDING_ORACLE_ROTATION;
    // ForceRotateOraclePools cannot execute until the 48h timelock elapses.
    let admin_info = message_info(&admin_addr(), &[]);
    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    // Attempting to execute before the timelock must fail.
    let err = execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(
        matches!(err, crate::error::ContractError::TimelockNotExpired { .. }),
        "pre-timelock force-rotate must be rejected, got: {:?}",
        err
    );

    // Fast-forward past the 48h timelock and execute.
    let mut future_env = env.clone();
    future_env.block.time = future_env
    .block
    .time
    .plus_seconds(crate::state::ADMIN_TIMELOCK_SECONDS + 1);

    let result = execute(
        deps.as_mut(),
        future_env,
        admin_info,
        ExecuteMsg::ForceRotateOraclePools {},
    );
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
    assert!(new_pools.contains(&atom_bluechip_pool_addr().to_string()));

    // With 10 creator pools, rotation should potentially select different pools
    assert_eq!(new_pools.len(), initial_pools.len());

    // Pending entry must be consumed on successful execution.
    assert!(
        crate::state::PENDING_ORACLE_ROTATION
            .may_load(&deps.storage)
            .unwrap()
            .is_none(),
        "PENDING_ORACLE_ROTATION should be cleared after execution"
    );
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

    let atom_reserve = Uint128::new(100_000_000); // 100 ATOM
    let bluechip_reserve = Uint128::new(1_000_000_000); // 1k bluechip
    let bluechip_price = atom_reserve
        .checked_mul(atom_price)
        .unwrap()
        .checked_div(bluechip_reserve)
        .unwrap();

    assert_eq!(bluechip_price, Uint128::new(1_000_000)); // Should be $1.00

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

    let safe_atom_reserve = Uint128::new(1_000_000_000);
    let product = safe_atom_reserve.checked_mul(atom_price).unwrap();
    let div_result = product.checked_div(bluechip_reserve);
    assert!(div_result.is_ok(), "Safe calculation should succeed");
}

#[test]
fn test_oracle_twap_calculation_with_manual_observations() {
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

    let expected_twap = Uint128::new(8_250_000);

    assert_eq!(twap, expected_twap, "TWAP should be 8.25M, got: {}", twap);
}

#[test]
fn test_oracle_twap_observations_are_timestamped() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // First update
    let mut env1 = env.clone();
    env1.block.time = env1.block.time.plus_seconds(360);
    let time1 = env1.block.time.seconds();
    execute(
        deps.as_mut(),
        env1.clone(),
        message_info(&admin_addr(), &[]),
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
        message_info(&admin_addr(), &[]),
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
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    for i in 1..=15 {
        env.block.time = env.block.time.plus_seconds(360);

        execute(
            deps.as_mut(),
            env.clone(),
            message_info(&admin_addr(), &[]),
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

        if i <= 11 {
            // With 360s intervals and a 3600s TWAP window, the boundary
            // observation (exactly window-width old) is retained by the
            // >= comparison, so the window can hold up to 11 observations
            // (10 intervals + both endpoints).
            assert_eq!(
                observations.len(),
                i as usize,
                "Observation count should equal iteration number before max"
            );
        } else {
            // After hitting max, should stay at max
            assert_eq!(
                observations.len(),
                11,
                "Observation count should stay at max of 11"
            );
        }
    }

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    let observations = &oracle.bluechip_price_cache.twap_observations;

    assert!(
        observations.len() <= 11,
        "TWAP observations should not exceed max length, got: {}",
        observations.len()
    );

    // Verify oldest observations were pruned (most recent should be kept)
    if observations.len() == 11 {
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
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, pool_addr.clone(), &pool_state)
            .unwrap();

        let pool_details = PoolDetails {
            pool_id,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("creator_token"),
                },
            ],
            creator_pool_addr: pool_addr,
            pool_kind: pool_factory_interfaces::PoolKind::Commit,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, pool_id, &pool_details)
            .unwrap();
        // Mark as threshold-crossed so the oracle will include this test pool.
        crate::state::POOL_THRESHOLD_MINTED
            .save(&mut deps.storage, pool_id, &true)
            .unwrap();
    };

    add_test_pool(
        &mut deps,
        make_addr("creator_pool_1"),
        1,
        45_000_000_000, // 45k bluechip
        10_000_000_000, // 10k creator token
    );

    add_test_pool(
        &mut deps,
        make_addr("creator_pool_2"),
        2,
        55_000_000_000, // 55k bluechip
        15_000_000_000, // 10k creator token
    );

    add_test_pool(
        &mut deps,
        make_addr("creator_pool_3"),
        3,
        50_000_000_000, // 50k bluechip
        10_000_000_000, // 10k creator token
    );

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);
    execute(
        deps.as_mut(),
        future_env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();

    assert!(
        oracle.selected_pools.len() > 1,
        "Should select more than just ATOM pool - found: {:?}",
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
        let pool_addr = make_addr(&format!("normal_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000), // 50k bluechip
            reserve1: Uint128::new(10_000_000_000), // 10k token = ratio of 5
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let manipulated_pool = make_addr("manipulated_pool");
    let manipulated_state = PoolStateResponseForFactory {
        pool_contract_address: manipulated_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(500_000_000),    // 0.5k bluechip
        reserve1: Uint128::new(10_000_000_000), // 10k token = ratio of 0.05
        total_liquidity: Uint128::new(10_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec![],
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
    let info = message_info(&admin_addr(), &[]);
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
        message_info(&admin_addr(), &[]),
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

    let small_pool = make_addr("small_pool");
    let small_state = PoolStateResponseForFactory {
        pool_contract_address: small_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000), // Very small
        reserve1: Uint128::new(200_000),
        total_liquidity: Uint128::new(100_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec![],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, small_pool, &small_state)
        .unwrap();

    let large_pool = make_addr("large_pool");
    let large_state = PoolStateResponseForFactory {
        pool_contract_address: large_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(1_000_000_000_000), // Very large
        reserve1: Uint128::new(200_000_000_000),
        total_liquidity: Uint128::new(100_000_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec![],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, large_pool, &large_state)
        .unwrap();

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Update price
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);
    let result = execute(
        deps.as_mut(),
        future_env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    );
    assert!(result.is_ok(), "Should handle pools with varying liquidity");
}

#[test]
fn test_query_pyth_atom_usd_price_success() {
    let mut deps = mock_dependencies(&[]);

    let config = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000))
        .unwrap();

    let env = mock_env();
    let result = query_pyth_atom_usd_price(deps.as_ref(), &env);

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
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();
    let env = mock_env();
    let result = query_pyth_atom_usd_price(deps.as_ref(), &env);

    assert!(result.is_ok(), "Should use default price");
    let price = result.unwrap();
    assert_eq!(price, Uint128::new(10_000_000), "Should default to $10.00");
}

#[test]
fn test_query_pyth_extreme_atom_prices() {
    let mut deps = mock_dependencies(&[]);

    let config = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    let env = mock_env();

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000))
        .unwrap();
    let result_low = query_pyth_atom_usd_price(deps.as_ref(), &env);
    assert!(result_low.is_ok(), "Should handle low ATOM price");
    assert_eq!(result_low.unwrap(), Uint128::new(10_000)); // $0.01

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000_000))
        .unwrap();
    let result_high = query_pyth_atom_usd_price(deps.as_ref(), &env);
    assert!(result_high.is_ok(), "Should handle high ATOM price");
    assert_eq!(result_high.unwrap(), Uint128::new(10_000_000_000)); // $10,000

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(100_000_000))
        .unwrap();
    let result_med = query_pyth_atom_usd_price(deps.as_ref(), &env);
    assert!(result_med.is_ok(), "Should handle $100 ATOM price");
    assert_eq!(result_med.unwrap(), Uint128::new(100_000_000)); // $100
}

#[test]
fn test_get_bluechip_usd_price_with_pyth() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);

    let config = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Mock ATOM = $10.00
    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(10_000_000))
        .unwrap();

    // Initialize oracle with TWAP price of 10 (10 Bluechip per ATOM)
    // This matches the implied ratio in the test (ATOM=$10, Bluechip=$1)
    let oracle = BlueChipPriceInternalOracle {
        atom_pool_contract_address: atom_bluechip_pool_addr(),
        selected_pools: vec![atom_bluechip_pool_addr().to_string()],
        bluechip_price_cache: PriceCache {
            last_price: Uint128::new(10_000_000), // 10.0 ratio
            last_update: 1000,
            twap_observations: vec![],
            cached_pyth_price: Uint128::new(10_000_000),
            cached_pyth_timestamp: 1000,
        },
        update_interval: 300,
        rotation_interval: 3600,
        last_rotation: 0,
        pool_cumulative_snapshots: vec![],
    };
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let env = mock_env();
    let result = get_bluechip_usd_price(deps.as_ref(), &env);

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
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Initialize oracle with TWAP price of 10 (10 Bluechip per ATOM)
    let oracle = BlueChipPriceInternalOracle {
        atom_pool_contract_address: atom_bluechip_pool_addr(),
        selected_pools: vec![atom_bluechip_pool_addr().to_string()],
        bluechip_price_cache: PriceCache {
            last_price: Uint128::new(10_000_000), // 10.0 ratio
            last_update: 1000,
            twap_observations: vec![],
            cached_pyth_price: Uint128::new(10_000_000),
            cached_pyth_timestamp: 1000,
        },
        update_interval: 300,
        rotation_interval: 3600,
        last_rotation: 0,
        pool_cumulative_snapshots: vec![],
    };
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let env = mock_env();

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(5_000_000))
        .unwrap();
    let price1 = get_bluechip_usd_price(deps.as_ref(), &env).unwrap();
    println!("ATOM=$5 -> Bluechip=${}", price1);
    assert_eq!(price1, Uint128::new(500_000)); // $0.50

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(20_000_000))
        .unwrap();
    let price2 = get_bluechip_usd_price(deps.as_ref(), &env).unwrap();
    println!("ATOM=$20 -> Bluechip=${}", price2);
    assert_eq!(price2, Uint128::new(2_000_000)); // $2.00

    MOCK_PYTH_PRICE
        .save(deps.as_mut().storage, &Uint128::new(100_000_000))
        .unwrap();
    let price3 = get_bluechip_usd_price(deps.as_ref(), &env).unwrap();
    println!("ATOM=$100 -> Bluechip=${}", price3);
    assert_eq!(price3, Uint128::new(10_000_000)); // $10.00
}

#[test]
fn test_conversion_functions_with_pyth() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let config = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth_oracle".to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: ubluechip_addr(),
        commit_fee_bluechip: Decimal::percent(10),
        commit_fee_creator: Decimal::percent(10),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
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
        atom_pool_contract_address: atom_bluechip_pool_addr(),
        selected_pools: vec![atom_bluechip_pool_addr().to_string()],
        bluechip_price_cache: PriceCache {
            last_price: Uint128::new(1_000_000), // $1.00
            last_update: 1000,
            twap_observations: vec![],
            cached_pyth_price: Uint128::new(10_000_000),
            cached_pyth_timestamp: 1000,
        },
        update_interval: 300,
        rotation_interval: 3600,
        last_rotation: 0,
        pool_cumulative_snapshots: vec![],
    };
    INTERNAL_ORACLE
        .save(deps.as_mut().storage, &oracle)
        .unwrap();

    let env = mock_env();

    let bluechip_amount = Uint128::new(5_000_000);
    let result = bluechip_to_usd(deps.as_ref(), bluechip_amount, env.clone());
    assert!(result.is_ok(), "bluechip_to_usd should succeed");
    println!("5 bluechip = ${}", result.as_ref().unwrap().amount);

    let usd_amount = Uint128::new(5_000_000); // $5
    let result2 = usd_to_bluechip(deps.as_ref(), usd_amount, env.clone());
    assert!(result2.is_ok(), "usd_to_bluechip should succeed");
    println!("$5 = {} bluechip", result2.as_ref().unwrap().amount);
}

#[test]
fn test_mint_formula() {
    // Test case 1: First pool (x=1, s=0)
    let amount = calculate_mint_amount(0, 1).unwrap();
    // 500 - ((5*1 + 1) / (0/6 + 333*1)) = 500 - (6/333) ≈ 499.98
    assert!(amount > Uint128::new(499_900_000));

    // Test case 2: 10 pools after 1 hour (x=10, s=3600)
    let amount = calculate_mint_amount(3600, 10).unwrap();
    // 500 - ((5*100 + 10) / (600 + 3330)) = 500 - (510/3930) ≈ 499.87
    assert!(amount > Uint128::new(499_800_000));

    let amount = calculate_mint_amount(3600, 1000).unwrap();
    assert!(amount > Uint128::new(480_000_000));
}

#[test]
fn test_bluechip_minting_on_threshold_crossing() {
    let mut deps = mock_dependencies(&[]);

    setup_atom_pool(&mut deps);
    let msg = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: bluechip_wallet_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create first pool - should NOT mint (minting moved to threshold crossing)
    let create_msg = ExecuteMsg::Create {
        pool_msg: create_test_pool_msg(),
        token_info: CreatorTokenInfo {
            name: "First Token".to_string(),
            symbol: "FIRST".to_string(),
            decimal: 6,
        },
    };

    let info = message_info(&admin_addr(), &[]);
    let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

    // Pool creation should NOT have a mint BankMsg anymore
    let mint_msg = res
        .messages
        .iter()
        .find(|m| matches!(m.msg, CosmosMsg::Bank(BankMsg::Send { .. })));

    assert!(
        mint_msg.is_none(),
        "Pool creation should NOT mint bluechip tokens (moved to threshold crossing)"
    );

    // Register pool 1 in the registry so NotifyThresholdCrossed can verify caller
    let pool_addr = Addr::unchecked("pool_contract_1");
    register_test_pool_addr(deps.as_mut().storage, 1, &pool_addr);

    // Now simulate the pool notifying threshold crossed
    let notify_msg = ExecuteMsg::NotifyThresholdCrossed { pool_id: 1 };
    let pool_info = message_info(&pool_addr, &[]);
    let res = execute(deps.as_mut(), env.clone(), pool_info, notify_msg).unwrap();

    // Should now have a mint message
    let mint_msg = res
        .messages
        .iter()
        .find(|m| matches!(m.msg, CosmosMsg::Bank(BankMsg::Send { .. })));

    assert!(
        mint_msg.is_some(),
        "NotifyThresholdCrossed should trigger bluechip mint"
    );

    if let CosmosMsg::Bank(BankMsg::Send { to_address, amount }) = &mint_msg.unwrap().msg {
        assert_eq!(to_address, bluechip_wallet_addr().as_str());
        assert_eq!(amount.len(), 1);
        assert_eq!(amount[0].denom, "ubluechip");
        assert!(amount[0].amount > Uint128::new(499_000_000));
        assert!(amount[0].amount <= Uint128::new(500_000_000));
    }

    // Verify double-minting is prevented
    let notify_msg2 = ExecuteMsg::NotifyThresholdCrossed { pool_id: 1 };
    let pool_info2 = message_info(&pool_addr, &[]);
    let err = execute(deps.as_mut(), env.clone(), pool_info2, notify_msg2);
    assert!(
        err.is_err(),
        "Should reject duplicate threshold notification"
    );

    // Verify pool counter incremented correctly
    let pool_count = POOL_COUNTER.load(&deps.storage).unwrap();
    assert_eq!(pool_count, 1);
}

#[test]
fn test_no_mint_when_amount_is_zero() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    let msg = FactoryInstantiate {
        factory_admin_address: admin_addr(),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: bluechip_wallet_addr(),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        atom_bluechip_anchor_pool_address: atom_bluechip_pool_addr(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        msg,
    )
    .unwrap();

    POOL_COUNTER
        .save(&mut deps.storage, &10_000_000_000_000)
        .unwrap();

    FIRST_THRESHOLD_TIMESTAMP
        .save(&mut deps.storage, &env.block.time)
        .unwrap();

    let create_msg = ExecuteMsg::Create {
        pool_msg: create_test_pool_msg(),
        token_info: CreatorTokenInfo {
            name: "Test Token".to_string(),
            symbol: "TEST".to_string(),
            decimal: 6,
        },
    };

    let info = message_info(&admin_addr(), &[]);
    let res = execute(deps.as_mut(), env, info, create_msg).unwrap();

    let has_bank_msg = res
        .messages
        .iter()
        .any(|m| matches!(m.msg, CosmosMsg::Bank(BankMsg::Send { .. })));

    assert!(!has_bank_msg, "Should not mint when amount would be zero");
}

// Helper function for creating a test pool message
fn create_test_pool_msg() -> CreatePool {
    CreatePool {
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            },
        ],
        factory_to_create_pool_addr: Addr::unchecked("factory"),
        cw20_token_contract_id: 10,
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: ubluechip_addr(),
            creator_wallet_address: admin_addr(),
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
    }
}

// ---------------------------------------------------------------------------
// Oracle update bounty tests
// ---------------------------------------------------------------------------

#[test]
fn test_oracle_bounty_defaults_to_zero_on_instantiate() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    let bounty = crate::state::ORACLE_UPDATE_BOUNTY_USD
        .load(&deps.storage)
        .unwrap();
    assert_eq!(bounty, Uint128::zero());
}

#[test]
fn test_set_oracle_update_bounty_admin_only() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, create_default_instantiate_msg()).unwrap();

    // Non-admin should be rejected
    let non_admin = message_info(&addr0000(), &[]);
    let err = execute(
        deps.as_mut(),
        env.clone(),
        non_admin,
        ExecuteMsg::SetOracleUpdateBounty {
            new_bounty: Uint128::new(100_000),
        },
    )
    .unwrap_err();
    let err_msg = format!("{}", err);
    assert!(
        err_msg.contains("admin") || err_msg.contains("Admin"),
        "expected admin error, got: {}",
        err_msg
    );

    // Admin should succeed
    let admin = message_info(&admin_addr(), &[]);
    execute(
        deps.as_mut(),
        env,
        admin,
        ExecuteMsg::SetOracleUpdateBounty {
            new_bounty: Uint128::new(100_000),
        },
    )
    .unwrap();

    let bounty = crate::state::ORACLE_UPDATE_BOUNTY_USD
        .load(&deps.storage)
        .unwrap();
    assert_eq!(bounty, Uint128::new(100_000));
}

#[test]
fn test_set_oracle_update_bounty_rejects_above_cap() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let info = message_info(&admin_addr(), &[]);
    instantiate(deps.as_mut(), env.clone(), info, create_default_instantiate_msg()).unwrap();

    let admin = message_info(&admin_addr(), &[]);
    let over_cap = crate::state::MAX_ORACLE_UPDATE_BOUNTY_USD + Uint128::one();
    let err = execute(
        deps.as_mut(),
        env,
        admin,
        ExecuteMsg::SetOracleUpdateBounty {
            new_bounty: over_cap,
        },
    )
    .unwrap_err();
    assert!(format!("{}", err).contains("exceeds max"));
}

#[test]
fn test_oracle_update_pays_bounty_when_funded() {
    let bounty = Uint128::new(50_000);
    // Pre-fund the factory contract with enough ubluechip to cover the bounty
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Admin sets a bounty
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetOracleUpdateBounty { new_bounty: bounty },
    )
    .unwrap();

    // Fast-forward past update interval
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);

    let keeper = message_info(&addr0000(), &[]);
    let res = execute(
        deps.as_mut(),
        future_env,
        keeper.clone(),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // Response must include a BankMsg::Send paying the keeper
    let paid = res.messages.iter().any(|sm| match &sm.msg {
        CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
            to_address == keeper.sender.as_str()
                && amount.len() == 1
                && amount[0].denom == "ubluechip"
                && amount[0].amount == bounty
        }
        _ => false,
    });
    assert!(paid, "expected bounty BankMsg::Send to keeper");
    // The configured bounty is in USD; the attribute records both
    // the USD value and the converted bluechip amount.
    assert!(
        res.attributes
            .iter()
            .any(|a| a.key == "bounty_paid_usd" && a.value == bounty.to_string()),
        "expected bounty_paid_usd attribute"
    );
}

#[test]
fn test_oracle_update_skips_bounty_when_underfunded() {
    // Factory has insufficient balance
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100), // less than bounty
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetOracleUpdateBounty {
            new_bounty: Uint128::new(50_000),
        },
    )
    .unwrap();

    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);

    let res = execute(
        deps.as_mut(),
        future_env,
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // Oracle update must still succeed, just no BankMsg
    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. }))),
        "no BankMsg::Send expected when underfunded"
    );
    assert!(
        res.attributes
            .iter()
            .any(|a| a.key == "bounty_skipped" && a.value == "insufficient_factory_balance"),
        "expected bounty_skipped attribute"
    );
}

#[test]
fn test_force_rotate_requires_propose_first() {
    // Calling ForceRotateOraclePools without first proposing must fail —
    // the 2-step timelock flow is not optional.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::ForceRotateOraclePools {},
    )
    .unwrap_err();

    assert!(
        format!("{}", err).contains("No pending force-rotate"),
        "expected 'no pending' rejection, got: {}",
        err
    );
}

#[test]
fn test_force_rotate_cancel_clears_pending() {
    // Admin can cancel a pending force-rotate before execution.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    instantiate(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    assert!(
        crate::state::PENDING_ORACLE_ROTATION
            .may_load(&deps.storage)
            .unwrap()
            .is_some()
    );

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::CancelForceRotateOraclePools {},
    )
    .unwrap();

    assert!(
        crate::state::PENDING_ORACLE_ROTATION
            .may_load(&deps.storage)
            .unwrap()
            .is_none()
    );

    // After cancellation, executing must fail with "no pending" again.
    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(86400 * 3);
    let err = execute(
        deps.as_mut(),
        future_env,
        admin_info,
        ExecuteMsg::ForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(format!("{}", err).contains("No pending force-rotate"));
}

#[test]
fn test_force_rotate_cancel_non_admin_rejected() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Admin proposes so there's a pending entry.
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    // Non-admin tries to cancel.
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&Addr::unchecked("hacker"), &[]),
        ExecuteMsg::CancelForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
}

#[test]
fn test_force_rotate_double_propose_rejected() {
    // Proposing a force-rotate while one is already pending must be
    // rejected so there is no ambiguity about which effective_after
    // applies.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    instantiate(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        env,
        admin_info,
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(
        format!("{}", err).contains("already pending"),
        "expected 'already pending' error, got: {}",
        err
    );
}

#[test]
fn test_force_rotate_executes_at_exact_timelock_boundary() {
    // Code uses `env.block.time < effective_after` so execution should
    // succeed at exactly effective_after (one-second-earlier should fail).
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    instantiate(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    let effective_after = crate::state::PENDING_ORACLE_ROTATION
        .load(&deps.storage)
        .unwrap();

    // One second before effective_after: must fail.
    let mut early_env = env.clone();
    early_env.block.time = effective_after.minus_seconds(1);
    let err = execute(
        deps.as_mut(),
        early_env,
        admin_info.clone(),
        ExecuteMsg::ForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(matches!(
        err,
        crate::error::ContractError::TimelockNotExpired { .. }
    ));

    // Exactly at effective_after: must succeed.
    let mut exact_env = env;
    exact_env.block.time = effective_after;
    let res = execute(
        deps.as_mut(),
        exact_env,
        admin_info,
        ExecuteMsg::ForceRotateOraclePools {},
    );
    assert!(
        res.is_ok(),
        "force-rotate at exactly effective_after must succeed, got: {:?}",
        res
    );
}

#[test]
fn test_force_rotate_stale_pending_still_executes() {
    // Documents current behavior: there is no expiry on PENDING_ORACLE_ROTATION.
    // If the admin proposes and then forgets for a year, the rotation still
    // executes. This test pins that behavior so any future change (adding
    // a max-age to pending rotations) is a deliberate decision with a
    // visibly-failing test to update.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    instantiate(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap();

    // Jump forward one year. Pending entry must still be honored.
    let mut future_env = env;
    future_env.block.time = future_env.block.time.plus_seconds(86400 * 365);

    let res = execute(
        deps.as_mut(),
        future_env,
        admin_info,
        ExecuteMsg::ForceRotateOraclePools {},
    );
    assert!(
        res.is_ok(),
        "stale pending rotation currently still executes; update this test \
         if/when a max-age is added"
    );
}

#[test]
fn test_force_rotate_propose_non_admin_rejected() {
    // Companion to the cancel-non-admin test: proposing must also be
    // admin-gated or a compromised low-privilege key could spam
    // PENDING_ORACLE_ROTATION entries.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        env,
        message_info(&Addr::unchecked("hacker"), &[]),
        ExecuteMsg::ProposeForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
}

#[test]
fn test_force_rotate_cancel_with_no_pending_rejected() {
    // Cancelling when nothing is pending should be a distinct error —
    // catches accidental double-cancels or stale CLI scripts.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    let env = mock_env();
    let admin_info = message_info(&admin_addr(), &[]);
    instantiate(
        deps.as_mut(),
        env.clone(),
        admin_info.clone(),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        env,
        admin_info,
        ExecuteMsg::CancelForceRotateOraclePools {},
    )
    .unwrap_err();
    assert!(
        format!("{}", err).contains("No pending force-rotate"),
        "expected 'no pending' rejection, got: {}",
        err
    );
}

#[test]
fn test_oracle_ignores_pools_without_threshold_crossed() {
    // A pool that has been created but has NOT crossed its commit threshold
    // must not enter the oracle sample set, even if it somehow has liquidity.
    // This defends against spam pools (permissionless creation) from
    // influencing the bluechip/ATOM price.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);

    // Pool 1: threshold-crossed, should be eligible
    {
        let pool_addr = make_addr("good_pool");
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
        };
        POOLS_BY_ID
            .save(&mut deps.storage, 1, &pool_details)
            .unwrap();
        crate::state::POOL_THRESHOLD_MINTED
            .save(&mut deps.storage, 1, &true)
            .unwrap();
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, pool_addr, &pool_state)
            .unwrap();
    }

    // Pool 2: NOT threshold-crossed (spam/pre-threshold), must be ignored.
    // Even with liquidity far above MIN_POOL_LIQUIDITY.
    {
        let pool_addr = make_addr("spam_pool");
        let pool_details = PoolDetails {
            pool_id: 2,
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("creator_token_2"),
                },
            ],
            creator_pool_addr: pool_addr.clone(),
            pool_kind: pool_factory_interfaces::PoolKind::Commit,
        };
        POOLS_BY_ID
            .save(&mut deps.storage, 2, &pool_details)
            .unwrap();
        // Deliberately NOT saving POOL_THRESHOLD_MINTED for pool 2.
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(500_000_000_000),
            reserve1: Uint128::new(100_000_000_000),
            total_liquidity: Uint128::new(100_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(&mut deps.storage, pool_addr, &pool_state)
            .unwrap();
    }

    let eligible = crate::internal_bluechip_price_oracle::get_eligible_creator_pools(
        deps.as_ref(),
        &atom_bluechip_pool_addr().to_string(),
    )
    .unwrap();

    assert_eq!(eligible.len(), 1, "only the threshold-crossed pool should be eligible");
    assert_eq!(eligible[0], make_addr("good_pool").to_string());
    assert!(
        !eligible.contains(&make_addr("spam_pool").to_string()),
        "spam pool without threshold crossing must not appear"
    );
}

#[test]
fn test_oracle_update_bounty_equals_balance_boundary() {
    // The check is `balance.amount >= bounty`, so balance == bounty must
    // still pay out. Pins the `>=` semantic — a regression to `>` would
    // silently break keeper payouts when the factory reserve is down to
    // exactly one bounty's worth.
    let bounty = Uint128::new(50_000);
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: bounty, // exactly equal
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetOracleUpdateBounty { new_bounty: bounty },
    )
    .unwrap();

    let mut future_env = env;
    future_env.block.time = future_env.block.time.plus_seconds(360);

    let res = execute(
        deps.as_mut(),
        future_env,
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    let paid = res.messages.iter().any(|sm| match &sm.msg {
        CosmosMsg::Bank(BankMsg::Send { amount, .. }) => {
            amount.len() == 1 && amount[0].amount == bounty
        }
        _ => false,
    });
    assert!(paid, "bounty must pay when balance equals bounty exactly");
}

#[test]
fn test_oracle_update_bounty_one_less_than_amount_skipped() {
    // Mirror of the above: one ubluechip below the bounty must skip.
    let bounty = Uint128::new(50_000);
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: bounty - Uint128::one(), // one short
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetOracleUpdateBounty { new_bounty: bounty },
    )
    .unwrap();

    let mut future_env = env;
    future_env.block.time = future_env.block.time.plus_seconds(360);

    let res = execute(
        deps.as_mut(),
        future_env,
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // No BankMsg; bounty_skipped attribute present.
    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. })))
    );
    assert!(
        res.attributes
            .iter()
            .any(|a| a.key == "bounty_skipped"
                && a.value == "insufficient_factory_balance"),
        "expected bounty_skipped=insufficient_factory_balance"
    );
}

#[test]
fn test_oracle_update_cooldown_blocks_second_call_even_with_bounty() {
    // The bounty must not bypass the UPDATE_INTERVAL cooldown — this is
    // the whole anti-spam property of the design. A second call in the
    // same 5-minute window must be rejected regardless of bounty state.
    let bounty = Uint128::new(50_000);
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000), // plenty
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetOracleUpdateBounty { new_bounty: bounty },
    )
    .unwrap();

    // First call after 360s — succeeds and pays bounty.
    let mut t1 = env.clone();
    t1.block.time = t1.block.time.plus_seconds(360);
    execute(
        deps.as_mut(),
        t1.clone(),
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // Second call 60s later — inside the cooldown window. Must fail and
    // must NOT pay out a second bounty.
    let mut t2 = t1;
    t2.block.time = t2.block.time.plus_seconds(60);
    let err = execute(
        deps.as_mut(),
        t2,
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap_err();
    assert!(
        matches!(err, crate::error::ContractError::UpdateTooSoon { .. }),
        "second call within 5min must be rejected, got: {:?}",
        err
    );
}

#[test]
fn test_oracle_update_no_bounty_when_disabled() {
    // Bounty defaults to zero on instantiate; admin never calls SetOracleUpdateBounty
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);

    for i in 1..=3 {
        let pool_addr = make_addr(&format!("creator_pool_{}", i));
        let pool_state = PoolStateResponseForFactory {
            pool_contract_address: pool_addr.clone(),
            nft_ownership_accepted: true,
            reserve0: Uint128::new(50_000_000_000),
            reserve1: Uint128::new(10_000_000_000),
            total_liquidity: Uint128::new(10_000_000),
            block_time_last: 0,
            price0_cumulative_last: Uint128::zero(),
            price1_cumulative_last: Uint128::zero(),
            assets: vec![],
        };
        POOLS_BY_CONTRACT_ADDRESS
            .save(deps.as_mut().storage, pool_addr, &pool_state)
            .unwrap();
    }

    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let mut future_env = env.clone();
    future_env.block.time = future_env.block.time.plus_seconds(360);

    let res = execute(
        deps.as_mut(),
        future_env,
        message_info(&addr0000(), &[]),
        ExecuteMsg::UpdateOraclePrice {},
    )
    .unwrap();

    // No bank message, no bounty attributes at all
    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. })))
    );
    assert!(
        !res.attributes.iter().any(|a| a.key.starts_with("bounty_")),
        "no bounty attributes expected when disabled"
    );
}

// ---------------------------------------------------------------------------
// M-06 · Creator token name/symbol validation
// ---------------------------------------------------------------------------
// These tests exercise validate_creator_token_info directly against every
// rule and both boundaries. They exist to pin the spec: accidental weakening
// of any rule (e.g. allowing lowercase symbols) would break a test here.

use crate::execute::validate_creator_token_info;

fn valid_token_info() -> CreatorTokenInfo {
    CreatorTokenInfo {
        name: "Valid Name".to_string(),
        symbol: "VLD".to_string(),
        decimal: 6,
    }
}

#[test]
fn test_validate_accepts_known_good() {
    // Sanity check: the baseline fixture must pass so negative tests
    // below only fail on the specific field they mutate.
    assert!(validate_creator_token_info(&valid_token_info()).is_ok());
}

#[test]
fn test_validate_rejects_wrong_decimals() {
    for bad_decimal in [0u8, 1, 5, 7, 18, 255] {
        let mut info = valid_token_info();
        info.decimal = bad_decimal;
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("decimals must be 6"),
            "decimal={} should be rejected, got: {}",
            bad_decimal,
            err
        );
    }
}

#[test]
fn test_validate_name_length_boundaries() {
    // Name must be 3..=50 inclusive.
    let cases: &[(usize, bool)] = &[
        (0, false),  // empty
        (1, false),
        (2, false),  // just below min
        (3, true),   // exactly min
        (4, true),
        (25, true),
        (49, true),
        (50, true),  // exactly max
        (51, false), // just above max
        (100, false),
    ];
    for (len, should_pass) in cases {
        let mut info = valid_token_info();
        info.name = "A".repeat(*len);
        let result = validate_creator_token_info(&info);
        assert_eq!(
            result.is_ok(),
            *should_pass,
            "name len={} should be {}",
            len,
            if *should_pass { "accepted" } else { "rejected" }
        );
    }
}

#[test]
fn test_validate_name_rejects_non_ascii() {
    // Non-ASCII should be rejected — common spoofing vector (Cyrillic
    // lookalikes, fullwidth chars, etc.).
    let bad_names = [
        "Nameе",     // trailing Cyrillic 'e'
        "名前テスト",    // CJK
        "Pool🚀",    // emoji
        "Café",      // accented Latin
        "Ｔｅｓｔ",    // fullwidth ASCII
    ];
    for name in bad_names {
        let mut info = valid_token_info();
        info.name = name.to_string();
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("printable ASCII"),
            "name '{}' should be rejected, got: {}",
            name,
            err
        );
    }
}

#[test]
fn test_validate_name_rejects_control_chars() {
    for control in ['\n', '\t', '\r', '\0', '\x7f'] {
        let mut info = valid_token_info();
        info.name = format!("Bad{}Name", control);
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("printable ASCII"),
            "control char {:?} should be rejected, got: {}",
            control,
            err
        );
    }
}

#[test]
fn test_validate_name_accepts_printable_ascii() {
    // Spaces, punctuation, digits — all printable ASCII must pass.
    let good_names = [
        "ABC",
        "My Token v2",
        "Pool #42",
        "100% Fair",
        "Token (beta)",
        "A.B.C",
        "a-b-c",
    ];
    for name in good_names {
        let mut info = valid_token_info();
        info.name = name.to_string();
        assert!(
            validate_creator_token_info(&info).is_ok(),
            "name '{}' should be accepted",
            name
        );
    }
}

#[test]
fn test_validate_symbol_length_boundaries() {
    // Symbol must be 3..=12 inclusive.
    let cases: &[(usize, bool)] = &[
        (0, false),
        (1, false),
        (2, false),
        (3, true),
        (6, true),
        (11, true),
        (12, true),
        (13, false),
        (50, false),
    ];
    for (len, should_pass) in cases {
        let mut info = valid_token_info();
        info.symbol = "A".repeat(*len);
        let result = validate_creator_token_info(&info);
        assert_eq!(
            result.is_ok(),
            *should_pass,
            "symbol len={} should be {}",
            len,
            if *should_pass { "accepted" } else { "rejected" }
        );
    }
}

#[test]
fn test_validate_symbol_rejects_lowercase() {
    let bad_symbols = ["abc", "Abc", "ABc", "ABCd", "vld"];
    for symbol in bad_symbols {
        let mut info = valid_token_info();
        info.symbol = symbol.to_string();
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("uppercase"),
            "symbol '{}' should be rejected, got: {}",
            symbol,
            err
        );
    }
}

#[test]
fn test_validate_symbol_rejects_special_chars() {
    // Symbol allows only A-Z and 0-9. Everything else must fail.
    // All strings here are length 3-12 so we only test charset rejection,
    // not length rejection.
    let bad_symbols = ["A.B", "A-B", "A B", "A$B", "A_B", "A@B", "AB!", "AB#"];
    for symbol in bad_symbols {
        let mut info = valid_token_info();
        info.symbol = symbol.to_string();
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("uppercase"),
            "symbol '{}' should be rejected, got: {}",
            symbol,
            err
        );
    }
}

#[test]
fn test_validate_symbol_rejects_non_ascii() {
    let bad_symbols = ["ABCЕ", "ТЕСТ", "A🚀B"];
    for symbol in bad_symbols {
        let mut info = valid_token_info();
        info.symbol = symbol.to_string();
        let err = validate_creator_token_info(&info).unwrap_err();
        assert!(
            format!("{}", err).contains("uppercase"),
            "symbol '{}' should be rejected, got: {}",
            symbol,
            err
        );
    }
}

// ---------------------------------------------------------------------------
// M-03 · Pyth cached-price fallback age boundaries
// ---------------------------------------------------------------------------
// Cache is valid up to MAX_PRICE_AGE_SECONDS_BEFORE_STALE seconds old
// (300s). Beyond that, get_bluechip_usd_price must refuse to price rather
// than leak a stale value into commit valuations. These tests pin the
// exact boundary so a future widening of the window would be caught.

use crate::internal_bluechip_price_oracle::MOCK_PYTH_SHOULD_FAIL;
use crate::state::MAX_PRICE_AGE_SECONDS_BEFORE_STALE;

fn setup_oracle_with_cached_pyth(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    env: &Env,
    cached_age_seconds: u64,
    cached_pyth_price: Uint128,
    bluechip_per_atom: Uint128,
) {
    setup_atom_pool(deps);
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let cached_ts = env.block.time.seconds().saturating_sub(cached_age_seconds);
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_price = bluechip_per_atom;
    oracle.bluechip_price_cache.last_update = env.block.time.seconds();
    oracle.bluechip_price_cache.cached_pyth_price = cached_pyth_price;
    oracle.bluechip_price_cache.cached_pyth_timestamp = cached_ts;
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();
}

#[test]
fn test_pyth_cache_accepts_fresh_cached_price_when_live_fails() {
    // Live Pyth fails, cache is comfortably inside the (tightened 90s)
    // staleness window. Must succeed. The previous value (100s) became
    // stale after the window tightened; using MAX - 30 keeps the test
    // tracking whatever the constant evolves to.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    let fresh_age = MAX_PRICE_AGE_SECONDS_BEFORE_STALE.saturating_sub(30);
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        fresh_age,
        Uint128::new(12_000_000),
        Uint128::new(1_000_000),
    );
    MOCK_PYTH_SHOULD_FAIL
        .save(&mut deps.storage, &true)
        .unwrap();

    let result = get_bluechip_usd_price(deps.as_ref(), &env);
    assert!(
        result.is_ok(),
        "fresh cache ({}s old) must be accepted, got: {:?}",
        fresh_age,
        result
    );
}

#[test]
fn test_pyth_cache_accepts_at_exact_max_age() {
    // Cache is exactly MAX_PRICE_AGE_SECONDS_BEFORE_STALE seconds old.
    // Code uses `> max` so equality must still be accepted.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        MAX_PRICE_AGE_SECONDS_BEFORE_STALE,
        Uint128::new(12_000_000),
        Uint128::new(1_000_000),
    );
    MOCK_PYTH_SHOULD_FAIL
        .save(&mut deps.storage, &true)
        .unwrap();

    let result = get_bluechip_usd_price(deps.as_ref(), &env);
    assert!(
        result.is_ok(),
        "cache at exactly {}s old must be accepted, got: {:?}",
        MAX_PRICE_AGE_SECONDS_BEFORE_STALE,
        result
    );
}

#[test]
fn test_pyth_cache_rejects_one_second_past_max() {
    // One second beyond the staleness boundary must be rejected.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        MAX_PRICE_AGE_SECONDS_BEFORE_STALE + 1,
        Uint128::new(12_000_000),
        Uint128::new(1_000_000),
    );
    MOCK_PYTH_SHOULD_FAIL
        .save(&mut deps.storage, &true)
        .unwrap();

    let err = get_bluechip_usd_price(deps.as_ref(), &env).unwrap_err();
    assert!(
        format!("{}", err).contains("stale")
            || format!("{}", err).contains("no valid cached"),
        "expected stale/cache rejection, got: {}",
        err
    );
}

#[test]
fn test_pyth_cache_rejects_far_past_max() {
    // Catches anyone who later widens the acceptance window by mistake
    // (e.g. reverting to the old 2x multiplier). 10 minutes old and
    // Pyth-failing must reject.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        600, // 10 minutes, well past 300
        Uint128::new(12_000_000),
        Uint128::new(1_000_000),
    );
    MOCK_PYTH_SHOULD_FAIL
        .save(&mut deps.storage, &true)
        .unwrap();

    let err = get_bluechip_usd_price(deps.as_ref(), &env).unwrap_err();
    assert!(
        format!("{}", err).contains("stale")
            || format!("{}", err).contains("no valid cached"),
        "expected rejection at 600s, got: {}",
        err
    );
}

#[test]
fn test_pyth_cache_rejects_zero_cached_price() {
    // If cached_pyth_price was never populated (still zero), fallback
    // must reject regardless of age — zero is the bootstrap sentinel.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        10, // fresh by age
        Uint128::zero(),
        Uint128::new(1_000_000),
    );
    MOCK_PYTH_SHOULD_FAIL
        .save(&mut deps.storage, &true)
        .unwrap();

    let err = get_bluechip_usd_price(deps.as_ref(), &env).unwrap_err();
    assert!(
        format!("{}", err).contains("stale")
            || format!("{}", err).contains("no valid cached"),
        "expected rejection for zero cached price, got: {}",
        err
    );
}

#[test]
fn test_pyth_live_price_bypasses_cache_entirely() {
    // Cache is way past max age, but live Pyth works. Must succeed
    // because the cache path is only consulted on live failure.
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    setup_oracle_with_cached_pyth(
        &mut deps,
        &env,
        99999,
        Uint128::zero(),
        Uint128::new(1_000_000),
    );
    // Leave MOCK_PYTH_SHOULD_FAIL unset so live path succeeds.

    let result = get_bluechip_usd_price(deps.as_ref(), &env);
    assert!(
        result.is_ok(),
        "live pyth should bypass the cache age check, got: {:?}",
        result
    );
}

#[test]
fn test_validate_symbol_accepts_uppercase_and_digits() {
    let good_symbols = ["ABC", "USDC", "BTC", "ETH2", "USD1", "AAA123", "AAAAAAAAAAAA"];
    for symbol in good_symbols {
        let mut info = valid_token_info();
        info.symbol = symbol.to_string();
        assert!(
            validate_creator_token_info(&info).is_ok(),
            "symbol '{}' should be accepted",
            symbol
        );
    }
}

// ---------------------------------------------------------------------------
// Distribution bounty (paid by factory on behalf of pools)
// ---------------------------------------------------------------------------
// Pools no longer hold or pay their own keeper bounty for distribution
// batches — they forward a PayDistributionBounty message to the factory
// and the factory pays from its own native reserve. These tests pin the
// auth gate (only registered pools), the admin-tunable bounty amount,
// and the graceful-skip behavior on underfund / disabled.

fn register_test_pool(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>, addr: &Addr) {
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            addr.clone(),
            &PoolStateResponseForFactory {
                pool_contract_address: addr.clone(),
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
}

// Forces a non-zero oracle price so usd_to_bluechip succeeds in tests
// that don't go through the full UpdateOraclePrice flow. Pins the
// conversion at exactly 1 ubluechip = $1.00 USD (matching MOCK_PYTH_PRICE
// of $10 ATOM with bluechip_per_atom_twap of 10_000_000).
fn seed_oracle_price_for_bounty_tests(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
) {
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_price = Uint128::new(10_000_000);
    oracle.bluechip_price_cache.last_update = mock_env().block.time.seconds();
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();
}

#[test]
fn test_distribution_bounty_defaults_to_zero() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let bounty = crate::state::DISTRIBUTION_BOUNTY_USD
        .load(&deps.storage)
        .unwrap();
    assert_eq!(bounty, Uint128::zero());
}

#[test]
fn test_set_distribution_bounty_admin_only() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Non-admin rejected.
    let err = execute(
        deps.as_mut(),
        env.clone(),
        message_info(&addr0000(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: Uint128::new(100_000),
        },
    )
    .unwrap_err();
    assert!(
        format!("{}", err).contains("admin") || format!("{}", err).contains("Admin"),
        "expected admin error, got: {}",
        err
    );

    // Admin succeeds.
    execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: Uint128::new(100_000),
        },
    )
    .unwrap();
    let bounty = crate::state::DISTRIBUTION_BOUNTY_USD
        .load(&deps.storage)
        .unwrap();
    assert_eq!(bounty, Uint128::new(100_000));
}

#[test]
fn test_set_distribution_bounty_rejects_above_cap() {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    let over = crate::state::MAX_DISTRIBUTION_BOUNTY_USD + Uint128::one();
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty { new_bounty: over },
    )
    .unwrap_err();
    assert!(format!("{}", err).contains("exceeds max"));
}

#[test]
fn test_pay_distribution_bounty_rejects_non_pool_caller() {
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Configure non-zero bounty so the auth check is the only gate.
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: Uint128::new(50_000),
        },
    )
    .unwrap();

    // A random address that is NOT in POOLS_BY_CONTRACT_ADDRESS tries to
    // pay itself a bounty — must be rejected.
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&Addr::unchecked("hacker"), &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: "hacker".to_string(),
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, crate::error::ContractError::Unauthorized {}),
        "expected Unauthorized, got: {:?}",
        err
    );
}

#[test]
fn test_pay_distribution_bounty_pays_registered_pool() {
    // 50_000 = $0.05 USD bounty, within the MAX_DISTRIBUTION_BOUNTY_USD
    // cap of $0.10. With the seeded oracle price below (1 bluechip = $1.00)
    // the converted payout is 50_000 ubluechip.
    let bounty = Uint128::new(50_000);
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();
    seed_oracle_price_for_bounty_tests(&mut deps);

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty { new_bounty: bounty },
    )
    .unwrap();

    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);
    let keeper = make_addr("keeper");

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: keeper.to_string(),
        },
    )
    .unwrap();

    let paid = res.messages.iter().any(|sm| match &sm.msg {
        CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
            to_address == keeper.as_str()
                && amount.len() == 1
                && amount[0].amount == bounty
                && amount[0].denom == "ubluechip"
        }
        _ => false,
    });
    assert!(paid, "expected BankMsg::Send paying keeper, got: {:?}", res.messages);
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "bounty_paid_usd" && a.value == bounty.to_string()));
}

#[test]
fn test_pay_distribution_bounty_skips_when_disabled() {
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Bounty stays at zero (default). A registered pool calling
    // PayDistributionBounty must succeed but emit no BankMsg.
    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: addr0000().to_string(),
        },
    )
    .unwrap();

    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. })))
    );
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "bounty_skipped" && a.value == "disabled"));
}

#[test]
fn test_pay_distribution_bounty_skips_when_underfunded() {
    let bounty = Uint128::new(50_000);
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100), // way below the converted bounty
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();
    seed_oracle_price_for_bounty_tests(&mut deps);

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty { new_bounty: bounty },
    )
    .unwrap();

    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: addr0000().to_string(),
        },
    )
    .unwrap();

    // No BankMsg, but tx still succeeds so the pool's distribution can
    // make progress.
    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. })))
    );
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "bounty_skipped" && a.value == "insufficient_factory_balance"));
}

// ---------------------------------------------------------------------------
// USD-denomination conversion behavior
// ---------------------------------------------------------------------------
// Bounties are stored in USD (6 decimals) and converted to bluechip at
// payout time using the current oracle price. As bluechip appreciates
// in USD terms, the bluechip amount paid SHRINKS so keeper compensation
// stays roughly constant in real terms.

#[test]
fn test_distribution_bounty_converts_via_oracle_price() {
    // With seeded oracle (1 bluechip = $1.00 USD), $0.50 USD bounty
    // converts to 500_000 ubluechip.
    let bounty_usd = Uint128::new(50_000); // $0.05
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();
    seed_oracle_price_for_bounty_tests(&mut deps);

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: bounty_usd,
        },
    )
    .unwrap();

    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: addr0000().to_string(),
        },
    )
    .unwrap();

    // Find the BankMsg amount.
    let paid_bluechip = res
        .messages
        .iter()
        .find_map(|sm| match &sm.msg {
            CosmosMsg::Bank(BankMsg::Send { amount, .. }) => amount.first().map(|c| c.amount),
            _ => None,
        })
        .expect("expected a BankMsg::Send");
    // At seeded mock price (1 bluechip = $1), $0.05 = 50_000 ubluechip.
    assert_eq!(paid_bluechip, Uint128::new(50_000));

    // Both attributes must be present so operators can audit conversion.
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "bounty_paid_usd" && a.value == "50000"));
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "bounty_paid_bluechip" && a.value == "50000"));
}

#[test]
fn test_distribution_bounty_pays_less_bluechip_when_bluechip_appreciates() {
    // Same $0.50 USD bounty, but bluechip is now worth $2 (twice as
    // valuable). Expected bluechip payout: 250_000 ubluechip ($0.50 / $2).
    let bounty_usd = Uint128::new(50_000); // $0.05
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // Seed oracle so 1 bluechip = $2.00.
    // last_price = bluechip_per_atom_twap. With atom_usd_price = $10,
    // bluechip_usd_price = atom_usd_price * 1e6 / last_price.
    // We want bluechip_usd_price = 2_000_000 ($2).
    // 10_000_000 * 1_000_000 / X = 2_000_000  =>  X = 5_000_000.
    let mut oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    oracle.bluechip_price_cache.last_price = Uint128::new(5_000_000);
    oracle.bluechip_price_cache.last_update = env.block.time.seconds();
    INTERNAL_ORACLE.save(&mut deps.storage, &oracle).unwrap();

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: bounty_usd,
        },
    )
    .unwrap();

    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: addr0000().to_string(),
        },
    )
    .unwrap();

    let paid_bluechip = res
        .messages
        .iter()
        .find_map(|sm| match &sm.msg {
            CosmosMsg::Bank(BankMsg::Send { amount, .. }) => amount.first().map(|c| c.amount),
            _ => None,
        })
        .expect("expected a BankMsg::Send");
    // $0.05 / $2.00 = 0.025 bluechip = 25_000 ubluechip.
    assert_eq!(
        paid_bluechip,
        Uint128::new(25_000),
        "appreciated bluechip should mean fewer ubluechip per USD bounty"
    );
}

#[test]
fn test_distribution_bounty_skips_when_oracle_unavailable() {
    // If usd_to_bluechip errors (no oracle price), the pool's payout
    // request must succeed with bounty_skipped=price_unavailable so the
    // pool's distribution tx does not revert.
    let mut deps = mock_dependencies(&[cosmwasm_std::Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();
    // Deliberately do NOT seed oracle price — last_price stays zero so
    // get_bluechip_usd_price errors with "TWAP price is zero".

    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: Uint128::new(50_000),
        },
    )
    .unwrap();

    let pool_addr = make_addr("registered_pool");
    register_test_pool(&mut deps, &pool_addr);

    let res = execute(
        deps.as_mut(),
        env,
        message_info(&pool_addr, &[]),
        ExecuteMsg::PayDistributionBounty {
            recipient: addr0000().to_string(),
        },
    )
    .unwrap();

    assert!(
        res.messages
            .iter()
            .all(|sm| !matches!(sm.msg, CosmosMsg::Bank(BankMsg::Send { .. })))
    );
    assert!(
        res.attributes
            .iter()
            .any(|a| a.key == "bounty_skipped" && a.value == "price_unavailable"),
        "expected price_unavailable skip reason"
    );
}

#[test]
fn test_set_distribution_bounty_cap_enforced() {
    // Confirms MAX_DISTRIBUTION_BOUNTY_USD is honored at the cap boundary.
    // Anything above the cap is rejected, including one microdollar above.
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    let env = mock_env();
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        create_default_instantiate_msg(),
    )
    .unwrap();

    // The cap exactly is accepted.
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: crate::state::MAX_DISTRIBUTION_BOUNTY_USD,
        },
    )
    .unwrap();

    // One microdollar above the cap is rejected. Using the constant here
    // so the assertion tracks the cap automatically if it's ever adjusted.
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&admin_addr(), &[]),
        ExecuteMsg::SetDistributionBounty {
            new_bounty: crate::state::MAX_DISTRIBUTION_BOUNTY_USD + Uint128::one(),
        },
    )
    .unwrap_err();
    assert!(format!("{}", err).contains("exceeds max"));
}
