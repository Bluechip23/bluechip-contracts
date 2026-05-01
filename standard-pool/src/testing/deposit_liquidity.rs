//! Pool-core `execute_deposit_liquidity` / `execute_add_to_position`
//! coverage via standard-pool's execute dispatch.

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::Env;

/// Advances `mock_env()` past the per-user rate-limit window
/// (`min_commit_interval = 13s` at instantiate). Used by tests that
/// perform a second user-rate-limited action right after a deposit so
/// they don't trip `TooFrequentCommits` in the same mock block.
fn env_after_rate_limit() -> Env {
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(60);
    env
}
use cosmwasm_std::{Addr, Coin, CosmosMsg, Uint128, WasmMsg};
use pool_core::state::{LIQUIDITY_POSITIONS, NEXT_POSITION_ID, POOL_STATE};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

// -- First deposit -------------------------------------------------------

#[test]
fn first_deposit_mints_position_and_emits_nft_accept() {
    let (mut deps, addrs) = instantiate_default_pool();

    // First deposit: any non-zero ratio is accepted (no existing reserves).
    let user = addrs.pool_owner.clone();
    let funds = vec![Coin::new(1_000_000u128, BLUECHIP_DENOM)];
    let info = message_info(&user, &funds);

    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Position 1 exists (NEXT_POSITION_ID started at 0, first deposit
    // increments to 1 before save).
    let position = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(!position.liquidity.is_zero());
    assert_eq!(position.owner, user);
    assert_eq!(NEXT_POSITION_ID.load(&deps.storage).unwrap(), 1);

    // Pool state updated.
    let pool_state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(pool_state.reserve0, Uint128::new(1_000_000));
    assert_eq!(pool_state.reserve1, Uint128::new(2_000_000));
    assert!(pool_state.nft_ownership_accepted, "flag should flip on first deposit");

    // Response carries the AcceptOwnership WasmMsg directed at the
    // position-NFT contract — first deposit accepts ownership.
    let nft_contract = addrs.position_nft.to_string();
    let nft_accept_msg = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == &nft_contract
                && String::from_utf8_lossy(msg.as_slice()).contains("update_ownership")
        }
        _ => false,
    });
    assert!(
        nft_accept_msg,
        "first deposit must emit UpdateOwnership(AcceptOwnership) to {}",
        nft_contract
    );

    // Response also carries the Cw20 TransferFrom for the CreatorToken side.
    let cw20_transfer = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == addrs.creator_token.as_str()
                && String::from_utf8_lossy(msg.as_slice()).contains("transfer_from")
        }
        _ => false,
    });
    assert!(
        cw20_transfer,
        "deposit must emit Cw20 TransferFrom for the CreatorToken side"
    );
}

#[test]
fn second_deposit_does_not_reemit_accept_ownership() {
    let (mut deps, addrs) = instantiate_default_pool();

    // First deposit: flips the flag.
    let user = addrs.pool_owner.clone();
    let funds1 = vec![Coin::new(1_000_000u128, BLUECHIP_DENOM)];
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&user, &funds1),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Second deposit at the existing 1:2 ratio.
    let funds2 = vec![Coin::new(500_000u128, BLUECHIP_DENOM)];
    let res = execute(
        deps.as_mut(),
        env_after_rate_limit(),
        message_info(&user, &funds2),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(500_000),
            amount1: Uint128::new(1_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    let nft_contract = addrs.position_nft.to_string();
    let accept_seen = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == &nft_contract
                && String::from_utf8_lossy(msg.as_slice()).contains("update_ownership")
        }
        _ => false,
    });
    assert!(
        !accept_seen,
        "second deposit must NOT re-emit AcceptOwnership"
    );
}

#[test]
fn deposit_refunds_overpaid_native() {
    let (mut deps, addrs) = instantiate_default_pool();

    // Send more native than the deposit amount requests — collect_deposit_side
    // should emit a BankMsg::Send refund.
    let user = addrs.pool_owner.clone();
    let funds = vec![Coin::new(1_500_000u128, BLUECHIP_DENOM)];
    let info = message_info(&user, &funds);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    let refund_seen = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, amount }) => {
            to_address == user.as_str()
                && amount.iter().any(|c| c.denom == BLUECHIP_DENOM
                    && c.amount == Uint128::new(500_000))
        }
        _ => false,
    });
    assert!(refund_seen, "500k overpayment should be refunded as BankMsg");
}

#[test]
fn deposit_rejects_underpaid_native() {
    let (mut deps, addrs) = instantiate_default_pool();

    let user = addrs.pool_owner.clone();
    // Send less than requested — collect_deposit_side returns
    // InvalidNativeAmount.
    let funds = vec![Coin::new(500_000u128, BLUECHIP_DENOM)];
    let info = message_info(&user, &funds);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ContractError::InvalidNativeAmount { .. }
    ));
}

#[test]
fn deposit_rejects_zero_side_for_initial() {
    let (mut deps, addrs) = instantiate_default_pool();

    let user = addrs.pool_owner.clone();
    let funds = vec![Coin::new(1_000_000u128, BLUECHIP_DENOM)];
    let info = message_info(&user, &funds);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::zero(),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap_err();

    match err {
        ContractError::Std(e) => {
            assert!(e.to_string().contains("Initial deposit requires both assets"));
        }
        other => panic!("expected Std error, got {:?}", other),
    }
}

// -- Slippage / ratio guards --------------------------------------------

#[test]
fn deposit_slippage_guard_triggers() {
    let (mut deps, addrs) = instantiate_default_pool();

    // First deposit at 1:2 ratio.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[Coin::new(1_000_000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Second deposit supplies 1M native, 2M creator — but demands
    // min_amount1 = 5M, which ratio math can't meet.
    let err = execute(
        deps.as_mut(),
        env_after_rate_limit(),
        message_info(&addrs.pool_owner, &[Coin::new(1_000_000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: Some(Uint128::new(1_000_000)),
            min_amount1: Some(Uint128::new(5_000_000)), // unreasonable
            transaction_deadline: None,
        },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::SlippageExceeded { .. }));
}

// -- Deadline guard ------------------------------------------------------

#[test]
fn deposit_rejects_past_deadline() {
    let (mut deps, addrs) = instantiate_default_pool();

    let env = mock_env();
    let past = env.block.time.minus_seconds(1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.pool_owner, &[Coin::new(1_000_000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: Some(past),
        },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::TransactionExpired {}));
}

// -- Unused import shim: referenced from `matches!` only. ----------------

#[allow(dead_code)]
const _ADDR_USED: Option<Addr> = None;
