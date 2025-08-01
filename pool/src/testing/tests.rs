/*use crate::asset::{Asset, AssetInfo, CoinsExt, PairInfo, PairType};
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
    StdError, SubMsgResponse, SubMsgResult, Uint128, Env,
};
use protobuf::Message;
use crate::state::{THRESHOLD_HIT,COMMITSTATUS, POSITIONS, NEXT_POSITION_ID, POOLS, Position, Pool};
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

// Add these imports to your existing test file

// Test helper function for setting up a pool after threshold
fn setup_pool_post_threshold(deps: DepsMut) -> (Env, Addr) {
    // Set threshold as hit so liquidity operations are allowed
    THRESHOLD_HIT.save(deps.storage, &true).unwrap();
    
    // Create a simple pool for testing
    let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(1000000), // 1M native tokens
        reserve1: Uint128::new(2000000), // 2M CW20 tokens
        total_liquidity: Uint128::zero(),
        fee_growth_global_0: Uint128::zero(),
        fee_growth_global_1: Uint128::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOLS.save(deps.storage, 1, &pool).unwrap();
    
    (mock_env(), Addr::unchecked("user"))
}

#[test]
fn test_deposit_liquidity_creates_nft() {
    let mut deps = mock_dependencies(&[
        Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1000000u128),
        }
    ]);

    // Standard setup
    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".to_string() },
            AssetInfo::Token { contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR) },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("creator"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        commit_limit_usd: Uint128::new(5000),
        commit_amount: Uint128::new(5000),
        pool_amount: Uint128::new(5000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        creator_amount: Uint128::new(5000),
        bluechip_amount: Uint128::new(5000),
        token_address: Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100000)],
    };

      let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
    store_liquidity_token(deps.as_mut(), 42, "liquidity0000".to_string());

    // *** FIX: Set up threshold BEFORE trying to deposit ***
    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();
    COMMITSTATUS.save(deps.as_mut().storage, &Uint128::new(5000)).unwrap(); // Match commit_limit_usd
    
    // Create pool
    let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(1000000),
        reserve1: Uint128::new(2000000), 
        total_liquidity: Uint128::zero(),
        fee_growth_global_0: Uint128::zero(),
        fee_growth_global_1: Uint128::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOLS.save(deps.as_mut().storage, 1, &pool).unwrap();

    // Now test deposit liquidity
    let deposit_msg = ExecuteMsg::DepositLiquidity {
        pool_id: 1,
        amount0: Uint128::new(100000),
        amount1: Uint128::new(200000),
    };

    let user = Addr::unchecked("user");
    let info = message_info(
        &user,
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100000),
        }]
    );

    let res = execute(deps.as_mut(), env.clone(), info, deposit_msg).unwrap();

    // Verify response
    assert!(!res.messages.is_empty()); // Should have NFT mint message
    assert_eq!(res.attributes[0], ("action", "deposit_liquidity"));
    assert_eq!(res.attributes[1], ("position_id", "1"));
    assert_eq!(res.attributes[2], ("depositor", user.to_string()));

    // Verify position was created
    let position = POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.pool_id, 1);
    assert_eq!(position.owner, user);
    assert!(!position.liquidity.is_zero());

    // Verify position ID counter incremented
    let next_id = NEXT_POSITION_ID.load(&deps.storage).unwrap();
    assert_eq!(next_id, 1);
}

#[test]
fn test_collect_fees() {
    let mut deps = mock_dependencies(&[]);

    // Setup (same as above test)
    // ... instantiate and setup code ...

    // Create a position first
    let position = Position {
        pool_id: 1,
        liquidity: Decimal::from_ratio(100000u128, 1u128),
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Create pool with some accumulated fees
    let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(1000000),
        reserve1: Uint128::new(2000000),
        total_liquidity: Uint128::new(100000),
        fee_growth_global_0: Uint128::new(1000), // Some fees accumulated
        fee_growth_global_1: Uint128::new(2000),
        total_fees_collected_0: Uint128::new(500),
        total_fees_collected_1: Uint128::new(1000),
    };
    POOLS.save(deps.as_mut().storage, 1, &pool).unwrap();

    // Test collect fees
    let collect_msg = ExecuteMsg::CollectFees {
        position_id: "1".to_string(),
    };

    let info = message_info(&Addr::unchecked("user"), &[]);
    let res = execute(deps.as_mut(), mock_env(), info, collect_msg).unwrap();

    // Verify response
    assert_eq!(res.attributes[0], ("action", "collect_fees"));
    assert_eq!(res.attributes[1], ("position_id", "1"));

    // Should have messages sending fees to user
    assert!(!res.messages.is_empty());

    // Verify position fee tracking was updated
    let updated_position = POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(updated_position.fee_growth_inside_0_last, pool.fee_growth_global_0);
    assert_eq!(updated_position.fee_growth_inside_1_last, pool.fee_growth_global_1);
}

#[test]
fn test_add_to_position() {
    let mut deps = mock_dependencies(&[
        Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(50000u128),
        }
    ]);

    // *** ADD THIS: Complete instantiation setup ***
    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".to_string() },
            AssetInfo::Token { contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR) },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("creator"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        commit_limit_usd: Uint128::new(5000),
        commit_amount: Uint128::new(5000),
        pool_amount: Uint128::new(5000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        creator_amount: Uint128::new(5000),
        bluechip_amount: Uint128::new(5000),
        token_address: Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100000)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
    store_liquidity_token(deps.as_mut(), 42, "liquidity0000".to_string());

    // Set threshold and create pool
    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();
    COMMITSTATUS.save(deps.as_mut().storage, &Uint128::new(5000)).unwrap();
    
    let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(1000000),
        reserve1: Uint128::new(2000000),
        total_liquidity: Uint128::new(100000), // Should have some existing liquidity
        fee_growth_global_0: Uint128::zero(),
        fee_growth_global_1: Uint128::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOLS.save(deps.as_mut().storage, 1, &pool).unwrap();

    // Now create the position
    let initial_liquidity = Decimal::from_ratio(100000u128, 1u128);
    let position = Position {
        pool_id: 1,
        liquidity: initial_liquidity,
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Test adding to position
    let add_msg = ExecuteMsg::AddToPosition {
        position_id: "1".to_string(),
        amount0: Uint128::new(50000), // Add 50k more native
        amount1: Uint128::new(100000), // Add 100k more CW20
    };

    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(50000),
        }]
    );

    let res = execute(deps.as_mut(), mock_env(), info, add_msg).unwrap();

    // Verify response
    assert_eq!(res.attributes[0], ("action", "add_to_position"));
    assert_eq!(res.attributes[1], ("position_id", "1"));

    // Verify position liquidity increased
    let updated_position = POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(updated_position.liquidity > initial_liquidity);
}

#[test]
fn test_remove_partial_liquidity() {
    let mut deps = mock_dependencies(&[]);
let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".to_string() },
            AssetInfo::Token { contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR) },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("creator"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        commit_limit_usd: Uint128::new(5000),
        commit_amount: Uint128::new(5000),
        pool_amount: Uint128::new(5000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        creator_amount: Uint128::new(5000),
        bluechip_amount: Uint128::new(5000),
        token_address: Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100000)],
    };
     let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
    store_liquidity_token(deps.as_mut(), 42, "liquidity0000".to_string());
    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();
    // Setup and create position
    let initial_liquidity = Decimal::from_ratio(100u128, 1u128);
    let position = Position {
        pool_id: 1,
        liquidity: initial_liquidity,
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Create pool
     let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(1000u128),   // Much smaller
        reserve1: Uint128::new(2000u128),   // Much smaller
        total_liquidity: Uint128::from(initial_liquidity.atomics()), // This is huge, so let's match it
        fee_growth_global_0: Uint128::zero(),
        fee_growth_global_1: Uint128::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOLS.save(deps.as_mut().storage, 1, &pool).unwrap();

    // Test partial removal (50%)
    let remove_msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 50,
    };

    let info = message_info(&Addr::unchecked("user"), &[]);
    let res = execute(deps.as_mut(), mock_env(), info, remove_msg).unwrap();

    // Verify response
    assert_eq!(res.attributes[0], ("action", "remove_partial_liquidity"));
    assert_eq!(res.attributes[1], ("position_id", "1"));

    // Should have messages sending tokens to user
    assert!(!res.messages.is_empty());

    // Verify position still exists but with reduced liquidity
    let updated_position = POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(updated_position.liquidity < initial_liquidity);
    assert!(!updated_position.liquidity.is_zero());

    // Position should still exist (NFT not burned)
    assert!(POSITIONS.has(&deps.storage, "1"));
}

#[test]
fn test_remove_full_liquidity_burns_nft() {
    let mut deps = mock_dependencies(&[]);
 let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".to_string() },
            AssetInfo::Token { contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR) },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("creator"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        commit_limit_usd: Uint128::new(5000),
        commit_amount: Uint128::new(5000),
        pool_amount: Uint128::new(5000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        creator_amount: Uint128::new(5000),
        bluechip_amount: Uint128::new(5000),
        token_address: Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100000)],
    };

     let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();
    store_liquidity_token(deps.as_mut(), 42, "liquidity0000".to_string());
    THRESHOLD_HIT.save(deps.as_mut().storage, &true).unwrap();

    // *** FIX: Make total_liquidity match the Decimal's atomics value ***
    let position_liquidity = Decimal::from_ratio(50u128, 1u128);
    let pool = Pool {
        pool_id: 1,
        reserve0: Uint128::new(100000u128), 
        reserve1: Uint128::new(200000u128), 
        total_liquidity: Uint128::from(position_liquidity.atomics()), 
        fee_growth_global_0: Uint128::zero(),
        fee_growth_global_1: Uint128::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOLS.save(deps.as_mut().storage, 1, &pool).unwrap();

    let position = Position {
        pool_id: 1,
        liquidity: position_liquidity,
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Test full removal
    let remove_msg = ExecuteMsg::RemoveLiquidity {
        position_id: "1".to_string(),
    };

    let info = message_info(&Addr::unchecked("user"), &[]);
    let res = execute(deps.as_mut(), env, info, remove_msg).unwrap();

    // Verify response
    assert_eq!(res.attributes[0], ("action", "remove_liquidity"));
    assert_eq!(res.attributes[1], ("position_id", "1"));

    // Should have multiple messages: token transfers + NFT burn
    assert!(res.messages.len() >= 2);

    // Verify position was deleted (NFT burned)
    assert!(!POSITIONS.has(&deps.storage, "1"));
}

#[test]
fn test_unauthorized_access() {
    let mut deps = mock_dependencies(&[]);

    // Create position owned by "user"
    let position = Position {
        pool_id: 1,
        liquidity: Decimal::from_ratio(100000u128, 1u128),
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Try to collect fees as different user
    let collect_msg = ExecuteMsg::CollectFees {
        position_id: "1".to_string(),
    };

    let info = message_info(&Addr::unchecked("attacker"), &[]);
    let err = execute(deps.as_mut(), mock_env(), info, collect_msg).unwrap_err();
    
    assert_eq!(err, ContractError::Unauthorized {});
}

#[test]
fn test_liquidity_deposit_before_threshold() {
    let mut deps = mock_dependencies(&[
        Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100000u128),
        }
    ]);

    // Standard setup but DON'T cross threshold
    let msg = InstantiateMsg {
        factory_addr: Addr::unchecked("factory"),
        asset_infos: [
            AssetInfo::NativeToken { denom: "ubluechip".to_string() },
            AssetInfo::Token { contract_addr: Addr::unchecked(MOCK_CONTRACT_ADDR) },
        ],
        token_code_id: 10u64,
        init_params: None,
        fee_info: FeeInfo {
            bluechip_address: Addr::unchecked("bluechip"),
            creator_address: Addr::unchecked("creator"),
            bluechip_fee: Decimal::from_ratio(10u128, 100u128),
            creator_fee: Decimal::from_ratio(10u128, 100u128),
        },
        commit_limit: Uint128::new(5000),
        commit_limit_usd: Uint128::new(5000),
        commit_amount: Uint128::new(5000),
        pool_amount: Uint128::new(5000),
        oracle_addr: Addr::unchecked("oracle0000"),
        oracle_symbol: "ORCL".to_string(),
        creator_amount: Uint128::new(5000),
        bluechip_amount: Uint128::new(5000),
        token_address: Addr::unchecked("token_address"),
        available_payment: vec![Uint128::new(100000)],
    };

    let env = mock_env();
    let info = message_info(&Addr::unchecked("addr0000"), &[]);
    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Try to deposit liquidity before threshold (should fail)
    let deposit_msg = ExecuteMsg::DepositLiquidity {
        pool_id: 1,
        amount0: Uint128::new(100000),
        amount1: Uint128::new(200000),
    };

    let info = message_info(
        &Addr::unchecked("user"),
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(100000),
        }]
    );

    let err = execute(deps.as_mut(), env, info, deposit_msg).unwrap_err();
    assert_eq!(err, ContractError::ShortOfThreshold {});
}

#[test]
fn test_invalid_partial_removal_amounts() {
    let mut deps = mock_dependencies(&[]);

    // Create position
    let position = Position {
        pool_id: 1,
        liquidity: Decimal::from_ratio(100000u128, 1u128),
        owner: Addr::unchecked("user"),
        fee_growth_inside_0_last: Uint128::zero(),
        fee_growth_inside_1_last: Uint128::zero(),
        created_at: 12345,
        last_fee_collection: 12345,
    };
    POSITIONS.save(deps.as_mut().storage, "1", &position).unwrap();

    // Test 0% removal (should fail)
    let remove_msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 0,
    };

    let info = message_info(&Addr::unchecked("user"), &[]);
    let err = execute(deps.as_mut(), mock_env(), info, remove_msg).unwrap_err();
    assert_eq!(err, ContractError::InvalidAmount {});

    // Test 100% removal (should fail - use full removal instead)
    let remove_msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 100,
    };

    let info = message_info(&Addr::unchecked("user"), &[]);
    let err = execute(deps.as_mut(), mock_env(), info, remove_msg).unwrap_err();
    assert_eq!(err, ContractError::InvalidAmount {});
}
    */