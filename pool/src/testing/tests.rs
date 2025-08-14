
use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR}, 
    to_json_binary, Addr, BankMsg, Binary, Coin, ContractResult, CosmosMsg, Decimal, OwnedDeps, SystemError, 
    SystemResult, Timestamp, Uint128, WasmMsg, WasmQuery
};
use cw20::Cw20ReceiveMsg;
use cw721::OwnerOfResponse;
use crate::{asset::PairType, 
    contract::{execute, execute_add_to_position, execute_collect_fees, execute_deposit_liquidity, execute_remove_liquidity, execute_swap_cw20, instantiate, trigger_threshold_payout}, 
    msg::{Cw20HookMsg, FeeInfo, PoolInstantiateMsg}, 
    oracle::PriceResponse, 
    state::{CommitInfo, OracleInfo, PoolFeeState, PoolInfo, PoolSpecs, PoolState, ThresholdPayout, COMMITSTATUS, 
        COMMIT_CONFIG, FEEINFO, LIQUIDITY_POSITIONS, NATIVE_RAISED, ORACLE_INFO, POOL_INFO, POOL_SPECS, THRESHOLD_PAYOUT
    }};
use crate::msg::ExecuteMsg;
use crate::state::{
    THRESHOLD_HIT, USD_RAISED, COMMIT_LEDGER, REENTRANCY_GUARD,
    SUB_INFO, POOL_STATE, POOL_FEE_STATE, Position, NEXT_POSITION_ID, PairInfo
};
use crate::error::ContractError;
use crate::asset::{Asset, AssetInfo};

// ============= COMMIT TESTS =============



fn mock_dependencies_with_balance(balances: &[Coin]) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    // Give the contract some balance
    deps.querier.update_balance(MOCK_CONTRACT_ADDR, balances.to_vec());
    deps
}
    
    // Removed as_ref: OwnedDeps expects owned types, not references.
fn with_oracle_price(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>, price: i64, publish_time: u64, expo: i32, conf: Uint128 ) {
    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "oracle_contract" {
                    // Return the hardcoded price
                    let response = PriceResponse { price, publish_time, expo, conf };
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&response).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
}
#[test]
fn test_commit_pre_threshold_basic() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let env = mock_env();
    let commit_amount = Uint128::new(1_000_000_000); // 1k bluechip
    
    // Mock oracle response for $1 per bluechip
    with_oracle_price(&mut deps,  100_000_000, 10000000000000, -8, Uint128::new(100_000),);// $1 with 8 decimals

    
    let info = mock_info("user1", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    
    // Verify fees were sent (1% bluechip, 5% creator)
    assert_eq!(res.messages.len(), 2); // Two fee transfers
    
    // Verify commit was recorded in USD
    let user_addr = Addr::unchecked("user1");
    let user_commit_usd = COMMIT_LEDGER.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(user_commit_usd, Uint128::new(1_000_000_000)); // $1k with 6 decimals
    
    // Verify USD raised updated
    let total_usd = USD_RAISED.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(1_000_000_000));
    
    // Verify threshold not hit
    assert_eq!(THRESHOLD_HIT.load(&deps.storage).unwrap(), false);
    
    // Verify subscription created
    let sub = SUB_INFO.load(&deps.storage, &user_addr).unwrap();
    assert_eq!(sub.total_paid_native, commit_amount);
    assert_eq!(sub.total_paid_usd, Uint128::new(1_000_000_000));
}

#[test]
fn test_commit_crosses_threshold() {
      let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(10_000_000_000), // 10k tokens - enough for all transfers
    }]);
    setup_pool_storage(&mut deps);
    
    // Set USD raised to just below threshold
    USD_RAISED.save(&mut deps.storage, &Uint128::new(24_900_000_000)).unwrap(); // $24.9k
    
    let env = mock_env();
    let commit_amount = Uint128::new(200_000_000); // 200 bluechip = $200
    
    // Mock oracle response
        with_oracle_price(&mut deps,  100_000_000, 10000000000000, -8, Uint128::new(100_000),);// $1 with 8 decimals
// $1
    
    let info = mock_info("whale", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        deadline: None
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify threshold was hit
    assert_eq!(THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    
    // Verify multiple messages were sent (fees + threshold payouts)
    // Should have: 2 fee transfers + 4 mints (creator, bluechip, pool, commit rewards)
    assert!(res.messages.len() >= 6);
    
    // Verify pool state was initialized with initial liquidity
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.total_liquidity, Uint128::zero()); // Set to zero initially as per contract
}

#[test]
fn test_commit_post_threshold_swap() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let commit_amount = Uint128::new(100_000_000); // 100 bluechip
    
    // Mock oracle response
    with_oracle_price(&mut deps,  100_000_000, 10000000000000, -8, Uint128::new(100_000),);// $1 with 8 decimals
// $1 with 8 decimals
 // $1
    
    let info = mock_info("subscriber", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify it performed a swap (fees + CW20 transfer)
    assert!(res.messages.len() >= 3); // 2 fees + 1 CW20 transfer
    
    // Verify pool reserves updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Increased from commit
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // Decreased from swap
    
    // Verify fee growth updated
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_0 > Decimal::zero());
    assert!(fee_state.total_fees_collected_0 > Uint128::zero());
}

#[test]
fn test_threshold_payout_integrity_check() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    
    // Corrupt the threshold payout state
    let mut bad_payout = THRESHOLD_PAYOUT.load(&deps.storage).unwrap();
    bad_payout.creator_amount = Uint128::new(999_999_999_999); // Wrong!
    THRESHOLD_PAYOUT.save(&mut deps.storage, &bad_payout).unwrap();
    
    // Try to trigger threshold
    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    let mut pool_fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    let commit_config = COMMIT_CONFIG.load(&deps.storage).unwrap();
    let fee_info = FEEINFO.load(&deps.storage).unwrap();
    
    let result = trigger_threshold_payout(
        &mut deps.storage,
        &pool_info,
        &mut pool_state,
        &mut pool_fee_state,
        &commit_config,
        &bad_payout,
        &fee_info,
        &mock_env(),
    );
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("corruption"));
}

#[test]
fn test_commit_reentrancy_protection() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    
    // Set reentrancy guard
    REENTRANCY_GUARD.save(&mut deps.storage, &true).unwrap();
    
    let env = mock_env();
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        deadline: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::ReentrancyGuard {} => (),
        _ => panic!("Expected ReentrancyGuard error"),
    }
}

#[test]
fn test_commit_rate_limiting() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let mut env = mock_env();
    let user = Addr::unchecked("user");
    
    // First commit succeeds
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    with_oracle_price(&mut deps,  100_000_000, 10000000000000, -8, Uint128::new(100_000),);// $1 with 8 decimals


    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        deadline: None,
    };
    
    execute(deps.as_mut(), env.clone(), info.clone(), msg.clone()).unwrap();
    
    // Second commit too soon should fail
    env.block.time = env.block.time.plus_seconds(30); // Only 30 seconds later
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::TooFrequentCommits { wait_time } => {
            assert_eq!(wait_time, 30); // Should wait 30 more seconds (60 total - 30 elapsed)
        },
        _ => panic!("Expected TooFrequentCommits error"),
    }
}

#[test]
fn test_commit_with_deadline() {
     let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_storage(&mut deps);
    
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_000_000);
    
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    // Set deadline in the past
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        amount: Uint128::new(1_000_000),
        deadline: Some(Timestamp::from_seconds(999_999))
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::TransactionExpired {} => (),
        _ => panic!("Expected DeadlineExceeded error"),
    }
}

// ============= SWAP TESTS =============

#[test]
fn test_simple_swap_native_to_cw20() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000_000), // Give contract 1000 tokens
    }]);
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let swap_amount = Uint128::new(100_000_000); // 1k bluechip
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: swap_amount,
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string()},
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: None,
        to: None,
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify swap executed
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "swap");
    
    // Verify reserves updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 > Uint128::new(23_500_000_000)); // Native increased
    assert!(pool_state.reserve1 < Uint128::new(350_000_000_000)); // CW20 decreased
    
    // Verify fee growth
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fee_state.fee_growth_global_0 > Decimal::zero());
}

#[test]
fn test_swap_with_max_spread() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k bluechip (large swap)
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: swap_amount,
    }]);
    
    // Set very tight max spread (0.1%)
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: swap_amount,
        },
        belief_price: None,
        max_spread: Some(Decimal::permille(1)), // 0.1%
        to: None,
        deadline: None,
    };
    
    // Large swap should exceed max spread
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::MaxSpreadAssertion {} => (),
        _ => panic!("Expected MaxSpreadAssertion error"),
    }
}

#[test]
fn test_commit_threshold_overshoot_split() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000), // Plenty for all operations
    }]);
    
    setup_pool_storage(&mut deps);
    
    // Set USD raised to just below threshold
    USD_RAISED.save(&mut deps.storage, &Uint128::new(24_999_000_000)).unwrap(); // $24,999
    
    let env = mock_env();
    
    // Mock oracle at $1 per token
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "oracle_contract" {
                    let response = PriceResponse {
                       price: 100_000_000,// $1 with 8 decimals
                       conf: Uint128::new(100_000),      
                       expo: -8, //so we can check the right amount of decimals.
                       publish_time: 1000000000000, //this value is to small so it was failing. 
                    };
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&response).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    // Commit $5 worth (5 tokens at $1 each)
    let commit_amount = Uint128::new(5_000_000); // 5 tokens with 6 decimals
    
    let info = mock_info("whale", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();
        println!("\n=== Response Attributes ===");
    for attr in &res.attributes {
        println!("{}: {}", attr.key, attr.value);
    }

       println!("\n=== All Messages ({} total) ===", res.messages.len());
    for (i, submsg) in res.messages.iter().enumerate() {
        match &submsg.msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
                println!("Message {}: Bank Send to {} amount {:?}", i, to_address, amount);
            }
            CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
                println!("Message {}: Wasm Execute to {} with msg: {}", i, contract_addr, 
                    String::from_utf8_lossy(msg.as_slice()));
            }
            _ => println!("Message {}: Other type", i),
        }
    }

     let has_transfer = res.messages.iter().any(|submsg| {
        if let CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) = &submsg.msg {
            let msg_str = String::from_utf8_lossy(msg.as_slice());
            msg_str.contains("transfer")
        } else {
            false
        }
    });
   let binding = "0".to_string();
   let return_amt_str = res.attributes.iter()
        .find(|a| a.key == "native_excess_returned")
        .map(|a| &a.value)
        .unwrap_or(&binding);
    println!("Return amount from attributes: {}", return_amt_str);
    // Verify threshold was hit
    assert_eq!(THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
       let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    println!("\n=== Pool State After ===");
    println!("reserve0: {}", pool_state.reserve0);
    println!("reserve1: {}", pool_state.reserve1);
    // Verify USD raised is exactly at threshold
    assert_eq!(USD_RAISED.load(&deps.storage).unwrap(), Uint128::new(25_000_000_000));
    
    // Verify commit ledger was cleared (trigger_threshold_payout clears it)
    assert!(COMMIT_LEDGER.load(&deps.storage, &info.sender).is_err());
    
    // Check attributes to verify split happened
    let attrs = &res.attributes;
    assert_eq!(attrs.iter().find(|a| a.key == "phase").unwrap().value, "threshold_crossing");
    assert_eq!(attrs.iter().find(|a| a.key == "threshold_amount_usd").unwrap().value, "1000000");
    assert_eq!(attrs.iter().find(|a| a.key == "swap_amount_usd").unwrap().value, "4000000");
    let native_excess = attrs.iter().find(|a| a.key == "swap_amount_native").unwrap().value.clone();
    let return_amt = attrs.iter().find(|a| a.key == "native_excess_returned").unwrap().value.clone();
    
    println!("\n=== Swap Details ===");
    println!("Native excess to swap: {}", native_excess);
    println!("CW20 returned: {}", return_amt);
    // Verify CW20 tokens were sent (from the swap portion)
    // Verify subscription recorded full amount
    let sub = SUB_INFO.load(&deps.storage, &info.sender).unwrap();
    assert_eq!(sub.total_paid_native, commit_amount); // Full 5 tokens
    assert_eq!(sub.total_paid_usd, Uint128::new(5_000_000)); // Full $5
    
    // Verify that threshold payout messages were created
    // Should have mints for: creator, bluechip, pool, and commit rewards
  
        if has_transfer {
        println!("SUCCESS: CW20 transfer found!");
    } else {
        println!("ISSUE: No CW20 transfer found despite return_amt = {}", return_amt_str);
    }
}


#[test]
fn test_commit_exact_threshold() {
    let mut deps = mock_dependencies_with_balance(&[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(100_000_000_000),
    }]);
    
    setup_pool_storage(&mut deps);
    
    // Set USD raised to need exactly $1 more
    USD_RAISED.save(&mut deps.storage, &Uint128::new(24_999_000_000)).unwrap();
    
    // add previous commits to simulate the 24,999
    let previous_user = Addr::unchecked("previous_user");
    COMMIT_LEDGER.save(&mut deps.storage, &previous_user, &Uint128::new(24_999_000_000)).unwrap();
    
    let env = mock_env();
    
    // Mock oracle
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "oracle_contract" {
                    let response = PriceResponse {
                        price: 100_000_000,
                        conf: Uint128::new(100_000),      
                        expo: -8,        
                        publish_time: 1000000000000,
                    };
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&response).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    // Commit exactly $1
    let commit_amount = Uint128::new(1_000_000); // 1 token = $1
    
    let info = mock_info("user", &[Coin {
        denom: "stake".to_string(),
        amount: commit_amount,
    }]);
    
    let msg = ExecuteMsg::Commit {
        asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: commit_amount,
        },
        amount: commit_amount,
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env, info.clone(), msg).unwrap();
    
    // Should be a normal funding phase commit that triggers threshold
    assert_eq!(res.attributes.iter().find(|a| a.key == "phase").unwrap().value, "funding");
    
    // Verify threshold hit
    assert_eq!(THRESHOLD_HIT.load(&deps.storage).unwrap(), true);
    // verify that the total USD raised is at the threshold
    let total_usd = USD_RAISED.load(&deps.storage).unwrap();
    assert_eq!(total_usd, Uint128::new(25_000_000_000)); // Should be exactly at $25k threshold
}
#[test]
fn test_swap_cw20_via_hook() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Mock CW20 balance query for the pool contract
    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "token_contract" {
                    // Parse the query to check if it's a balance query
                    if msg.to_string().contains("balance") {
                        let balance_response = cw20::BalanceResponse {
                            balance: Uint128::new(350_000_000_000), // Pool has 350k tokens
                        };
                        SystemResult::Ok(ContractResult::Ok(
                            to_json_binary(&balance_response).unwrap()
                        ))
                    } else {
                        SystemResult::Err(SystemError::InvalidRequest {
                            error: "Unknown query".to_string(),
                            request: msg.clone(),
                        })
                    }
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let swap_amount = Uint128::new(10_000_000_000); // 10k tokens
    
    // Message from CW20 token contract
    let info = mock_info("token_contract", &[]);
    
    let cw20_msg = Cw20ReceiveMsg {
        sender: "trader".to_string(),
        amount: swap_amount,
        msg: to_json_binary(&Cw20HookMsg::Swap {
            belief_price: None,
            max_spread: Some(Decimal::percent(10)), // Allow spread
            to: None,
        }).unwrap(),
    };
    
    let res = execute_swap_cw20(deps.as_mut(), env, info, cw20_msg).unwrap();
    
    // Verify swap executed
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "swap");
    
    // Verify reserves updated (opposite direction from native swap)
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(23_500_000_000)); // Native decreased
    assert!(pool_state.reserve1 > Uint128::new(350_000_000_000)); // CW20 increased
}

#[test]
fn test_swap_wrong_asset() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let info = mock_info("trader", &[Coin {
        denom: "wrong_token".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken { denom: "wrong_token".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        deadline: None,
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::AssetMismatch {} => (),
        _ => panic!("Expected AssetMismatch error"),
    }
}

#[test]
fn test_swap_price_accumulator_update() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let mut env = mock_env();
    env.block.time = Timestamp::from_seconds(1_600_001_000); // 1000 seconds later
    
    let initial_state = POOL_STATE.load(&deps.storage).unwrap();
    let initial_price0 = initial_state.price0_cumulative_last;
    
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        deadline: None,
    };
    
    execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    
    // Verify price accumulator updated
    let updated_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(updated_state.price0_cumulative_last > initial_price0);
    assert_eq!(updated_state.block_time_last, env.block.time.seconds());
}

#[test]
fn test_deposit_liquidity_first_position() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let native_amount = Uint128::new(1_000_000_000); // 1k bluechip
    let token_amount = Uint128::new(14_893_617_021); // Approximately correct ratio
    
    // User sends native tokens with the message
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: native_amount,
    }]);
    
    // Call execute_deposit_liquidity directly
    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user.clone(),
        native_amount,
        token_amount,
        None, // min_amount0
        None, // min_amount1
        None, // deadline
    ).unwrap();
    
    // Verify NFT mint message sent
    assert!(res.messages.iter().any(|msg| {
        matches!(&msg.msg, CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, .. }) 
            if contract_addr == "nft_contract")
    }));
    
    // Verify position created
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "2").unwrap(); // ID starts at 1, increments to 2
    assert_eq!(position.owner, user);
    assert!(position.liquidity > Uint128::zero());
    
    // Verify pool state updated
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.total_liquidity > Uint128::new(91_104_335_791)); // Initial + new
    
    // Verify next position ID incremented
    assert_eq!(NEXT_POSITION_ID.load(&deps.storage).unwrap(), 2);
}

#[test]
fn test_deposit_liquidity_with_slippage() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let native_amount = Uint128::new(1_000_000_000);
    let token_amount = Uint128::new(10_000_000_000); // Incorrect ratio
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: native_amount,
    }]);
    
    // Set minimum amounts for slippage protection
    let err = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user,
        native_amount,
        token_amount,
        Some(Uint128::new(950_000_000)), // min_amount0 - Expect at least 95% of native
        Some(Uint128::new(14_000_000_000)), // min_amount1 - Expect significant token amount
        None,
    ).unwrap_err();
    
    match err {
        ContractError::SlippageExceeded { .. } => (),
        _ => panic!("Expected SlippageExceeded error"),
    }
}

#[test]
fn test_add_to_existing_position() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create initial position
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Mock NFT ownership check - the user owns position NFT #1
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    // Mock ownership query response
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let native_amount = Uint128::new(500_000_000); // 500 bluechip
    let token_amount = Uint128::new(7_500_000_000); // Approximately correct ratio
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: native_amount,
    }]);
    
    let res = execute_add_to_position(
        deps.as_mut(),
        env,
        info,
        user,
        "1".to_string(), // position_id
        native_amount,
        token_amount,
        None, // min_amount0
        None, // min_amount1
        None, // deadline
    ).unwrap();
    
    // Verify position liquidity increased
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(position.liquidity > Uint128::new(1_000_000));
    
    // Verify action
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "add_to_position");
}

#[test]
fn test_add_to_position_not_owner() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position owned by someone else
    create_test_position(&mut deps, 1, "other_user", Uint128::new(1_000_000));
    
    // Mock NFT ownership check - different owner
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "other_user".to_string(), // Different owner
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let err = execute_add_to_position(
        deps.as_mut(),
        env,
        info,
        user,
        "1".to_string(),
        Uint128::new(1_000_000),
        Uint128::new(15_000_000),
        None,
        None,
        None,
    ).unwrap_err();
    
    match err {
        ContractError::Unauthorized {} => (),
        _ => panic!("Expected Unauthorized error"),
    }
}

#[test]
fn test_collect_fees_with_accrued_fees() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position with significant liquidity
    create_test_position(&mut deps, 1, "fee_collector", Uint128::new(10_000_000));
    
    // Mock NFT ownership
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "fee_collector".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    // Simulate fee accrual
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::percent(1); // 1% fees
    fee_state.fee_growth_global_1 = Decimal::percent(2); // 2% fees
    fee_state.total_fees_collected_0 = Uint128::new(100_000);
    fee_state.total_fees_collected_1 = Uint128::new(200_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();
    
    let env = mock_env();
    let info = mock_info("fee_collector", &[]);
    
    let res = execute_collect_fees(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
    ).unwrap();
    
    // Verify fee collection messages (native and CW20)
    assert!(res.messages.len() >= 1); // At least one fee transfer
    
    // Verify position fee growth updated
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.fee_growth_inside_0_last, fee_state.fee_growth_global_0);
    assert_eq!(position.fee_growth_inside_1_last, fee_state.fee_growth_global_1);
}
#[test]
fn test_remove_all_liquidity() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Store initial liquidity for comparison
    let initial_liquidity = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    
    // Create position (this will increase total liquidity)
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Verify liquidity increased
    let after_add = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert!(after_add > initial_liquidity);
    
    // Mock NFT ownership
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let res = execute_remove_liquidity(
        deps.as_mut(),
        env,
        info,
        "1".to_string(),
        None, // deadline
        None, // min_amount0
        None, // min_amount1
    ).unwrap();
    
    // Verify assets returned (native + CW20 transfers)
    assert!(res.messages.len() >= 2);
    
    // Verify position removed
    assert!(LIQUIDITY_POSITIONS.load(&deps.storage, "1").is_err());
    
    // Verify pool liquidity decreased back to initial amount
    let final_liquidity = POOL_STATE.load(&deps.storage).unwrap().total_liquidity;
    assert_eq!(final_liquidity, initial_liquidity);
}

#[test]
fn test_deposit_liquidity_imbalanced_amounts() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    let env = mock_env();
    let user = Addr::unchecked("liquidity_provider");
    // Provide way too much native token
    let native_amount = Uint128::new(10_000_000_000); // 10k bluechip
    let token_amount = Uint128::new(1_000_000_000); // Only 1k tokens (should need ~149k)
    
    let info = mock_info(user.as_str(), &[Coin {
        denom: "stake".to_string(),
        amount: native_amount,
    }]);
    
    let res = execute_deposit_liquidity(
        deps.as_mut(),
        env,
        info,
        user,
        native_amount,
        token_amount,
        None,
        None,
        None,
    ).unwrap();
    
    // Should have refund message for excess native tokens
    let refund_msg = res.messages.iter().find(|msg| {
        matches!(&msg.msg, CosmosMsg::Bank(BankMsg::Send { .. }))
    });
    assert!(refund_msg.is_some());
    
    // Check refund amount in attributes
    let refund_attr = res.attributes.iter().find(|a| a.key == "refunded_amount0").unwrap();
    assert!(Uint128::new(refund_attr.value.parse::<u128>().unwrap()) > Uint128::zero());
}

#[test]
fn test_remove_liquidity_with_slippage_protection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    // Manipulate pool to cause slippage
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.reserve0 = Uint128::new(20_000_000_000); // Reduce reserves
    pool_state.reserve1 = Uint128::new(300_000_000_000);
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    let msg = ExecuteMsg::RemoveLiquidity {
        position_id: "1".to_string(),
        deadline: None,
        min_amount0: Some(Uint128::new(1_000_000_000)), // Expect high amount
        min_amount1: Some(Uint128::new(15_000_000_000)),

    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::SlippageExceeded { .. } => (),
        _ => panic!("Expected SlippageExceeded error"),
    }
}


#[test]
fn test_remove_partial_liquidity_amount() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));

     deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    // Mock ownership query response
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    // Create position with 1M liquidity
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Remove 300k liquidity
    let msg = ExecuteMsg::RemovePartialLiquidity {
        position_id: "1".to_string(),
        liquidity_to_remove: Uint128::new(300_000),
        deadline: None,
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify partial removal
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.liquidity, Uint128::new(700_000)); // 1M - 300k
    
    // Verify proportional fee collection
    assert!(res.messages.len() >= 2); // Asset returns
}

#[test]
fn test_remove_partial_liquidity_by_percent() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position with 1M liquidity
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    
    // Store initial pool state
    let initial_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    
    // Mock NFT ownership
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Remove 25% of liquidity
    let msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 25, // 25%
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Verify the action
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "remove_partial_liquidity");
    
    // Verify 25% was removed from position
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(position.liquidity, Uint128::new(750_000)); // 75% remaining
    
    // Verify liquidity removed attribute
    assert_eq!(
        res.attributes.iter().find(|a| a.key == "liquidity_removed").unwrap().value, 
        "250000" // 25% of 1M
    );
    
    // Verify pool total liquidity decreased by 25% of position
    let final_pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        final_pool_state.total_liquidity,
        initial_pool_state.total_liquidity - Uint128::new(250_000)
    );
    
    // Verify transfer messages were created (native + CW20)
    assert!(res.messages.len() >= 2);
}
// ============= EDGE CASE TESTS =============

#[test]
fn test_zero_liquidity_fee_collection() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Set pool to zero liquidity
    let mut pool_state = POOL_STATE.load(&deps.storage).unwrap();
    pool_state.total_liquidity = Uint128::zero();
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
    
    // Try to update fee growth (should not panic)
    let env = mock_env();
    let info = mock_info("trader", &[Coin {
        denom: "stake".to_string(),
        amount: Uint128::new(1_000_000),
    }]);
    
    let msg = ExecuteMsg::SimpleSwap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken { denom: "stake".to_string() },
            amount: Uint128::new(1_000_000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        deadline: None,
    };
    
    // Should execute without updating fee growth
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Fee growth should remain zero
    let fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(fee_state.fee_growth_global_0, Decimal::zero());
}

#[test]
fn test_price_accumulator_zero_reserves() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps); // Pre-threshold, zero reserves
    
    // Mark as post-threshold but keep zero reserves
    THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(1000);
    
    // This should not panic with zero reserves
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::zero());
    assert_eq!(pool_state.reserve1, Uint128::zero());
    
    // Price accumulator should not update with zero reserves
    assert_eq!(pool_state.price0_cumulative_last, Uint128::zero());
    assert_eq!(pool_state.price1_cumulative_last, Uint128::zero());
}

#[test]
fn test_collect_fees_no_fees_accrued() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    // Create position but don't accrue any fees
    create_test_position(&mut deps, 1, "fee_collector", Uint128::new(1_000_000));
    
    
        deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "fee_collector".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    let env = mock_env();
    let info = mock_info("fee_collector", &[]);
    let msg = ExecuteMsg::CollectFees {
        position_id: "1".to_string(),
    };
    
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    
    // Should succeed but no transfer messages
    assert_eq!(res.messages.len(), 0);
    assert_eq!(res.attributes.iter().find(|a| a.key == "action").unwrap().value, "collect_fees");
}

#[test]
fn test_invalid_percentage_removal() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    
    create_test_position(&mut deps, 1, "liquidity_provider", Uint128::new(1_000_000));
    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { contract_addr, msg } => {
                if contract_addr == "nft_contract" {
                    SystemResult::Ok(ContractResult::Ok(
                        to_json_binary(&cw721::OwnerOfResponse {
                            owner: "liquidity_provider".to_string(),
                            approvals: vec![],
                        }).unwrap()
                    ))
                } else {
                    SystemResult::Err(SystemError::InvalidRequest {
                        error: "Unknown contract".to_string(),
                        request: msg.clone(),
                    })
                }
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query type".to_string(),
                request: Binary::default(),
            }),
        }
    });
    let env = mock_env();
    let info = mock_info("liquidity_provider", &[]);
    
    // Try to remove more than 100%
    let msg = ExecuteMsg::RemovePartialLiquidityByPercent {
        position_id: "1".to_string(),
        percentage: 0, // Invalid
    };
    
    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match err {
        ContractError::InvalidPercent {} => (),
        _ => panic!("Expected InvalidPercent error"),
    }
}

/// Sets up a pool in pre-threshold state with all necessary configuration
pub fn setup_pool_storage(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    // Set up PoolInfo
    let pool_info = PoolInfo {
        pool_id: 1u64,
        pair_info: PairInfo {
            asset_infos: [
                AssetInfo::NativeToken {
                    denom: "stake".to_string(), // Using "stake" as native token
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked("token_contract"),
                },
            ],
            contract_addr: Addr::unchecked("pool_contract"),
            liquidity_token: Addr::unchecked("lp_token"), // Not used with NFTs but kept for compatibility
            pair_type: PairType::Xyk {},
        },
        factory_addr: Addr::unchecked("factory_contract"),
        token_address: Addr::unchecked("token_contract"),
        position_nft_address: Addr::unchecked("nft_contract"),
    };
    POOL_INFO.save(&mut deps.storage, &pool_info).unwrap();

    // Set up PoolState - Pre-threshold (no liquidity yet)
    let pool_state = PoolState {
        nft_ownership_accepted: true,
        reserve0: Uint128::zero(), // No reserves pre-threshold
        reserve1: Uint128::zero(),
        total_liquidity: Uint128::zero(),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    // Set up PoolFeeState
    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };
    POOL_FEE_STATE.save(&mut deps.storage, &pool_fee_state).unwrap();

    // Set up PoolSpecs
    let pool_specs = PoolSpecs {
        subscription_period: 86400, // 1 day in seconds
        lp_fee: Decimal::percent(3) / Uint128::new(10), // 0.3% fee (3/1000)
        min_commit_interval: 60, // 1 minute minimum between commits
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    // Set up CommitInfo
    let commit_config = CommitInfo {
        commit_limit: Uint128::new(100_000_000), // 100 bluechip tokens
        commit_limit_usd: Uint128::new(25_000_000_000), // $25k with 6 decimals
        available_payment: vec![
            Uint128::new(1_000_000),   // 1 token
            Uint128::new(5_000_000),   // 5 tokens
            Uint128::new(10_000_000),  // 10 tokens
        ],
        available_payment_usd: vec![
            Uint128::new(1_000_000_000),   // $1k
            Uint128::new(5_000_000_000),   // $5k
            Uint128::new(10_000_000_000),  // $10k
        ],
    };
    COMMIT_CONFIG.save(&mut deps.storage, &commit_config).unwrap();

    // Set up ThresholdPayout
    let threshold_payout = ThresholdPayout {
        creator_amount: Uint128::new(325_000_000_000), // 325k tokens
        bluechip_amount: Uint128::new(25_000_000_000), // 25k tokens
        pool_amount: Uint128::new(350_000_000_000),     // 350k tokens
        commit_amount: Uint128::new(500_000_000_000),   // 500k tokens
    };
    THRESHOLD_PAYOUT.save(&mut deps.storage, &threshold_payout).unwrap();

    // Set up FeeInfo
    let fee_info = FeeInfo {
        bluechip_address: Addr::unchecked("bluechip_treasury"),
        creator_address: Addr::unchecked("creator_wallet"),
        bluechip_fee: Decimal::percent(1), // 1%
        creator_fee: Decimal::percent(5),   // 5%
    };
    FEEINFO.save(&mut deps.storage, &fee_info).unwrap();

    // Set up OracleInfo
    let oracle_info = OracleInfo {
        oracle_addr: Addr::unchecked("oracle_contract"),
        oracle_symbol: "BLUECHIP".to_string(),
    };
    ORACLE_INFO.save(&mut deps.storage, &oracle_info).unwrap();

    // Initialize other state variables
    THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();
    USD_RAISED.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NATIVE_RAISED.save(&mut deps.storage, &Uint128::zero()).unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();
}

/// Sets up a pool in post-threshold state with initial liquidity
pub fn setup_pool_post_threshold(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    // First set up basic pool
    setup_pool_storage(deps);
    COMMITSTATUS.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap();
    // Mark threshold as hit
    THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    USD_RAISED.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap(); // $25k reached
    
    // Update pool state with initial liquidity
    // Initial liquidity: 23.5k bluechip (25k - fees) and 350k creator tokens
    let pool_state = PoolState {
        nft_ownership_accepted: true,
        reserve0: Uint128::new(23_500_000_000), // 23.5k bluechip (25k - 6% fees)
        reserve1: Uint128::new(350_000_000_000), // 350k creator tokens
        total_liquidity: Uint128::new(91_104_335_791), // sqrt(23.5k * 350k) ≈ 91k
        block_time_last: 1_600_000_000,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();
}

#[test]
fn test_factory_impersonation_prevented() {
    let mut deps = mock_dependencies();
    
    // Try to instantiate from non-factory address
      let msg = PoolInstantiateMsg {
        pool_id: 1u64,
        asset_infos: [
                AssetInfo::NativeToken {
                    denom: "bluechip".to_string(),
                },
                AssetInfo::Token {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
        token_code_id: 2u64,
        factory_addr: Addr::unchecked("factory_contract"),
        init_params: None,
        fee_info: FeeInfo {
                bluechip_address: Addr::unchecked("bluechip"),
                creator_address: Addr::unchecked("addr0000"),
                bluechip_fee: Decimal::from_ratio(10u128, 100u128),
                creator_fee: Decimal::from_ratio(10u128, 100u128),
            },
        commit_limit: Uint128::new(350_000_000_000),
        commit_limit_usd: Uint128::new(350_000_000_000),
        position_nft_address: Addr::unchecked("NFT_contract"),
        oracle_addr: Addr::unchecked("oracle_contract"),
        oracle_symbol: "BLUECHIP".to_string(),
        token_address: Addr::unchecked("token_contract"),
        available_payment: vec![Uint128::new(1_000_000)],
        available_payment_usd: vec![Uint128::new(1_000_000)],
    };
    let info = mock_info("fake_factory", &[]); // Wrong sender!
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();
    
    match err {
        ContractError::Unauthorized {} => (),
        _ => panic!("Expected Unauthorized error"),
    }
}


/// Creates a test liquidity position with specified parameters
pub fn create_test_position(
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>,
    position_id: u64,
    owner: &str,
    liquidity: Uint128,
) {
    let position = Position {
        liquidity,
        owner: Addr::unchecked(owner),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: 1_600_000_000,
        last_fee_collection: 1_600_000_000,
    };
    
    LIQUIDITY_POSITIONS.save(&mut deps.storage, &position_id.to_string(), &position).unwrap();
    
    // Also update the pool's total liquidity
    POOL_STATE.update(&mut deps.storage, |mut state| -> Result<_, cosmwasm_std::StdError> {
        state.total_liquidity += liquidity;
        Ok(state)
    }).unwrap();
}
