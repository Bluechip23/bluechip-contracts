
use cosmwasm_std::{Addr, Decimal, Reply, SubMsgResponse, SubMsgResult, Uint128};
use crate::state::{Config, TEMPPAIRINFO, TEMPCREATOR};

use crate::mock_querier::mock_dependencies;
use cosmwasm_std::testing::{mock_env, message_info};
use crate::execute::{execute, instantiate, reply};
use crate::asset::{Asset, AssetInfo, PairInfo, PairType};
use crate::msg::{ExecuteMsg, InstantiateMsg, TokenInfo};
use crate::pair::{FeeInfo, InstantiateMsg as PairInstantiateMsg};

use crate::error::ContractError;

use protobuf::Message;
use crate::response::MsgInstantiateContractResponse;

const ADMIN: &str = "admin";
const USER: &str = "user";

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let _owner = "owner0000".to_string();

    let msg = InstantiateMsg {
        config: Config {
            admin:              Addr::unchecked(ADMIN),
            total_token_amount: Uint128::new(5_000),
            creator_amount:     Uint128::new(1_000),
            pool_amount:        Uint128::new(3_000),
            commit_amount:      Uint128::new(1_000),
            bluechip_amount:    Uint128::new(500),
            commit_limit:       Uint128::new(100),
            commit_limit_usd:   Uint128::new(100),
            oracle_addr:        Addr::unchecked("oracle0000"),
            oracle_symbol:      "ORCL".to_string(),
            token_id:           10,
            pair_id:            11,
            bluechip_address:   Addr::unchecked("bluechip"),
            bluechipe_fee:      Decimal::percent(10),
            creator_fee:        Decimal::percent(10),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = message_info(&addr, &[]);


    println!("addr: {:?}", addr);
    println!("info: {:?}", info);


    let _res0 = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap_or_else(|e| {
        println!("error: {:?}", e);
        panic!("error: {:?}", e);
    });

    println!("result: {:?}", _res0);

    let env = mock_env();

    let addr = Addr::unchecked("addr0001");

    let info = message_info(&addr, &[]);

    let _res1 = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap_or_else(|e| {
        println!("error: {:?}", e);
        panic!("error: {:?}", e);
    });

    let mut deps = mock_dependencies(&[]);

    let env = mock_env();

    let addr = Addr::unchecked("addr0002");

    let info = message_info(&addr, &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();


    // let query_res = query(deps.as_ref(), env, QueryMsg::Config {}).unwrap();
    // let config_res: ConfigResponse = from_binary(&query_res).unwrap();

}

#[test]
fn create_pair() {
    let mut deps = mock_dependencies(&[]);

    let msg = InstantiateMsg {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            total_token_amount: Uint128::new(5000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(3000),
            commit_amount: Uint128::new(1000),
            bluechip_amount: Uint128::new(500),
            commit_limit:     Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr:     Addr::unchecked("oracle0000"),
            oracle_symbol:   "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            bluechipe_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = message_info(&addr, &[]);

    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let asset_infos = [
        AssetInfo::NativeToken {
            denom: "bluechip".to_string(),
        },
        AssetInfo::Token {
            contract_addr: Addr::unchecked("asset0001"),
        },
    ];

    // Create new env and info for execute
    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = message_info(&addr, &[]);

    // Check pair creation using a non-whitelisted pair ID
    let _res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::Create {
            pair_msg: PairInstantiateMsg {
                asset_infos: asset_infos.clone(),
                init_params: None,
                token_code_id: 10,
                factory_addr: "admin".to_string(),
                fee_info: FeeInfo {
                    bluechip_address: Addr::unchecked("bluechip".to_string()),
                    creator_address: Addr::unchecked("creator".to_string()),
                    bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
                    creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
                },
                 commit_amount:     Uint128::new(10_000),
            pool_amount:       Uint128::new(10_000),
            creator_amount:    Uint128::new(10_000),
            bluechip_amount:   Uint128::new(10_000),
            commit_limit:      Uint128::new(10_000),
            commit_limit_usd:  Uint128::new(10_000),
            oracle_addr:       Addr::unchecked("oracle0000"),
            oracle_symbol:     "ORCL".to_string(),
            token_address:     Addr::unchecked("admin"),
            },
            token_info: TokenInfo {
                name: "commit".to_string(),
                decimal: 8,
                symbol: "commit".to_string(),
            },
        },
    )
    .unwrap();
}

#[test]
fn test_asset_info() {
    let mut deps = mock_dependencies(&[]);

    // Test native token
    let native_info = AssetInfo::NativeToken {
        denom: "bluechip".to_string(),
    };
    assert!(native_info.is_native_token());
    assert!(!native_info.is_ibc());

    // Test token
    let token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked("bluechip..."),
    };
    assert!(!token_info.is_native_token());
    assert!(!token_info.is_ibc());

    // Test equality
    assert!(native_info.equal(&AssetInfo::NativeToken {
        denom: "bluechip".to_string(),
    }));
    assert!(!native_info.equal(&token_info));

    // Test validation
    // native_info.check(&deps.api).unwrap();
    // token_info.check(&deps.api).unwrap();
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
        total_token_amount: Uint128::new(1_000_000),
        creator_amount: Uint128::new(200_000),
        pool_amount: Uint128::new(500_000),
        commit_amount: Uint128::new(200_000),
        commit_limit:     Uint128::new(100),
        commit_limit_usd: Uint128::new(100),
        oracle_addr:     Addr::unchecked("oracle0000"),
        oracle_symbol:   "ORCL".to_string(),
        bluechip_amount: Uint128::new(100_000),
        token_id: 1,
        pair_id: 1,
        bluechip_address: Addr::unchecked("bluechip1..."),
        bluechipe_fee: Decimal::percent(10),
        creator_fee: Decimal::percent(10),
    };

    // Test config values
    assert_eq!(config.admin, Addr::unchecked("admin1..."));
    assert_eq!(config.total_token_amount, Uint128::new(1_000_000));
    assert_eq!(config.creator_amount, Uint128::new(200_000));
    assert_eq!(config.pool_amount, Uint128::new(500_000));
    assert_eq!(config.commit_amount, Uint128::new(200_000));
    assert_eq!(config.bluechip_amount, Uint128::new(100_000));
    assert_eq!(config.token_id, 1);
    assert_eq!(config.pair_id, 1);
    assert_eq!(config.bluechip_address, Addr::unchecked("bluechip1..."));
    assert_eq!(config.bluechipe_fee, Decimal::percent(10));
    assert_eq!(config.creator_fee, Decimal::percent(10));

    // Test total amounts add up
    assert_eq!(
        config.creator_amount + config.pool_amount + config.commit_amount + config.bluechip_amount,
        config.total_token_amount
    );
}

#[test]
fn test_asset_validation() {
    let mut deps = mock_dependencies(&[]);
    let env = mock_env();

    // Test native token validation
    let native_info = AssetInfo::NativeToken {
        denom: "bluechip".to_string(),
    };
    assert!(native_info.check(&deps.api).is_ok());

    // Test invalid token address
    let invalid_token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked("invalid..."),
    };
    assert!(invalid_token_info.check(&deps.api).is_err());
 // Note: In mock environment, address validation is lenient

    // Test valid token address
    let valid_token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked("bluechipvalid..."),
    };
    assert!(invalid_token_info.check(&deps.api).is_err());
}

#[test]
fn test_update_config() {
    let mut deps = mock_dependencies(&[]);

    // Initialize with first config
    let msg = InstantiateMsg {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            total_token_amount: Uint128::new(5000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(3000),
            commit_amount: Uint128::new(500),
            bluechip_amount: Uint128::new(500),
            commit_limit:     Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr:     Addr::unchecked("oracle0000"),
            oracle_symbol:   "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = message_info(&addr, &[]);

    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    // Try updating with non-admin
    let unauthorized_info = message_info(&Addr::unchecked("unauthorized"), &[]);
    let update_msg = ExecuteMsg::UpdateConfig {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            total_token_amount: Uint128::new(6000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(4000),
            commit_amount: Uint128::new(500),
            bluechip_amount: Uint128::new(500),
            commit_limit:     Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr:     Addr::unchecked("oracle0000"),
            oracle_symbol:   "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let err = execute(deps.as_mut(), env.clone(), unauthorized_info, update_msg.clone()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Generic error: Only the admin can execute this function. Admin: addr0000, Sender: unauthorized"
    );

    // Try updating with invalid amounts
    let invalid_msg = ExecuteMsg::UpdateConfig {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            total_token_amount: Uint128::new(5000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(5000), // This makes sum > total
            commit_amount: Uint128::new(500),
            bluechip_amount: Uint128::new(500),
            commit_limit:     Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr:     Addr::unchecked("oracle0000"),
            oracle_symbol:   "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let err = execute(deps.as_mut(), env.clone(), info.clone(), invalid_msg).unwrap_err();
    match err {
        ContractError::WrongConfiguration {} => {}
        _ => panic!("Expected WrongConfiguration error"),
    }

    // Update config successfully
    let res = execute(deps.as_mut(), env.clone(), info, update_msg).unwrap();
    assert_eq!(1, res.attributes.len());
    assert_eq!(("action", "update_config"), res.attributes[0]);
}

#[test]
fn test_reply_handling() {
    let mut deps = mock_dependencies(&[]);

    // Initialize contract
    let msg = InstantiateMsg {
        config: Config {
            admin: Addr::unchecked("addr0000"),
            total_token_amount: Uint128::new(5000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(3000),
            commit_amount: Uint128::new(500),
            bluechip_amount: Uint128::new(500),
            commit_limit:     Uint128::new(100),
            commit_limit_usd: Uint128::new(100),
            oracle_addr:     Addr::unchecked("oracle0000"),
            oracle_symbol:   "ORCL".to_string(),
            token_id: 10,
            pair_id: 11,
            bluechip_address: Addr::unchecked("bluechip"),
            bluechipe_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
    };

    let env = mock_env();
    let addr = Addr::unchecked("addr0000");
    let info = message_info(&addr, &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Create token instantiation reply
  
    let contract_addr = env.contract.address.to_string();
    let mut token_response = MsgInstantiateContractResponse::new();
    token_response.set_contract_address(contract_addr.clone());
    let token_data = token_response.write_to_bytes().unwrap();
    
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(token_data.into()),
            msg_responses: vec![],
        }),
        gas_used: 0,
        payload: vec![].into(),
    };

    // Set up temporary storage for reply
    let pair_msg = PairInstantiateMsg {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "bluechip".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("token0000"),
            },
        ],
        factory_addr: String::from("factory"),
        token_code_id: 10,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("addr0000"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        creator_amount:     Uint128::new(1_000),
        pool_amount:        Uint128::new(3_000),
        commit_amount:      Uint128::new(1_000),
        bluechip_amount:    Uint128::new(500),
        commit_limit:     Uint128::new(100),
        commit_limit_usd: Uint128::new(100),
        oracle_addr:     Addr::unchecked("oracle0000"),
        oracle_symbol:   "ORCL".to_string(),
        token_address: Addr::unchecked("token0000"),
    };

    TEMPPAIRINFO.save(deps.as_mut().storage, &pair_msg).unwrap();
    TEMPCREATOR.save(deps.as_mut().storage, &addr).unwrap();

    let res = reply(deps.as_mut(), env.clone(), reply_msg).unwrap();
    assert_eq!(res.attributes, vec![("token_address", contract_addr)]);
}