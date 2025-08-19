use crate::state::{
    CreationState, CreationStatus, FactoryInstantiate, COMMIT, CREATION_STATES, NEXT_POOL_ID,
    POOLS_BY_ID, TEMPCREATOR, TEMPNFTADDR, TEMPPAIRINFO, TEMPPOOLID, TEMPTOKENADDR,
};
use cosmwasm_std::{
    Addr, Binary, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse, SubMsgResult, Uint128,
};

use crate::asset::{Asset, AssetInfo, PairInfo, PairType};
use crate::execute::{execute, instantiate, reply, FINALIZE_POOL, MINT_CREATE_POOL, SET_TOKENS};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{ExecuteMsg, TokenInfo};
use crate::pair::{CreatePool, FeeInfo};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage};

const ADMIN: &str = "admin";

fn create_default_instantiate_msg() -> FactoryInstantiate {
    FactoryInstantiate {
        admin: Addr::unchecked(ADMIN),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(25_000_000_000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "BLUECHIP".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::percent(1),
        creator_fee: Decimal::percent(5),
    }
}

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let _owner = "owner0000".to_string();

    let msg = FactoryInstantiate {
        admin: Addr::unchecked(ADMIN),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::percent(10),
        creator_fee: Decimal::percent(10),
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
        admin: Addr::unchecked("addr0000"),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "BLUECHIP".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::percent(1), // 1%
        creator_fee: Decimal::percent(5),  // 5%
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let asset_infos = [
        AssetInfo::NativeToken {
            denom: "bluechip".to_string(),
        },
        AssetInfo::Token {
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
                asset_infos: asset_infos.clone(),
                token_code_id: 10,
                factory_addr: Addr::unchecked("factory"),
                threshold_payout: None,
                fee_info: FeeInfo {
                    bluechip_address: Addr::unchecked("bluechip"),
                    creator_address: Addr::unchecked("creator"),
                    bluechip_fee: Decimal::percent(1),
                    creator_fee: Decimal::percent(5),
                },
                commit_amount_for_threshold: Uint128::zero(),
                commit_limit_usd: Uint128::new(25_000_000_000),
                oracle_addr: Addr::unchecked("oracle0000"),
                oracle_symbol: "BLUECHIP".to_string(),
                token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            },
            token_info: TokenInfo {
                name: "Test Token".to_string(),
                symbol: "TEST".to_string(),
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
        admin: Addr::unchecked(ADMIN),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(25_000_000_000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "BLUECHIP".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::percent(1),
        creator_fee: Decimal::percent(5),
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Create pair with custom init params
    let custom_params = Binary::from(b"custom_pool_params");

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            asset_infos: [
                AssetInfo::NativeToken {
                    denom: "bluechip".to_string(),
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            token_code_id: 10,
            factory_addr: Addr::unchecked("factory"),
            threshold_payout: Some(custom_params),
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked(ADMIN),
                bluechip_fee: Decimal::percent(1),
                creator_fee: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
        token_info: TokenInfo {
            name: "Custom Token".to_string(),
            symbol: "CUSTOM".to_string(),
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
            asset_infos: [
                AssetInfo::NativeToken {
                    denom: "bluechip".to_string(),
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
            token_code_id: 10,
            factory_addr: Addr::unchecked("factory"),
            threshold_payout: None,
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked("creator"),
                bluechip_fee: Decimal::percent(1),
                creator_fee: Decimal::percent(5),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
        },
        token_info: TokenInfo {
            name: token_name.to_string(),
            symbol: token_name.to_string(),
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
    let native_info = AssetInfo::NativeToken {
        denom: "bluechip".to_string(),
    };
    assert!(native_info.is_native_token());

    let token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked("bluechip..."),
    };
    assert!(!token_info.is_native_token());

    assert!(native_info.equal(&AssetInfo::NativeToken {
        denom: "bluechip".to_string(),
    }));
    assert!(!native_info.equal(&token_info));
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
        let creator = TEMPCREATOR.load(&deps.storage).unwrap();
        let creation_state = CreationState {
            pool_id,
            creator: creator.clone(),
            token_address: None,
            nft_address: None,
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
        admin: Addr::unchecked(ADMIN),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(25_000_000_000), // $25k in 6 decimals
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "BLUECHIP".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::percent(1), // 1%
        creator_fee: Decimal::percent(5),  // 5%
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            asset_infos: [
                AssetInfo::NativeToken {
                    denom: "bluechip".to_string(),
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked("token0000"),
                },
            ],
            factory_addr: Addr::unchecked("factory"),
            token_code_id: 10,
            threshold_payout: None,
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked("addr0000"),
                bluechip_fee: Decimal::from_ratio(10u128, 100u128),
                creator_fee: Decimal::from_ratio(10u128, 100u128),
            },
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_address: Addr::unchecked("token0000"),
        },
        token_info: TokenInfo {
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
    // Verify TEMP states are set
    assert!(TEMPPOOLID.load(&deps.storage).is_ok());
    assert!(TEMPPAIRINFO.load(&deps.storage).is_ok());
    assert!(TEMPCREATOR.load(&deps.storage).is_ok());

    // GET THE POOL ID AND SET UP CREATION STATE
    let pool_id = TEMPPOOLID.load(&deps.storage).unwrap();
    let creator = TEMPCREATOR.load(&deps.storage).unwrap();

    // Create the CreationState that your new code expects
    let creation_state = CreationState {
        pool_id,
        creator: creator.clone(),
        token_address: None,
        nft_address: None,
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
        TEMPTOKENADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("token_address")
    );
    assert_eq!(res.messages.len(), 1); // NFT instantiate message

    // Verify creation state was updated
    let updated_state = CREATION_STATES.load(&deps.storage, pool_id).unwrap();
    assert_eq!(updated_state.status, CreationStatus::TokenCreated);
    assert_eq!(
        updated_state.token_address,
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
        updated_state.nft_address,
        Some(Addr::unchecked("nft_address"))
    );

    // Simulate pool instantiation reply
    let pool_reply = create_instantiate_reply(FINALIZE_POOL, "pool_address");
    let res = reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    let creator = Addr::unchecked(ADMIN);
    let commit_info = COMMIT.load(&deps.storage, &creator.to_string()).unwrap();
    assert_eq!(commit_info.pool_id, pool_id);
    assert_eq!(commit_info.pool_addr, Addr::unchecked("pool_address"));

    // Verify pool saved by ID
    let pool_by_id = POOLS_BY_ID.load(&deps.storage, pool_id).unwrap();
    assert_eq!(pool_by_id.pool_addr, Addr::unchecked("pool_address"));

    // Verify TEMP states cleared
    assert!(TEMPPOOLID.load(&deps.storage).is_err());
    assert!(TEMPPAIRINFO.load(&deps.storage).is_err());
    assert!(TEMPCREATOR.load(&deps.storage).is_err());
    assert!(TEMPTOKENADDR.load(&deps.storage).is_err());
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
    let native_asset = Asset {
        info: AssetInfo::NativeToken {
            denom: "bluechip".to_string(),
        },
        amount: Uint128::new(100),
    };

    let token_asset = Asset {
        info: AssetInfo::Token {
            contract_addr: Addr::unchecked("bluechip..."),
        },
        amount: Uint128::new(100),
    };

    // Test native token methods
    assert!(native_asset.is_native_token());
    assert!(!token_asset.is_native_token());

    // Test tax computation (should be zero as per implementation)
    let deps = mock_dependencies(&[]);
    assert_eq!(
        native_asset.compute_tax(&deps.as_ref().querier).unwrap(),
        Uint128::zero()
    );
}

#[test]
fn test_pair_info() {
    let pair_info = PairInfo {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "bluechip".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("bluechip..."),
            },
        ],
        contract_addr: Addr::unchecked("pair1..."),
        pair_type: PairType::Xyk {},
    };

    // Test pair type display
    assert_eq!(pair_info.pair_type.to_string(), "xyk");
}

#[test]
fn test_config() {
    let config = FactoryInstantiate {
        admin: Addr::unchecked("admin1..."),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_id: 1,
        pair_id: 1,
        bluechip_address: Addr::unchecked("bluechip1..."),
        bluechip_fee: Decimal::percent(10),
        creator_fee: Decimal::percent(10),
    };

    // Test config values
    assert_eq!(config.admin, Addr::unchecked("admin1..."));
    assert_eq!(config.token_id, 1);
    assert_eq!(config.pair_id, 1);
    assert_eq!(config.bluechip_address, Addr::unchecked("bluechip1..."));
    assert_eq!(config.bluechip_fee, Decimal::percent(10));
    assert_eq!(config.creator_fee, Decimal::percent(10));

    // Test total amounts add up
}

#[test]
fn test_update_config() {
    let mut deps = mock_dependencies(&[]);

    // Initialize with first config
    let msg = FactoryInstantiate {
        position_nft_id: 58,
        admin: Addr::unchecked("addr0000"),
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::from_ratio(10u128, 100u128),
        creator_fee: Decimal::from_ratio(10u128, 100u128),
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    // Try updating with non-admin
    let unauthorized_info = mock_info("unauthorized", &[]);
    let update_msg = ExecuteMsg::UpdateConfig {
        config: FactoryInstantiate {
            admin: Addr::unchecked("addr0000"),
            position_nft_id: 58,
            commit_amount_for_threshold: Uint128::zero(),
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
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
        admin: Addr::unchecked("addr0000"),
        position_nft_id: 58,
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_id: 10,
        pair_id: 11,
        bluechip_address: Addr::unchecked("bluechip"),
        bluechip_fee: Decimal::from_ratio(10u128, 100u128),
        creator_fee: Decimal::from_ratio(10u128, 100u128),
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
        token_address: None, // Will be set during token reply
        nft_address: None,
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
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "bluechip".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("token0000"),
            },
        ],
        factory_addr: Addr::unchecked("factory"),
        token_code_id: 10,
        threshold_payout: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("addr0000"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_amount_for_threshold: Uint128::zero(),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_address: Addr::unchecked("token0000"),
    };

    TEMPPAIRINFO.save(deps.as_mut().storage, &pool_msg).unwrap();
    TEMPCREATOR.save(deps.as_mut().storage, &addr).unwrap();

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
        updated_state.token_address,
        Some(Addr::unchecked(contract_addr))
    );

    // Verify temp storage was updated
    let temp_token = TEMPTOKENADDR.load(deps.as_ref().storage).unwrap();
    assert_eq!(temp_token, Addr::unchecked(contract_addr));
}
