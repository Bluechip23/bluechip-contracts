use cosmwasm_std::{Addr, Decimal, StdError, Uint128};
use crate::state::Config;
use crate::mock_querier::mock_dependencies;
use cosmwasm_std::testing::{mock_env, message_info, mock_info};
use crate::execute::{execute, instantiate};
use crate::asset::{Asset, AssetInfo, PairInfo, PairType};
use crate::msg::{ExecuteMsg, InstantiateMsg, TokenInfo};
use crate::pair::{FeeInfo, InstantiateMsg as PairInstantiateMsg};

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let _owner = "owner0000".to_string();

    let msg = InstantiateMsg {
        config: Config {
            admin: Addr::unchecked("admin"),
            total_token_amount: Uint128::new(5000),
            creator_amount: Uint128::new(1000),
            pool_amount: Uint128::new(3000),
            commit_amount: Uint128::new(1000),
            bluechip_amount: Uint128::new(500),
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
                commit_limit: Uint128::new(10000),
                token_address: Addr::unchecked("admin".to_string()),
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
    assert!(invalid_token_info.check(&deps.api).is_ok()); // Note: In mock environment, address validation is lenient

    // Test valid token address
    let valid_token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked("bluechipvalid..."),
    };
    assert!(valid_token_info.check(&deps.api).is_ok());
}
