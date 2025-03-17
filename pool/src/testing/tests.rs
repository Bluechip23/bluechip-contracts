use crate::asset::{Asset, AssetInfo, PairInfo};

use crate::contract::{
    accumulate_prices, assert_max_spread, compute_swap, execute, instantiate, query_pair_info,
    query_pool, query_reverse_simulation, query_simulation,
};
use crate::error::ContractError;
use crate::mock_querier::mock_dependencies;
use crate::msg::{Cw20HookMsg, ExecuteMsg, FeeInfo, InstantiateMsg};
use crate::response::MsgInstantiateContractResponse;
use crate::state::Config;

use cosmwasm_std::testing::{message_info, mock_env, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    attr, to_json_binary, Addr, BankMsg, Binary, BlockInfo, Coin, CosmosMsg, Decimal, DepsMut, Env, Fraction, Reply,
    ReplyOn, Response, StdError, SubMsg, SubMsgResponse, SubMsgResult, Timestamp, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg, MinterResponse};
use proptest::prelude::*;
use protobuf::Message;

#[allow(deprecated)]
fn store_liquidity_token(deps: DepsMut, msg_id: u64, contract_addr: String) {
    let data = MsgInstantiateContractResponse {
        contract_address: contract_addr,
        data: vec![],
        unknown_fields: Default::default(),
        cached_size: Default::default(),
    }
    .write_to_bytes()
    .unwrap();

    let reply_msg = Reply {
        id: msg_id,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(data.into()),
            msg_responses: vec![],
        }),
        gas_used: 0,
        payload: Binary::default(),
    };
}

#[test]
fn proper_initialization() {
    let mut deps = mock_dependencies(&[]);

    deps.querier.with_token_balances(&[(
        &String::from("asset0000"),
        &[(&String::from(MOCK_CONTRACT_ADDR), &Uint128::new(123u128))],
    )]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let sender = "addr0000";
    let env = mock_env();
    let info = message_info(&Addr::unchecked(sender), &[]);
    
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    println!("Instantiated");

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());
}

#[test]
fn provide_liquidity() {
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: Uint128::new(200_000000000000000000u128),
    }]);

    deps.querier.with_token_balances(&[
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &Uint128::new(0))],
        ),
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &Uint128::new(0))],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());

    // Successfully provide liquidity for the existing pool

    // It must accept 1:1 and treat the leftover amount as a donation
    deps.querier.with_balance(&[(
        &String::from(MOCK_CONTRACT_ADDR),
        &[Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(200_000000000000000000 + 200_000000000000000000 /* user deposit must be pre-applied */),
        }],
    )]);

    deps.querier.with_token_balances(&[
        (
            &String::from("liquidity0000"),
            &[(
                &String::from(MOCK_CONTRACT_ADDR),
                &Uint128::new(100_000000000000000000),
            )],
        ),
        (
            &String::from("asset0000"),
            &[(
                &String::from(MOCK_CONTRACT_ADDR),
                &Uint128::new(200_000000000000000000),
            )],
        ),
    ]);
}

#[test]
fn withdraw_liquidity() {
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: Uint128::new(100u128),
    }]);

    // deps.querier
    //     .with_tax(Decimal::zero(), &[("uusd", &Uint128::from(1000000u128))]);
    deps.querier.with_token_balances(&[
        (
            &String::from("liquidity0000"),
            &[(&String::from("addr0000"), &Uint128::new(100u128))],
        ),
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &Uint128::new(100u128))],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());

    // Withdraw liquidity
}

#[test]
fn try_native_to_token() {
    let total_share = Uint128::new(30000000000u128);
    let asset_pool_amount = Uint128::new(20000000000u128);
    let collateral_pool_amount = Uint128::new(30000000000u128);
    let offer_amount = Uint128::new(1500000000u128);

    let env = mock_env();

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount + offer_amount, /* user deposit must be pre-applied */
    }]);

    // deps.querier.with_tax(
    //     Decimal::zero(),
    //     &[(&"uusd".to_string(), &Uint128::from(1000000u128))],
    // );

    deps.querier.with_token_balances(&[
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &total_share)],
        ),
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &asset_pool_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // we can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());

    // Normal swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            amount: offer_amount,
        },
        belief_price: None,
        max_spread: Some(Decimal::percent(50)),
        to: None,
    };

    let info = message_info(&Addr::unchecked("addr0000"), &[Coin {
        denom: "uusd".to_string(),
        amount: offer_amount,
    }]);

    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    let msg_transfer = res.messages.get(0).expect("no message");

    // Current price is 1.5, so expected return without spread is 1000
    // 952380952 = 20000000000 - (30000000000 * 20000000000) / (30000000000 + 1500000000)
    let expected_ret_amount = Uint128::new(952_380_952u128);

    // 47619047 = 1500000000 * (20000000000 / 30000000000) - 952380952
    let expected_spread_amount = Uint128::new(47619047u128);

    let expected_commission_amount = expected_ret_amount.multiply_ratio(3u128, 1000u128); // 0.3%
    let expected_maker_fee_amount = expected_commission_amount.multiply_ratio(166u128, 1000u128); // 0.166

    let expected_return_amount = expected_ret_amount
        .checked_sub(expected_commission_amount)
        .unwrap();
    let expected_tax_amount = Uint128::zero(); // no tax for token

    // Check simulation result
    deps.querier.with_balance(&[(
        &String::from(MOCK_CONTRACT_ADDR),
        &[Coin {
            denom: "uusd".to_string(),
            amount: collateral_pool_amount, /* user deposit must be pre-applied */
        }],
    )]);
}

#[test]
fn try_token_to_native() {
    let total_share = Uint128::new(20000000000u128);
    let asset_pool_amount = Uint128::new(30000000000u128);
    let collateral_pool_amount = Uint128::new(20000000000u128);
    let offer_amount = Uint128::new(1500000000u128);

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount,
    }]);
    // deps.querier.with_tax(
    //     Decimal::percent(1),
    //     &[(&"uusd".to_string(), &Uint128::from(1000000u128))],
    // );
    deps.querier.with_token_balances(&[
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &total_share)],
        ),
        (
            &String::from("asset0000"),
            &[(
                &String::from(MOCK_CONTRACT_ADDR),
                &(asset_pool_amount + offer_amount),
            )],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());

    // Unauthorized access; can not execute swap directly for token swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000".to_string()),
            },
            amount: offer_amount,
        },
        belief_price: None,
        max_spread: None,
        to: None,
    };

    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
    assert_eq!(res, ContractError::Cw20DirectSwap {});

    // Normal sell
    let msg = ExecuteMsg::Receive(Cw20ReceiveMsg {
        sender: String::from("addr0000"),
        amount: offer_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(50)),
            to: None,
        })
        .unwrap(),
    });

    let info = message_info(&Addr::unchecked("asset0000"), &[]);

    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    let msg_transfer = res.messages.get(0).expect("no message");

    // Current price is 1.5, so expected return without spread is 1000
    // 952380952,3809524 = 20000000000 - (30000000000 * 20000000000) / (30000000000 + 1500000000)
    let expected_ret_amount = Uint128::new(952_380_952u128);

    // 47619047 = 1500000000 * (20000000000 / 30000000000) - 952380952,3809524
    let expected_spread_amount = Uint128::new(47619047u128);

    let expected_commission_amount = expected_ret_amount.multiply_ratio(3u128, 1000u128); // 0.3%
    let expected_maker_fee_amount = expected_commission_amount.multiply_ratio(166u128, 1000u128);
    let expected_return_amount = expected_ret_amount
        .checked_sub(expected_commission_amount)
        .unwrap();
    let expected_tax_amount = Uint128::zero();
    // check simulation res
    // return asset token balance as normal

    deps.querier.with_token_balances(&[
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &total_share)],
        ),
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &(asset_pool_amount))],
        ),
    ]);
}

#[test]
fn test_max_spread() {
    assert_max_spread(
        Some(Decimal::from_ratio(1200u128, 1u128)),
        Some(Decimal::percent(1)),
        Uint128::from(1200000000u128),
        Uint128::from(989999u128),
        Uint128::zero(),
    )
    .unwrap_err();

    assert_max_spread(
        Some(Decimal::from_ratio(1200u128, 1u128)),
        Some(Decimal::percent(1)),
        Uint128::from(1200000000u128),
        Uint128::from(990000u128),
        Uint128::zero(),
    )
    .unwrap();

    assert_max_spread(
        None,
        Some(Decimal::percent(1)),
        Uint128::zero(),
        Uint128::from(989999u128),
        Uint128::from(10001u128),
    )
    .unwrap_err();

    assert_max_spread(
        None,
        Some(Decimal::percent(1)),
        Uint128::zero(),
        Uint128::from(990000u128),
        Uint128::from(10000u128),
    )
    .unwrap();
}

#[test]
#[ignore]
fn test_deduct() {
    let deps = mock_dependencies(&[]);

    let tax_rate = Decimal::percent(2);
    let tax_cap = Uint128::from(1_000_000u128);
    // deps.querier.with_tax(
    //     Decimal::percent(2),
    //     &[(&"uusd".to_string(), &Uint128::from(1000000u128))],
    // );

    let amount = Uint128::new(1000_000_000u128);
    let expected_after_amount = std::cmp::max(
        amount.checked_sub(amount * tax_rate.numerator() / tax_rate.denominator()).unwrap(),
        amount.checked_sub(tax_cap).unwrap(),
    );

    let after_amount = (Asset {
        info: AssetInfo::NativeToken {
            denom: "uusd".to_string(),
        },
        amount,
    })
    .deduct_tax(&deps.as_ref().querier)
    .unwrap();

    assert_eq!(expected_after_amount, after_amount.amount);
}

#[test]
fn test_query_pool() {
    let total_share_amount = Uint128::from(111u128);
    let asset_0_amount = Uint128::from(222u128);
    let asset_1_amount = Uint128::from(333u128);
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: asset_0_amount,
    }]);

    deps.querier.with_token_balances(&[
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &asset_1_amount)],
        ),
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &total_share_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());
}

#[test]
fn test_query_share() {
    let total_share_amount = Uint128::from(500u128);
    let asset_0_amount = Uint128::from(250u128);
    let asset_1_amount = Uint128::from(1000u128);
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: asset_0_amount,
    }]);

    deps.querier.with_token_balances(&[
        (
            &String::from("asset0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &asset_1_amount)],
        ),
        (
            &String::from("liquidity0000"),
            &[(&String::from(MOCK_CONTRACT_ADDR), &total_share_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked("asset0000"),
            },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit: Uint128::new(5000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    // We can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());
}
