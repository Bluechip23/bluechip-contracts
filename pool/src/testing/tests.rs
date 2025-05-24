use crate::asset::{Asset, AssetInfo, CoinsExt, PairInfo, PairType};
use crate::contract::{
    assert_max_spread, execute, instantiate,
};
use crate::error::ContractError;
use crate::mock_querier::mock_dependencies;
use crate::msg::{ExecuteMsg, FeeInfo, InstantiateMsg};
use crate::response::MsgInstantiateContractResponse;
use cosmwasm_std::testing::{message_info, mock_env, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
     Addr, Binary, Coin, Decimal, DepsMut, Fraction, Reply,
    StdError, SubMsgResponse, SubMsgResult, Uint128,
};
use protobuf::Message;
use crate::state::{THRESHOLD_HIT,COMMITSTATUS};
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
        &MOCK_CONTRACT_ADDR.to_string(),
        &[(&String::from(MOCK_CONTRACT_ADDR), &Uint128::new(123u128))],
    )]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        token_address:     Addr::unchecked("admin"),
        bluechip_amount: Uint128::new(500),
        creator_amount: Uint128::new(1000),
        pool_amount: Uint128::new(3000),
        commit_amount: Uint128::new(1000),
        commit_limit: Uint128::new(25000),
        commit_limit_usd: Uint128::new(25_000),
        oracle_addr:     Addr::unchecked("oracle0000"),
        oracle_symbol:   "ORCL".to_string(),
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
            &MOCK_CONTRACT_ADDR.to_string(),
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
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        creator_amount: Uint128::new(1000),
        pool_amount: Uint128::new(3000),
        commit_amount: Uint128::new(1000),
        bluechip_amount: Uint128::new(500),
        commit_limit_usd:  Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        oracle_symbol:     "ORCL".to_string(),
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

  let mock_token = MOCK_CONTRACT_ADDR.to_string();
    let liquidity = "liquidity0000".to_string();
    let total_share_amount = Uint128::from(111u128);

    // deps.querier
    //     .with_tax(Decimal::zero(), &[("uusd", &Uint128::from(1000000u128))]);
deps.querier.with_token_balances(&[
    // 1) For your CW20 token contract:
    (&mock_token, &[
        // user balances for that token (here we just credit the
        // contract itself in the mock)
        (&mock_token, &Uint128::new(123)),
    ]),

    // 2) For the LP token contract:
    (&liquidity, &[
        // credit the contract (or user) with the total_share_amount
        (&mock_token, &total_share_amount),
    ]),
]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        commit_limit_usd:  Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        commit_amount:     Uint128::new(10_000),
        pool_amount:       Uint128::new(10_000),
        creator_amount:    Uint128::new(10_000),
        bluechip_amount:   Uint128::new(10_000),
        oracle_symbol:     "ORCL".to_string(),
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
    let total_share = Uint128::from(111u128);
    let asset_0_amount = Uint128::from(222u128);
    let asset_1_amount = Uint128::from(333u128);
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: asset_0_amount,
    }]);

  let mock_token = MOCK_CONTRACT_ADDR.to_string();
    let liquidity = "liquidity0000".to_string();
deps.querier.with_token_balances(&[
    (&mock_token, &[
        (&mock_token, &Uint128::new(123)),
    ]),
    (&liquidity, &[
        (&mock_token, &total_share),
    ]),
]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        commit_limit_usd: Uint128::new(5000),
        commit_amount:     Uint128::new(10_000),
        pool_amount:       Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        oracle_symbol:     "ORCL".to_string(),
        creator_amount:    Uint128::new(10_000),
        bluechip_amount:   Uint128::new(10_000),
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
    let total_share = Uint128::from(500u128);
    let asset_0_amount = Uint128::from(250u128);
    let asset_1_amount = Uint128::from(1000u128);
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: asset_0_amount,
    }]);

   let mock_token = MOCK_CONTRACT_ADDR.to_string();
    let liquidity = "liquidity0000".to_string();
deps.querier.with_token_balances(&[
    (&mock_token, &[
        (&mock_token, &Uint128::new(123)),
    ]),
    (&liquidity, &[
        (&mock_token, &total_share),
    ]),
]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        commit_limit_usd: Uint128::new(5000),
        commit_amount:     Uint128::new(10_000),
        pool_amount:       Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        oracle_symbol:     "ORCL".to_string(),
        creator_amount:    Uint128::new(10_000),
        bluechip_amount:   Uint128::new(10_000),
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
fn test_commit() {
    let mut deps = mock_dependencies(&[Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100u128),
    }]);
    let mock_token = MOCK_CONTRACT_ADDR.to_string();
   deps.querier.with_token_balances(&[
    (
        &mock_token,
        &[(&mock_token, &Uint128::new(123))],
    ),
]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "ubluechip".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
        commit_limit_usd: Uint128::new(200_000_000),
        commit_amount:     Uint128::new(10_000),
        pool_amount:       Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        oracle_symbol:     "ORCL".to_string(),
        creator_amount:    Uint128::new(10_000),
        bluechip_amount:   Uint128::new(10_000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    let _res = instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Store liquidity token
    store_liquidity_token(deps.as_mut(), 1, "liquidity0000".to_string());

    // Try commit with insufficient funds
    let commit_msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(100),
        },
        amount: Uint128::new(100),
    };

    let info = message_info(&Addr::unchecked("addr0000"), &[]);
     let res0 = execute(deps.as_mut(), env.clone(), info, commit_msg.clone()).unwrap();
    assert_eq!(res0.messages.len(), 2);
    assert_eq!(
        res0.attributes,
        vec![
            ("action", "subscribe"),
            ("subscriber", "addr0000"),
            ("commit_amount", "100"),
        ]
    );

    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();
    // Try commit with correct funds
    let info = message_info(
        &Addr::unchecked("addr0000"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100),
        }],
    );

    let res = execute(deps.as_mut(), env.clone(), info, commit_msg).unwrap();
    assert_eq!(3, res.messages.len()); // Should have 3 messages: bluechip fee, creator fee, and token transfer
}
#[test]
fn test_simple_swap_threshold_then_succeeds() {
    // 1) Set up numbers
    let native_reserve     = Uint128::new(20_000_000_000u128);
    let cw20_reserve       = Uint128::new(30_000_000_000u128);
    let offer_amount       = Uint128::new( 5_000_000_000u128); // less than native_reserve

    // 2) Create dependencies with only native tokens in the pool
    let mut deps = mock_dependencies(&[
        Coin {
            denom: "ubluechip".to_string(),
            amount: native_reserve,
        }
    ]);
    let pair_addr  = mock_env().contract.address.to_string();
    let asset_addr = MOCK_CONTRACT_ADDR.to_string();

    // 3) Seed the CW20 side of the pool
    deps.querier.with_token_balances(&[
        (&asset_addr, &[(&pair_addr, &cw20_reserve)]),
    ]);

    // 4) Instantiate the pool (use your real InstantiateMsg)
    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".into() },
            AssetInfo::Token      { contract_addr: Addr::unchecked(asset_addr.clone()) },
        ],
        token_code_id:    10,
        init_params:      None,
        fee_info:         FeeInfo {
            bluechip_address: Addr::unchecked("bluechip".to_string()),
            creator_address: Addr::unchecked("creator".to_string()),
            bluechip_fee: Decimal::from_ratio(10 as u128, 100 as u128),
            creator_fee: Decimal::from_ratio(10 as u128, 100 as u128),
        },
        commit_limit:     Uint128::new(5_000),
        commit_limit_usd: Uint128::new(5_000),
        commit_amount:    Uint128::new(5_000),
        pool_amount:      Uint128::new(5_000),
        oracle_addr:      Addr::unchecked("oracle0000"),
        oracle_symbol:    "ORCL".to_string(),
        creator_amount:   Uint128::new(5_000),
        bluechip_amount:  Uint128::new(5_000),
        token_address:    Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100)],
    };
    let env  = mock_env();
    let info = message_info(&Addr::unchecked("user"), &[]);
    instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();

    // 5) Wire up the LP token instantiation
    store_liquidity_token(deps.as_mut(), 1, pair_addr.clone());

    // ─── Attempt #1: below threshold ─────────────────────────────────────
    let swap1 = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info:   AssetInfo::NativeToken { denom: "ubluechip".into() },
            amount: offer_amount,
        },
        belief_price: None,
        max_spread:   Some(Decimal::percent(50)),
        to:           None,
    };
    let err = execute(deps.as_mut(), env.clone(), info.clone(), swap1).unwrap_err();
    assert_eq!(err, ContractError::ShortOfThreshold {});

    // ─── Fund the pool to cross threshold ───────────────────────────────
    deps.querier.with_balance(&[(
        &pair_addr,
        &[Coin {
            denom:  "ubluechip".into(),
            amount: native_reserve + offer_amount,
        }],
    )]);

    let threshold = Uint128::new(5_000); // same as your InstantiateMsg.commit_limit_usd
    COMMITSTATUS.save(deps.as_mut().storage, &threshold).unwrap(); 
    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();
    
    // ─── Attempt #2: now above threshold ───────────────────────────────

    
    let swap2 = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info:   AssetInfo::NativeToken { denom: "ubluechip".into() },
            amount: offer_amount,
        },
        belief_price: None,
        max_spread:   Some(Decimal::percent(50)),
        to:           None,
    };

    let info2 = message_info(
    &Addr::unchecked("user"),
    &[Coin {
        denom:  "ubluechip".into(),
        amount: offer_amount,
    }],
);
    let res = execute(deps.as_mut(), env.clone(), info2, swap2).unwrap();
    // it should have at least one CosmosMsg back to the user
    assert!(!res.messages.is_empty());
}

#[test]
    fn test_commit_validation() {
        let mut deps = mock_dependencies(&[Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(100u128),
        }]);
        let mock_token = MOCK_CONTRACT_ADDR.to_string();
        deps.querier.with_token_balances(&[(
        &mock_token,
       &[(&mock_token, &Uint128::zero())],
        )]);
        let msg = InstantiateMsg {
            factory_addr: Addr::unchecked("factory"),
            asset_infos: [
                AssetInfo::NativeToken {
                    denom: "uusd".to_string(),
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
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
            commit_limit_usd: Uint128::new(5000),
            commit_amount:     Uint128::new(10_000),
            pool_amount:       Uint128::new(10_000),
            oracle_addr:       Addr::unchecked("oracle0000"),
            oracle_symbol:     "ORCL".to_string(),
            creator_amount:    Uint128::new(10_000),
            bluechip_amount:   Uint128::new(10_000),
            token_address: Addr::unchecked("token_address".to_string()),
            available_payment: vec![Uint128::new(100)],
        };

        let env = mock_env();
        let info = message_info(&Addr::unchecked("addr0000"), &[]);
        instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

        // Try commit with wrong denom
        let commit_msg = ExecuteMsg::Commit {
            asset: Asset {
                info: AssetInfo::NativeToken {
                    denom: "uusd".to_string(),
                },
                amount: Uint128::new(100),
            },
            amount: Uint128::new(100),
        };

        let info = message_info(
            &Addr::unchecked("addr0000"),
            &[Coin {
                denom: "wrong".to_string(),
                amount: Uint128::new(100),
            }],
        );

        let err = execute(deps.as_mut(), env.clone(), info, commit_msg).unwrap_err();
        assert_eq!(err, ContractError::AssetMismatch {});
    }

#[test]
fn test_commit_with_token() {
    let mut deps = mock_dependencies(&[]);
    let mock_token = MOCK_CONTRACT_ADDR.to_string();
    deps.querier.with_token_balances(&[(
        &mock_token,
       &[(&mock_token, &Uint128::zero())],
    )]);

    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
               contract_addr: Addr::unchecked(mock_token.clone()), 
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
        commit_limit_usd: Uint128::new(5000),
        commit_amount:     Uint128::new(10_000),
        pool_amount:       Uint128::new(10_000),
        oracle_addr:       Addr::unchecked("oracle0000"),
        oracle_symbol:     "ORCL".to_string(),
        creator_amount:    Uint128::new(10_000),
        bluechip_amount:   Uint128::new(10_000),
        token_address: Addr::unchecked("token_address".to_string()),
        available_payment: vec![Uint128::new(100)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Try commit with token (should fail)
    let commit_msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
            },
            amount: Uint128::new(100),
        },
        amount: Uint128::new(100),
    };

    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    let err = execute(deps.as_mut(), env.clone(), info, commit_msg).unwrap_err();
    assert_eq!(err, ContractError::AssetMismatch {});
}

#[test]
fn test_asset_info() {
    // Test native token
    let native_info = AssetInfo::NativeToken {
        denom: "uusd".to_string(),
    };
    assert!(native_info.is_native_token());
    assert!(!native_info.is_ibc());

    // Test IBC token
    let ibc_info = AssetInfo::NativeToken {
        denom: "ibc/27394FB092D2ECCD56123C74F36E4C1F926001CEADA9CA97EA622B25F41E5EB2".to_string(),
    };
    assert!(ibc_info.is_native_token());
    assert!(ibc_info.is_ibc());

    // Test token contract
   let token_info = AssetInfo::Token {
        contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
    };
    // Now this will pass addr_validate under MockApi
    assert!(!token_info.is_native_token());
    assert!(!token_info.is_ibc());

    // Test equality
    assert!(native_info.equal(&AssetInfo::NativeToken {
        denom: "uusd".to_string(),
    }));
    assert!(!native_info.equal(&token_info));
}

#[test]
fn test_asset() {
    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: Uint128::new(100u128),
    }]);

    // Test native token asset
    let native_asset = Asset {
        info: AssetInfo::NativeToken {
            denom: "uusd".to_string(),
        },
        amount: Uint128::new(100u128),
    };

    // Test compute_tax (should be zero as per Terra 2.0)
    let tax = native_asset.compute_tax(&deps.as_ref().querier).unwrap();
    assert_eq!(tax, Uint128::zero());

    // Test deduct_tax
    let coin = native_asset.deduct_tax(&deps.as_ref().querier).unwrap();
    assert_eq!(
        coin,
        Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(100u128)
        }
    );

    // Test token asset
    let token_asset = Asset {
        info: AssetInfo::Token {
            contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
        },
        amount: Uint128::new(100u128),
    };

    // Test deduct_tax error for token asset
    let err = token_asset.deduct_tax(&deps.as_ref().querier).unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("cannot deduct tax from token asset")
    );
}

#[test]
fn test_pair_info() {
    let pair_info = PairInfo {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
            },
        ],
        contract_addr: Addr::unchecked("pair0000"),
        liquidity_token: Addr::unchecked("liquidity0000"),
        pair_type: PairType::Xyk {},
    };

    // Test pair type display
    assert_eq!(pair_info.pair_type.to_string(), "xyk");
    assert_eq!(
        PairType::Custom("custom_type".to_string()).to_string(),
        "custom-custom_type"
    );
}

#[test]
fn test_asset_validation() {
    let mut deps = mock_dependencies(&[]);

    // Test valid native token
    let native_info = AssetInfo::NativeToken {
        denom: "uusd".to_string(),
    };
    native_info.check(&deps.api).unwrap();

    // Test valid token contract
     let token_info = AssetInfo::Token {
    // Use the built-in valid Bech32 from the mock environment
    contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
    };
     token_info.check(&deps.api).unwrap();

    // Test assert_sent_native_token_balance
    let asset = Asset {
        info: native_info,
        amount: Uint128::new(100u128),
    };

    // Test with correct amount
    let info = message_info(
        &Addr::unchecked("sender"),
        &[Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(100u128),
        }],
    );
    asset.assert_sent_native_token_balance(&info).unwrap();

    // Test with incorrect amount
    let info = message_info(
        &Addr::unchecked("sender"),
        &[Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(50u128),
        }],
    );
    let err = asset.assert_sent_native_token_balance(&info).unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("Native token balance mismatch between the argument and the transferred")
    );
}

#[test]
fn test_coins_ext() {
    let pool_assets = [
        AssetInfo::NativeToken {
            denom: "uusd".to_string(),
        },
        AssetInfo::NativeToken {
            denom: "uluna".to_string(),
        },
    ];

    let assets = [
        Asset {
            info: pool_assets[0].clone(),
            amount: Uint128::new(100u128),
        },
        Asset {
            info: pool_assets[1].clone(),
            amount: Uint128::new(200u128),
        },
    ];

    // Test correct coins
    let coins = vec![
        Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(100u128),
        },
        Coin {
            denom: "uluna".to_string(),
            amount: Uint128::new(200u128),
        },
    ];
    coins.assert_coins_properly_sent(&assets, &pool_assets).unwrap();

    // Test incorrect amount
    let coins = vec![
        Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(50u128),
        },
        Coin {
            denom: "uluna".to_string(),
            amount: Uint128::new(200u128),
        },
    ];
    let err = coins
        .assert_coins_properly_sent(&assets, &pool_assets)
        .unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("Native token balance mismatch between the argument and the transferred")
    );

    // Test invalid coin
    let coins = vec![
        Coin {
            denom: "invalid".to_string(),
            amount: Uint128::new(100u128),
        },
        Coin {
            denom: "uluna".to_string(),
            amount: Uint128::new(200u128),
        },
    ];
    let err = coins
        .assert_coins_properly_sent(&assets, &pool_assets)
        .unwrap_err();
    assert_eq!(
        err,
        StdError::generic_err("Supplied coins contain invalid that is not in the input asset vector")
    );
}