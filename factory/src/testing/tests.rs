use cosmwasm_std::{
    attr, from_binary, to_binary, Addr, Decimal, Reply, ReplyOn, SubMsg, SubMsgResponse,
    SubMsgResult, Uint128, WasmMsg,
};

use crate::mock_querier::mock_dependencies;
use crate::state::{Config, CONFIG};
use crate::{
    error::ContractError,
    execute::{execute, instantiate},
    query::query,
};

use crate::asset::{AssetInfo, PairInfo};
use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, QueryMsg, TokenInfo};

use crate::pair::{FeeInfo, InstantiateMsg as PairInstantiateMsg};
use crate::response::MsgInstantiateContractResponse;
use cosmwasm_std::testing::{mock_env, mock_info, MOCK_CONTRACT_ADDR};
use protobuf::Message;

#[test]
fn proper_initialization() {
    // Validate total and maker fee bps
    let mut deps = mock_dependencies(&[]);
    let owner = "owner0000".to_string();

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
    let info = mock_info("addr0000", &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap_err();

    let env = mock_env();
    let info = mock_info("addr0000", &[]);

    let _res = instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap_err();

    let mut deps = mock_dependencies(&[]);

    let env = mock_env();
    let info = mock_info("addr0000", &[]);

    instantiate(deps.as_mut(), env.clone(), info, msg.clone()).unwrap();

    let query_res = query(deps.as_ref(), env, QueryMsg::Config {}).unwrap();
    let config_res: ConfigResponse = from_binary(&query_res).unwrap();
}

#[test]
fn create_pair() {
    let mut deps = mock_dependencies(&[]);

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
    let info = mock_info("addr0000", &[]);

    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg.clone()).unwrap();

    let asset_infos = [
        AssetInfo::Token {
            contract_addr: Addr::unchecked("asset0000"),
        },
        AssetInfo::Token {
            contract_addr: Addr::unchecked("asset0001"),
        },
    ];

    let config = CONFIG.load(&deps.storage);
    let env = mock_env();
    let info = mock_info("addr0000", &[]);

    let asset_infos = [
        AssetInfo::NativeToken {
            denom: "bluechip".to_string(),
        },
        AssetInfo::Token {
            contract_addr: Addr::unchecked("asset0001"),
        },
    ];

    // Check pair creation using a non-whitelisted pair ID
    let res = execute(
        deps.as_mut(),
        env.clone(),
        info.clone(),
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
