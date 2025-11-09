use crate::asset::{TokenInfo, TokenType};
use crate::error::ContractError;
use crate::msg::{CommitFeeInfo, ExecuteMsg};
use crate::state::{
    CommitLimitInfo, CreatorExcessLiquidity, TokenMetadata, COMMITFEEINFO, COMMIT_LEDGER,
    COMMIT_LIMIT_INFO, CREATOR_EXCESS_POSITION, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, POOL_STATE,
    USD_RAISED_FROM_COMMIT,
};
use crate::{
    contract::execute,
    testing::liquidity_tests::{setup_pool_post_threshold, setup_pool_storage},
};
use cosmwasm_std::testing::{MockApi, MockQuerier, MockStorage};
use cosmwasm_std::{coin, Binary, Decimal, Empty, OwnedDeps, SystemError, WasmQuery};
use cosmwasm_std::{
    from_json,
    testing::{mock_dependencies, mock_env, mock_info},
    to_json_binary, Addr, ContractResult, CosmosMsg, SystemResult, Uint128, WasmMsg,
};
use cw721_base::ExecuteMsg as CW721BaseExecuteMsg;
use pool_factory_interfaces::ConversionResponse;

pub fn setup_pool_with_excess_config(deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier>) {
    setup_pool_storage(deps);

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(25_000_000_000),
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
        max_bluechip_lock_per_pool: Uint128::new(100_000), 
        creator_excess_liquidity_lock_days: 14,                 
    };

    COMMIT_LIMIT_INFO
        .save(&mut deps.storage, &commit_config)
        .unwrap();

    let fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip"),
        creator_wallet_address: Addr::unchecked("creator"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };
    COMMITFEEINFO.save(&mut deps.storage, &fee_info).unwrap();
}

#[test]
fn test_threshold_with_excess_creates_position() {
    let mut deps = mock_dependencies();

    setup_pool_with_excess_config(&mut deps);

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("user1"),
            &Uint128::new(24_900_000_000),
        )
        .unwrap();

    deps.querier.update_wasm(|query| {
        match query {
            WasmQuery::Smart { .. } => {
                let response = ConversionResponse {
                    amount: Uint128::new(1_000_000_000), 
                    rate_used: Uint128::new(1_000_000_000), 
                    timestamp: 1234567890u64,
                };
                SystemResult::Ok(ContractResult::Ok(to_json_binary(&response).unwrap()))
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query".to_string(),
                request: Binary::default(),
            }),
        }
    });
    let env = mock_env();
    let info = mock_info("final_committer", &[coin(100_000_000_000_000, "stake")]);

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(100_000_000_000_000),
        },
        amount: Uint128::new(100_000_000_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    println!(
        "USD raised after commit: {}",
        USD_RAISED_FROM_COMMIT.load(&deps.storage).unwrap()
    );
    println!(
        "Bluechip reserve: {}",
        POOL_STATE.load(&deps.storage).unwrap().reserve0
    );

    match CREATOR_EXCESS_POSITION.load(&deps.storage) {
        Ok(excess_position) => {
            assert!(excess_position.bluechip_amount > Uint128::zero());

            let fee_info = COMMITFEEINFO.load(&deps.storage).unwrap();
            assert_eq!(excess_position.creator, fee_info.creator_wallet_address);
            assert_eq!(
                excess_position.unlock_time,
                env.block.time.plus_seconds(14 * 86400)
            );
        }
        Err(_) => panic!("Creator excess position should exist"),
    }

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();

    assert!(pool_state.reserve0 > Uint128::new(100_000_000_000)); 
}

#[test]
fn test_claim_excess_before_unlock_fails() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time: env.block.time.plus_seconds(14 * 86400), // 14 days from now
                excess_nft_id: None,
            },
        )
        .unwrap();

    let info = mock_info("creator", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();

    match err {
        ContractError::PositionLocked { unlock_time } => {
            assert_eq!(
                unlock_time.seconds(),
                mock_env().block.time.seconds() + 14 * 86400
            );
        }
        _ => panic!("Expected PositionLocked error"),
    }

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_ok());
}

#[test]
fn test_claim_excess_after_unlock_succeeds() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    let unlock_time = env.block.time.minus_seconds(100); 

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time,
                excess_nft_id: None,
            },
        )
        .unwrap();

    NEXT_POSITION_ID.save(&mut deps.storage, &0u64).unwrap();

    let info = mock_info("creator", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let res = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
    assert_eq!(res.messages.len(), 1);
    match &res.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute { msg, .. }) => {
            let mint_msg: CW721BaseExecuteMsg<TokenMetadata, Empty> = from_json(msg).unwrap();
            match mint_msg {
                CW721BaseExecuteMsg::Mint { owner, .. } => {
                    assert_eq!(owner, "creator");
                }
                _ => panic!("Expected Mint message"),
            }
        }
        _ => panic!("Expected Wasm Execute message"),
    }

    let position = LIQUIDITY_POSITIONS
        .load(&deps.storage, "position_1")
        .unwrap();
    assert_eq!(position.owner, Addr::unchecked("creator"));
    assert!(position.liquidity > Uint128::zero());

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_err());
}

#[test]
fn test_claim_excess_wrong_user_fails() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    CREATOR_EXCESS_POSITION
        .save(
            &mut deps.storage,
            &CreatorExcessLiquidity {
                creator: Addr::unchecked("creator"),
                bluechip_amount: Uint128::new(50_000_000_000),
                token_amount: Uint128::new(175_000_000_000),
                unlock_time: env.block.time.minus_seconds(100), // Already unlocked
                excess_nft_id: None,
            },
        )
        .unwrap();

    let info = mock_info("hacker", &[]);
    let msg = ExecuteMsg::ClaimCreatorExcessLiquidity {};

    let err = execute(deps.as_mut(), env, info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_ok());
}

#[test]
fn test_no_excess_when_under_cap() {
    let mut deps = mock_dependencies();
    setup_pool_with_excess_config(&mut deps);

    let mut commit_config = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
    commit_config.max_bluechip_lock_per_pool = Uint128::new(10_000_000_000_000); // 10M bluechip
    COMMIT_LIMIT_INFO
        .save(&mut deps.storage, &commit_config)
        .unwrap();

    USD_RAISED_FROM_COMMIT
        .save(&mut deps.storage, &Uint128::new(24_900_000_000))
        .unwrap();
    COMMIT_LEDGER
        .save(
            &mut deps.storage,
            &Addr::unchecked("user1"),
            &Uint128::new(24_900_000_000),
        )
        .unwrap();

    deps.querier.update_wasm(move |query| {
        match query {
            WasmQuery::Smart { msg, .. } => {
                let response = ConversionResponse {
                    amount: Uint128::new(1_000_000),
                    rate_used: Uint128::new(1_000_000),
                    timestamp: 1234567890u64,
                };
                SystemResult::Ok(ContractResult::Ok(to_json_binary(&response).unwrap()))
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "Unknown query".to_string(),
                request: Binary::default(),
            }),
        }
    });

    let env = mock_env();
    let info = mock_info("final_committer", &[coin(100_000_000, "stake")]);

    let msg = ExecuteMsg::Commit {
        asset: TokenInfo {
            info: TokenType::Bluechip {
                denom: "stake".to_string(),
            },
            amount: Uint128::new(100_000_000),
        },
        amount: Uint128::new(100_000_000),
        transaction_deadline: None,
        belief_price: None,
        max_spread: None,
    };

    execute(deps.as_mut(), env, info, msg).unwrap();

    let excess = CREATOR_EXCESS_POSITION.load(&deps.storage);
    assert!(excess.is_err());

    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(pool_state.reserve0 < Uint128::new(10_000_000_000_000));
}
