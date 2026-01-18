use crate::asset::{TokenInfo, TokenType};
use crate::contract::{execute, instantiate};
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg, PoolInstantiateMsg};
use crate::state::IS_THRESHOLD_HIT;
use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info},
    Addr, Coin, Decimal, Uint128,
};

#[test]
fn test_standard_pool_instantiation() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let info = mock_info("factory", &[]);

    let msg = PoolInstantiateMsg {
        pool_id: 1,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("token"),
            },
        ],
        cw20_token_contract_id: 1,
        used_factory_addr: Addr::unchecked("factory"),
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip"),
            creator_wallet_address: Addr::unchecked("creator"),
            commit_fee_bluechip: Decimal::percent(1),
            commit_fee_creator: Decimal::percent(5),
        },
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        commit_amount_for_threshold: Uint128::new(100_000_000),
        position_nft_address: Addr::unchecked("nft"),
        token_address: Addr::unchecked("token"),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        is_standard_pool: Some(true),
    };

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    let is_threshold_hit = IS_THRESHOLD_HIT.load(&deps.storage).unwrap();
    assert!(is_threshold_hit);
}

#[test]
fn test_standard_pool_immediate_swap_and_deposit() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let info = mock_info("factory", &[]);

    // Instantiate as standard pool
    let msg = PoolInstantiateMsg {
        pool_id: 1,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("token"),
            },
        ],
        cw20_token_contract_id: 1,
        used_factory_addr: Addr::unchecked("factory"),
        threshold_payout: None,
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("bluechip"),
            creator_wallet_address: Addr::unchecked("creator"),
            commit_fee_bluechip: Decimal::percent(1),
            commit_fee_creator: Decimal::percent(5),
        },
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        commit_amount_for_threshold: Uint128::new(100_000_000),
        position_nft_address: Addr::unchecked("nft"),
        token_address: Addr::unchecked("token"),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
        is_standard_pool: Some(true),
    };

    instantiate(deps.as_mut(), env.clone(), info, msg).unwrap();

    // Try a swap (should NOT return ShortOfThreshold error, but might return InsufficientLiquidity if reserves are 0)
    let swap_info = mock_info(
        "trader",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1000),
        }],
    );
    let swap_msg = ExecuteMsg::SimpleSwap {
        offer_asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "ubluechip".to_string(),
            },
            amount: Uint128::new(1000),
        },
        belief_price: None,
        max_spread: None,
        to: None,
        transaction_deadline: None,
    };

    let err = execute(deps.as_mut(), env.clone(), swap_info, swap_msg).unwrap_err();
    // It should be InsufficientReserves, NOT ShortOfThreshold
    assert_eq!(err, ContractError::InsufficientReserves {});

    // Try a deposit (should NOT return ShortOfThreshold error)
    let deposit_info = mock_info(
        "provider",
        &[Coin {
            denom: "ubluechip".to_string(),
            amount: Uint128::new(1000),
        }],
    );
    let deposit_msg = ExecuteMsg::DepositLiquidity {
        amount0: Uint128::new(1000),
        amount1: Uint128::new(1000),
        min_amount0: None,
        min_amount1: None,
        transaction_deadline: None,
    };

    let res = execute(deps.as_mut(), env, deposit_info, deposit_msg).unwrap();
    // Should be successful
    assert_eq!(
        res.attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap()
            .value,
        "deposit_liquidity"
    );
}
