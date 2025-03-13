use cosmwasm_std::{
    Addr, Decimal, Uint128,
};

use crate::mock_querier::mock_dependencies;
use crate::state::Config;
use crate::execute::{execute, instantiate};

use crate::asset::AssetInfo;
use crate::msg::{ExecuteMsg, InstantiateMsg, TokenInfo};
use cosmwasm_std::testing::{message_info, mock_env};
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
