//! Standard-pool emergency-withdraw flow: Phase 1 initiate → Phase 2
//! drain. Standard pools pass `accumulation_drain = 0` on both sides
//! (no CREATOR_EXCESS_POSITION to sweep, no DISTRIBUTION_STATE to halt).

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::{Addr, Coin, CosmosMsg, Uint128, WasmMsg};
use pool_core::state::{
    EMERGENCY_DRAINED, EMERGENCY_WITHDRAWAL, PENDING_EMERGENCY_WITHDRAW, POOL_FEE_STATE,
    POOL_PAUSED, POOL_STATE,
};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

fn seed(deps: &mut cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier,
>, user: &Addr) {
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(user, &[Coin::new(1_000_000_000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000_000),
            amount1: Uint128::new(2_000_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();
}

#[test]
fn phase1_initiate_pauses_and_arms_timelock() {
    let (mut deps, addrs) = instantiate_default_pool();

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    assert!(POOL_PAUSED.load(&deps.storage).unwrap());
    assert!(PENDING_EMERGENCY_WITHDRAW
        .may_load(&deps.storage)
        .unwrap()
        .is_some());
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "emergency_withdraw_initiated"));
}

#[test]
fn phase1_rejects_non_factory() {
    let (mut deps, _addrs) = instantiate_default_pool();
    let attacker = cosmwasm_std::testing::MockApi::default().addr_make("attacker");
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&attacker, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn phase2_before_timelock_rejects() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed(&mut deps, &addrs.pool_owner);

    // Phase 1 arms the 24h timelock.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Phase 2 in the same block: timelock not elapsed.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::EmergencyTimelockPending { .. }
    ));
}

#[test]
fn phase2_drains_reserves_and_emits_transfers() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed(&mut deps, &addrs.pool_owner);

    // Phase 1.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Advance time past the 24h timelock.
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(25 * 3600);

    let state_before = POOL_STATE.load(&deps.storage).unwrap();
    let res = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Drain recipient = COMMITFEEINFO.bluechip_wallet_address, which
    // instantiate set to the factory address.
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "recipient" && a.value == addrs.factory.to_string()));

    // Post-drain state: reserves zeroed, drain flag flipped.
    let state_after = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(state_after.reserve0, Uint128::zero());
    assert_eq!(state_after.reserve1, Uint128::zero());
    assert_eq!(state_after.total_liquidity, Uint128::zero());
    let fees_after = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(fees_after.fee_reserve_0, Uint128::zero());
    assert_eq!(fees_after.fee_reserve_1, Uint128::zero());
    assert!(EMERGENCY_DRAINED.load(&deps.storage).unwrap());

    // Audit record captured the pre-drain reserves as the grand total.
    // Standard pools pass accumulation_drain=0 so total_0/1 == reserves +
    // fee_reserves + CREATOR_FEE_POT (which is zero here).
    let audit = EMERGENCY_WITHDRAWAL.load(&deps.storage).unwrap();
    assert_eq!(audit.amount0, state_before.reserve0);
    assert_eq!(audit.amount1, state_before.reserve1);
    assert_eq!(audit.recipient, addrs.factory);

    // Response carries the two transfer messages.
    let bank_sent = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, .. }) => {
            to_address == addrs.factory.as_str()
        }
        _ => false,
    });
    let cw20_sent = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == addrs.creator_token.as_str()
                && String::from_utf8_lossy(msg.as_slice()).contains("transfer")
        }
        _ => false,
    });
    assert!(bank_sent && cw20_sent, "drain must emit both transfers");
}

#[test]
fn phase2_after_drain_rejects_as_already_drained() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed(&mut deps, &addrs.pool_owner);

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(25 * 3600);
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Second attempt: EMERGENCY_DRAINED is true.
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::EmergencyDrained {}));
}

#[test]
fn cancel_emergency_withdraw_clears_pending() {
    let (mut deps, addrs) = instantiate_default_pool();

    // Phase 1.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();
    assert!(POOL_PAUSED.load(&deps.storage).unwrap());

    // Cancel.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::CancelEmergencyWithdraw {},
    )
    .unwrap();

    assert!(!POOL_PAUSED.load(&deps.storage).unwrap());
    assert!(PENDING_EMERGENCY_WITHDRAW
        .may_load(&deps.storage)
        .unwrap()
        .is_none());
}

#[test]
fn cancel_without_pending_rejects() {
    let (mut deps, addrs) = instantiate_default_pool();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::CancelEmergencyWithdraw {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::NoPendingEmergencyWithdraw {}));
}
