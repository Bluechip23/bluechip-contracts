use crate::state::{
    CreationState, CreationStatus, FactoryInstantiate, SETCOMMIT, CREATION_STATES, NEXT_POOL_ID,
    POOLS_BY_ID, TEMPCREATORWALLETADDR, TEMPNFTADDR, TEMPPOOLINFO, TEMPPOOLID, TEMPCREATORTOKENADDR,
};
use cosmwasm_std::{
    Addr, Binary, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse, SubMsgResult, Uint128,
};

use crate::asset::{TokenInfo, TokenType,};
use crate::execute::{execute, instantiate, reply, FINALIZE_POOL, MINT_CREATE_POOL, SET_TOKENS};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{ExecuteMsg, CreatorTokenInfo};
use crate::pool_struct::{CreatePool, CommitFeeInfo, PoolDetails};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage};

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

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let _owner = "owner0000".to_string();

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

    let _res0 = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();

    // Test multiple instantiations
    let env = mock_env();
    let addr = Addr::unchecked("addr0001");
    let info = mock_info(&addr.as_str(), &[]);

    let _res1 = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();

    let mut deps = mock_dependencies(&[]);
    let env = mock_env();
    let addr = Addr::unchecked("addr0002");
    let info = mock_info(&addr.as_str(), &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();
}

#[test]
fn create_pair() {
    let mut deps = mock_dependencies(&[]);

    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("addr0000"),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1), // 1%
        commit_fee_creator: Decimal::percent(5),  // 5%
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let pool_token_info = [
        TokenType::Bluechip {
            denom: "bluechip".to_string(),
        },
        TokenType::CreatorToken{
            contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
    ];

    // Create new env and info for execute
    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    // Create pair with correct PairInstantiateMsg structure
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

    // Verify submessages were created
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

    // Initialize factory first
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

    // Create pair with custom init params
    let custom_params = Binary::from(b"custom_pool_params");

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken{
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

    // Verify the creation was successful
    assert_eq!(res.messages.len(), 1);
}
fn create_pool_msg(token_name: &str) -> ExecuteMsg {
    ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Bluechip {
                    denom: "bluechip".to_string(),
                },
                TokenType::CreatorToken{
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
    // Token reply
    let token_reply = create_instantiate_reply(SET_TOKENS, &format!("token_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // NFT reply
    let nft_reply = create_instantiate_reply(MINT_CREATE_POOL, &format!("nft_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    // Pool reply
    let pool_reply = create_instantiate_reply(FINALIZE_POOL, &format!("pool_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), pool_reply).unwrap();
}

#[test]
fn test_asset_info() {
    let bluechip_info = TokenType::Bluechip {
        denom: "bluechip".to_string(),
    };
    assert!(bluechip_info.is_bluechip_token());

    let token_info = TokenType::CreatorToken{
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

    // Initialize factory
    let msg = create_default_instantiate_msg();
    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create 3 pools and verify IDs increment
    for expected_id in 1u64..=3u64 {
        // Create pool
        let create_msg = create_pool_msg(&format!("Token{}", expected_id));
        let info = mock_info(ADMIN, &[]);
        let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();
        assert!(
            //this fails
            res.attributes
                .iter()
                .any(|attr| attr.key == "pool_id" && attr.value == expected_id.to_string()),
            "Response should contain pool_id attribute"
        );
        // Verify pool ID was set correctly in TEMP storage
        let pool_id = TEMPPOOLID.load(&deps.storage).unwrap(); //this does not
        assert_eq!(pool_id, expected_id);

        // SET UP CREATION STATE - This is what's missing!
        let creator = TEMPCREATORWALLETADDR.load(&deps.storage).unwrap();
        let creation_state = CreationState {
            pool_id,
            creator: creator.clone(),
            creator_token_address: None,
            mint_new_position_nft_address: None,
            pool_address: None,
            creation_time: env.block.time,
            status: CreationStatus::Started,
            retry_count: 0,
        };
        CREATION_STATES
            .save(deps.as_mut().storage, pool_id, &creation_state)
            .unwrap();

        // Simulate complete reply chain
        simulate_complete_reply_chain(&mut deps, env.clone(), expected_id);

        // Verify next pool ID incremented
        assert_eq!(NEXT_POOL_ID.load(&deps.storage).unwrap(), expected_id + 1);

        // Verify creation state shows completed
        let final_state = CREATION_STATES.load(&deps.storage, pool_id).unwrap();
        assert_eq!(final_state.status, CreationStatus::Completed);
    }
}

#[test]
fn test_complete_pool_creation_flow() {
    let mut deps = mock_dependencies(&[]);

    // Initialize factory
    let msg = FactoryInstantiate {
        factory_admin_address: Addr::unchecked(ADMIN),
        cw721_nft_contract_id: 58,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000), // $25k in 6 decimals
        pyth_contract_addr_for_conversions: "oracle0000".to_string(),
        pyth_atom_usd_price_feed_id: "BLUECHIP".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        commit_fee_bluechip: Decimal::percent(1), // 1%
        commit_fee_creator: Decimal::percent(5),  // 5%
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
                TokenType::CreatorToken{
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
    // Verify TEMP states are set
    assert!(TEMPPOOLID.load(&deps.storage).is_ok());
    assert!(TEMPPOOLINFO.load(&deps.storage).is_ok());
    assert!(TEMPCREATORWALLETADDR.load(&deps.storage).is_ok());

    // GET THE POOL ID AND SET UP CREATION STATE
    let pool_id = TEMPPOOLID.load(&deps.storage).unwrap();
    let creator = TEMPCREATORWALLETADDR.load(&deps.storage).unwrap();

    // Create the CreationState that your new code expects
    let creation_state = CreationState {
        pool_id,
        creator: creator.clone(),
        creator_token_address: None,
        mint_new_position_nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    CREATION_STATES
        .save(deps.as_mut().storage, pool_id, &creation_state)
        .unwrap();

    // Simulate token instantiation reply
    let token_reply = create_instantiate_reply(SET_TOKENS, "token_address");
    let res = reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // Verify token address saved and NFT instantiation triggered
    assert_eq!(
        TEMPCREATORTOKENADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("token_address")
    );
    assert_eq!(res.messages.len(), 1); // NFT instantiate message

    // Verify creation state was updated
    let updated_state = CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked("token_address"))
    );

    // Simulate NFT instantiation reply
    let nft_reply = create_instantiate_reply(MINT_CREATE_POOL, "nft_address");
    let res = reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    // Verify NFT address saved and pool instantiation triggered
    assert_eq!(
        TEMPNFTADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("nft_address")
    );
    assert_eq!(res.messages.len(), 1); // Pool instantiate message

    // Verify creation state was updated
    let updated_state = CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::NftCreated);
    assert_eq!(
        updated_state.mint_new_position_nft_address,
        Some(Addr::unchecked("nft_address"))
    );

    // Simulate pool instantiation reply
    let pool_reply = create_instantiate_reply(FINALIZE_POOL, "pool_address");
    let res = reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    let creator = Addr::unchecked(ADMIN);
    let commit_info = SETCOMMIT.load(&deps.storage, &creator.to_string()).unwrap();
    assert_eq!(commit_info.pool_id, pool_id);
    assert_eq!(commit_info.creator_pool_addr, Addr::unchecked("pool_address"));

    // Verify pool saved by ID
    let pool_by_id = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    assert_eq!(pool_by_id.creator_pool_addr, Addr::unchecked("pool_address"));

    // Verify TEMP states cleared
    assert!(TEMPPOOLID.load(&deps.storage).is_err());
    assert!(TEMPPOOLINFO.load(&deps.storage).is_err());
    assert!(TEMPCREATORWALLETADDR.load(&deps.storage).is_err());
    assert!(TEMPCREATORTOKENADDR.load(&deps.storage).is_err());
    assert!(TEMPNFTADDR.load(&deps.storage).is_err());

    // Verify creation state shows completed
    let final_state = CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(final_state.status, CreationStatus::Completed);
    assert_eq!(
        final_state.pool_address,
        Some(Addr::unchecked("pool_address"))
    );

    // Verify minter update messages sent
    assert_eq!(res.messages.len(), 2); // token minter + NFT ownership
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
        info: TokenType::CreatorToken{
            contract_addr: Addr::unchecked("bluechip..."),
        },
        amount: Uint128::new(100),
    };

    // Test bluechip token methods
    assert!(bluechip_asset.is_bluechip_token());
    assert!(!token_asset.is_bluechip_token());

    // Test tax computation (should be zero as per implementation)
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

    // Test config values
    assert_eq!(config.factory_admin_address, Addr::unchecked("admin1..."));
    assert_eq!(config.cw20_token_contract_id, 1);
    assert_eq!(config.create_pool_wasm_contract_id, 1);
    assert_eq!(config.bluechip_wallet_address, Addr::unchecked("bluechip1..."));
    assert_eq!(config.commit_fee_bluechip, Decimal::percent(10));
    assert_eq!(config.commit_fee_creator, Decimal::percent(10));

    // Test total amounts add up
}

#[test]
fn test_update_config() {
    let mut deps = mock_dependencies(&[]);

    // Initialize with first config
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

    // Initialize contract
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

    // Set up the pool ID first
    let pool_id = 1u64;
    TEMPPOOLID.save(deps.as_mut().storage, &pool_id).unwrap();

    // CREATE THE MISSING CREATION STATE - This is what's causing the failure
    let creation_state = CreationState {
        pool_id,
        creator: addr.clone(),
        creator_token_address: None, // Will be set during token reply
        mint_new_position_nft_address: None,
        pool_address: None,
        creation_time: env.block.time,
        status: CreationStatus::Started,
        retry_count: 0,
    };
    CREATION_STATES
        .save(deps.as_mut().storage, pool_id, &creation_state)
        .unwrap();

    // Set up other temporary storage
    let pool_msg = CreatePool {
        pool_token_info: [
            TokenType::Bluechip {
                denom: "bluechip".to_string(),
            },
            TokenType::CreatorToken{
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
    TEMPCREATORWALLETADDR.save(deps.as_mut().storage, &addr).unwrap();

    // Create token instantiation reply with events
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

    let res = reply(deps.as_mut(), env.clone(), reply_msg).unwrap();

    assert_eq!(res.attributes.len(), 3);
    assert_eq!(res.attributes[0], ("action", "token_created_successfully")); // Updated message
    assert_eq!(res.attributes[1], ("token_address", contract_addr));
    assert_eq!(res.attributes[2], ("pool_id", "1"));

    // Verify the creation state was updated
    let updated_state = CREATION_STATES
        .load(deps.as_ref().storage, pool_id)
        .unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.creator_token_address,
        Some(Addr::unchecked(contract_addr))
    );

    // Verify temp storage was updated
    let temp_token = TEMPCREATORTOKENADDR.load(deps.as_ref().storage).unwrap();
    assert_eq!(temp_token, Addr::unchecked(contract_addr));
}
