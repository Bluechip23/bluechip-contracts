use crate::state::{
    Config, NEXT_POOL_ID, POOLS_BY_ID, SUBSCRIBE, TEMPCREATOR, TEMPNFTADDR, TEMPPAIRINFO,
    TEMPPOOLID, TEMPTOKENADDR,
};
use cosmwasm_std::{
    Addr, Binary, Decimal, Env, Event, OwnedDeps, Reply, SubMsgResponse, SubMsgResult, Uint128
};

use crate::asset::{Asset, AssetInfo, PairInfo, PairType};
use crate::execute::{execute, instantiate, reply};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{ExecuteMsg, OfficialInstantiateMsg, TokenInfo};
use crate::pair::{FeeInfo, PairInstantiateMsg};
use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage};

use crate::error::ContractError;

use crate::response::MsgInstantiateContractResponse;
use protobuf::Message;

const ADMIN: &str = "admin";
const USER: &str = "user";

fn create_default_instantiate_msg() -> OfficialInstantiateMsg {
    OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked(ADMIN),
            position_nft_id: 58,
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::percent(1),
            creator_fee: Decimal::percent(5),
        },
    }
}

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let _owner = "owner0000".to_string();

    let msg = OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked(ADMIN),
            position_nft_id: 58,
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::percent(10),
            creator_fee: Decimal::percent(10),
        },
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

    let msg = OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            commit_limit: Uint128::new(100),
            position_nft_id: 58,
            commit_limit_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::percent(1), // 1%
            creator_fee: Decimal::percent(5),   // 5%
        },
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
            pair_msg: PairInstantiateMsg {
                asset_infos: asset_infos.clone(),
                token_code_id: 10,
                factory_addr: Addr::unchecked("factory"),
                init_params: None,
                fee_info: FeeInfo {
                    bluechip_address: Addr::unchecked("bluechip"),
                    creator_address: Addr::unchecked("creator"),
                    bluechip_fee: Decimal::percent(1),
                    creator_fee: Decimal::percent(5),
                },
                commit_limit: Uint128::new(100),
                commit_limit_usd: Uint128::new(25_000_000_000),
                oracle_addr: Addr::unchecked("oracle0000"),
                oracle_symbol: "BLUECHIP".to_string(),
                token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                available_payment: vec![Uint128::new(1_000_000)],
                available_payment_usd: vec![Uint128::new(1_000_000)],
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
    assert_eq!(res.messages.len(), 1);
    assert_eq!(res.attributes.len(), 2);
    assert_eq!(res.attributes[0], ("action", "create"));
    assert_eq!(res.attributes[1].key, "creator");
}

#[test]
fn test_create_pair_with_custom_params() {
    let mut deps = mock_dependencies(&[]);

    // Initialize factory first
    let msg = OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked(ADMIN),
            commit_limit: Uint128::new(100),
            position_nft_id: 58,
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::percent(1),
            creator_fee: Decimal::percent(5),
        },
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Create pair with custom init params
    let custom_params = Binary::from(b"custom_pool_params");

    let create_msg = ExecuteMsg::Create {
        pair_msg: PairInstantiateMsg {
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
            init_params: Some(custom_params),
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked(ADMIN),
                bluechip_fee: Decimal::percent(1),
                creator_fee: Decimal::percent(5),
            },
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            available_payment: vec![
                Uint128::new(1_000_000),
                Uint128::new(5_000_000),
                Uint128::new(10_000_000),
            ],
            available_payment_usd: vec![
                Uint128::new(1_000_000_000),  // $1k
                Uint128::new(5_000_000_000),  // $5k
                Uint128::new(10_000_000_000), // $10k
            ],
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

    // Verify TEMP storage was set
    let temp_pair_info = TEMPPAIRINFO.load(&deps.storage).unwrap();
    assert_eq!(temp_pair_info.available_payment.len(), 3);
    assert_eq!(temp_pair_info.available_payment_usd.len(), 3);
}
fn create_pool_msg(token_name: &str) -> ExecuteMsg {
    ExecuteMsg::Create {
        pair_msg: PairInstantiateMsg {
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
            init_params: None,
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked("creator"),
                bluechip_fee: Decimal::percent(1),
                creator_fee: Decimal::percent(5),
            },
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(25_000_000_000),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_address: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
            available_payment: vec![Uint128::new(1_000_000)],
            available_payment_usd: vec![Uint128::new(1_000_000)],
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
    // Simulate token reply
    let token_reply = create_instantiate_reply(1, &format!("token_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // Simulate NFT reply
    let nft_reply = create_instantiate_reply(2, &format!("nft_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    // Simulate pool reply
    let pool_reply = create_instantiate_reply(3, &format!("pool_address_{}", pool_id));
    reply(deps.as_mut(), env.clone(), pool_reply).unwrap();
}
// Keep all your existing test functions below...
#[test]
fn test_asset_info() {
    // Your existing test_asset_info implementation
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
                Event::new("instantiate")
                    .add_attribute("_contract_address", contract_addr)
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
        execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

        // Verify pool ID was set correctly in TEMP storage
        assert_eq!(TEMPPOOLID.load(&deps.storage).unwrap(), expected_id);

        // Simulate complete reply chain
        simulate_complete_reply_chain(&mut deps, env.clone(), expected_id);

        // Verify next pool ID incremented
        assert_eq!(NEXT_POOL_ID.load(&deps.storage).unwrap(), expected_id + 1);
    }
}
#[test]
fn test_complete_pool_creation_flow() {
    let mut deps = mock_dependencies(&[]);

    // Initialize factory
    let msg = OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked(ADMIN),
            position_nft_id: 58,
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(25_000_000_000), // $25k in 6 decimals
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "BLUECHIP".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::percent(1), // 1%
            creator_fee: Decimal::percent(5),   // 5%
        },
    };

    let env = mock_env();
    let info = mock_info(ADMIN, &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let create_msg = ExecuteMsg::Create {
        pair_msg: PairInstantiateMsg {
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
            init_params: None,
            fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked("addr0000"),
                bluechip_fee: Decimal::from_ratio(10u128, 100u128),
                creator_fee: Decimal::from_ratio(10u128, 100u128),
            },
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_address: Addr::unchecked("token0000"),
            available_payment: vec![Uint128::new(100)],
            available_payment_usd: vec![Uint128::new(100)],
        },
        token_info: TokenInfo {
            name: "Test Token".to_string(),
            symbol: "TEST".to_string(),
            decimal: 6,
        },
    };

    let info = mock_info(ADMIN, &[]);
    let res = execute(deps.as_mut(), env.clone(), info, create_msg).unwrap();

    // Verify TEMP states are set
    assert!(TEMPPOOLID.load(&deps.storage).is_ok());
    assert!(TEMPPAIRINFO.load(&deps.storage).is_ok());
    assert!(TEMPCREATOR.load(&deps.storage).is_ok());

    // Simulate token instantiation reply
    let token_reply = create_instantiate_reply(1, "token_address");
    let res = reply(deps.as_mut(), env.clone(), token_reply).unwrap();

    // Verify token address saved and NFT instantiation triggered
    assert_eq!(
        TEMPTOKENADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("token_address")
    );
    assert_eq!(res.messages.len(), 1); // NFT instantiate message

    // Simulate NFT instantiation reply
    let nft_reply = create_instantiate_reply(2, "nft_address");
    let res = reply(deps.as_mut(), env.clone(), nft_reply).unwrap();

    // Verify NFT address saved and pool instantiation triggered
    assert_eq!(
        TEMPNFTADDR.load(&deps.storage).unwrap(),
        Addr::unchecked("nft_address")
    );
    assert_eq!(res.messages.len(), 1); // Pool instantiate message

    // Simulate pool instantiation reply
    let pool_reply = create_instantiate_reply(3, "pool_address");
    let res = reply(deps.as_mut(), env.clone(), pool_reply).unwrap();

    // Verify subscription saved
    let creator = Addr::unchecked(ADMIN);
    let subscribe_info = SUBSCRIBE.load(&deps.storage, &creator.to_string()).unwrap();
    assert_eq!(subscribe_info.pool_id, 1u64);
    assert_eq!(subscribe_info.pool_addr, Addr::unchecked("pool_address"));

    // Verify pool saved by ID
    let pool_by_id = POOLS_BY_ID.load(&deps.storage, 1u64).unwrap();
    assert_eq!(pool_by_id.pool_addr, Addr::unchecked("pool_address"));

    // Verify TEMP states cleared
    assert!(TEMPPOOLID.load(&deps.storage).is_err());
    assert!(TEMPPAIRINFO.load(&deps.storage).is_err());
    assert!(TEMPCREATOR.load(&deps.storage).is_err());
    assert!(TEMPTOKENADDR.load(&deps.storage).is_err());
    assert!(TEMPNFTADDR.load(&deps.storage).is_err());

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
        liquidity_token: Addr::unchecked("lp1..."),
        pair_type: PairType::Xyk {},
    };

    // Test pair type display
    assert_eq!(pair_info.pair_type.to_string(), "xyk");
}

#[test]
fn test_config() {
    let config = Config {
        admin: Addr::unchecked("admin1..."),
        position_nft_id: 58,
        commit_limit: Uint128::new(100),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_id: 1,
        pair_id: 1,
        bluechip_address: Addr::unchecked("bluechip1..."),
        bluechipe_fee: Decimal::percent(10),
        creator_fee: Decimal::percent(10),
    };

    // Test config values
    assert_eq!(config.admin, Addr::unchecked("admin1..."));
    assert_eq!(config.token_id, 1);
    assert_eq!(config.pair_id, 1);
    assert_eq!(config.bluechip_address, Addr::unchecked("bluechip1..."));
    assert_eq!(config.bluechipe_fee, Decimal::percent(10));
    assert_eq!(config.creator_fee, Decimal::percent(10));

    // Test total amounts add up
}

#[test]
fn test_update_config() {
    let mut deps = mock_dependencies(&[]);

    // Initialize with first config
    let msg = OfficialInstantiateMsg {
        config: Config {
            position_nft_id: 58,
            admin: Addr::unchecked("addr0000"),
            commit_limit: Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    // Try updating with non-admin
    let unauthorized_info = mock_info("unauthorized", &[]);
    let update_msg = ExecuteMsg::UpdateConfig {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            commit_limit: Uint128::new(100),
            position_nft_id: 58,
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
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
    let msg = OfficialInstantiateMsg {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            commit_limit: Uint128::new(100),
            position_nft_id: 58,
            commit_limit_usd: Uint128::new(100),
            oracle_addr: Addr::unchecked("oracle0000"),
            oracle_symbol: "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = mock_info(&addr.as_str(), &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create token instantiation reply with events (not data)
    let contract_addr = "token_contract_address";
    
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![
                Event::new("instantiate")
                    .add_attribute("_contract_address", contract_addr)
            ],
            data: None, // Your contract doesn't use data, it uses events
        }),
    };

    // Set up temporary storage for reply
    TEMPPOOLID.save(deps.as_mut().storage, &1u64).unwrap(); // Add pool ID
    
    let pair_msg = PairInstantiateMsg {
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
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("addr0000"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(100),
        commit_limit_usd: Uint128::new(100),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        token_address: Addr::unchecked("token0000"),
        available_payment: vec![Uint128::new(100)],
        available_payment_usd: vec![Uint128::new(100)],
    };

    TEMPPAIRINFO.save(deps.as_mut().storage, &pair_msg).unwrap();
    TEMPCREATOR.save(deps.as_mut().storage, &addr).unwrap();

    let res = reply(deps.as_mut(), env.clone(), reply_msg).unwrap();
    
    // Check the correct attributes
    assert_eq!(res.attributes.len(), 2);
    assert_eq!(res.attributes[0], ("action", "instantiate_token_reply"));
    assert_eq!(res.attributes[1], ("token_address", contract_addr));
}