//! Pool-core liquidity removal (`execute_remove_partial_liquidity`,
//! `execute_remove_all_liquidity`, `execute_remove_partial_liquidity_by_
//! percent`) via standard-pool's execute dispatch.

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::{Addr, Coin, CosmosMsg, Uint128, WasmMsg};
use pool_core::state::{LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, POOL_STATE};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

const NATIVE_DEPOSIT: u128 = 1_000_000_000;
const CW20_DEPOSIT: u128 = 2_000_000_000;

fn deposit(
    deps: &mut cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >,
    user: &Addr,
) {
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(user, &[Coin::new(NATIVE_DEPOSIT, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(NATIVE_DEPOSIT),
            amount1: Uint128::new(CW20_DEPOSIT),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();
}

#[test]
fn remove_all_liquidity_drains_position() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos_before = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert!(!pos_before.liquidity.is_zero());

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemoveAllLiquidity {
            position_id: "1".to_string(),
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap();

    // First-depositor position now persists with `liquidity == locked_liquidity
    // == MINIMUM_LIQUIDITY` so the depositor keeps fee rights on the
    // permanently-locked principal slice. Pre-lock-fix this test asserted
    // the position was burned outright.
    let pos_after = LIQUIDITY_POSITIONS
        .load(&deps.storage, "1")
        .expect("first-depositor position should persist with locked liquidity");
    assert_eq!(pos_after.liquidity, MINIMUM_LIQUIDITY);
    assert_eq!(pos_after.locked_liquidity, MINIMUM_LIQUIDITY);

    // Response carries both transfers back to the owner. Total
    // liquidity includes the MINIMUM_LIQUIDITY lock, so principal
    // returned is ~deposit - sqrt(1000*2000) * proportional share.
    let bank_send = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, .. }) => {
            to_address == addrs.pool_owner.as_str()
        }
        _ => false,
    });
    let cw20_transfer = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == addrs.creator_token.as_str()
                && String::from_utf8_lossy(msg.as_slice()).contains("transfer")
        }
        _ => false,
    });
    assert!(bank_send && cw20_transfer, "remove_all emits both sides");
}

#[test]
fn remove_partial_liquidity_reduces_position() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos_before = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    let half = pos_before.liquidity.u128() / 2;

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidity {
            position_id: "1".to_string(),
            liquidity_to_remove: Uint128::new(half),
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap();

    let pos_after = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(
        pos_after.liquidity,
        pos_before.liquidity - Uint128::new(half)
    );

    // Pool state reserves reduced proportionally.
    let state = POOL_STATE.load(&deps.storage).unwrap();
    assert!(state.reserve0 < Uint128::new(NATIVE_DEPOSIT));
    assert!(state.reserve1 < Uint128::new(CW20_DEPOSIT));
    assert!(!state.total_liquidity.is_zero());
}

#[test]
fn remove_partial_by_percent_50_reduces_half() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos_before = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id: "1".to_string(),
            percentage: 50,
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap();

    let pos_after = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    // Percent is now applied to `removable = liquidity - locked_liquidity`,
    // so 50% removes half of the unlocked slice. Remaining =
    // `locked + ceil(removable / 2)` (integer-floor on the removed half
    // means the surviving half rounds up by the floor remainder).
    let removable_before = pos_before.liquidity - pos_before.locked_liquidity;
    let to_remove = removable_before.u128() / 2; // matches contract's integer-floor
    let expected = pos_before.liquidity.u128() - to_remove;
    assert_eq!(pos_after.liquidity.u128(), expected);
    assert_eq!(pos_after.locked_liquidity, pos_before.locked_liquidity);
}

#[test]
fn remove_partial_by_percent_100_drains_completely() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);

    // percentage >= 100 dispatches to execute_remove_all_liquidity.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id: "1".to_string(),
            percentage: 100,
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap();

    // First-depositor position survives full-remove with `liquidity ==
    // locked_liquidity == MINIMUM_LIQUIDITY` so the depositor retains
    // a fee-earning Position on the permanently-locked principal slice.
    let pos_after = LIQUIDITY_POSITIONS
        .load(&deps.storage, "1")
        .expect("position survives full-remove via the locked-liquidity floor");
    assert_eq!(pos_after.liquidity, MINIMUM_LIQUIDITY);
    assert_eq!(pos_after.locked_liquidity, MINIMUM_LIQUIDITY);
}

#[test]
fn remove_partial_by_percent_rejects_zero() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id: "1".to_string(),
            percentage: 0,
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::InvalidPercent {}));
}

#[test]
fn remove_partial_rejects_over_position_liquidity() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidity {
            position_id: "1".to_string(),
            liquidity_to_remove: pos.liquidity + Uint128::new(1),
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::InsufficientLiquidity {}));
}

#[test]
fn remove_partial_slippage_guard_triggers() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidity {
            position_id: "1".to_string(),
            liquidity_to_remove: pos.liquidity / Uint128::new(10),
            transaction_deadline: None,
            // Demand more native than the 10% share can yield.
            min_amount0: Some(Uint128::new(NATIVE_DEPOSIT * 10)),
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::SlippageExceeded { .. }));
}

#[test]
fn remove_partial_rejects_past_deadline() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    let env = mock_env();
    let past = env.block.time.minus_seconds(1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::RemovePartialLiquidity {
            position_id: "1".to_string(),
            liquidity_to_remove: pos.liquidity / Uint128::new(10),
            transaction_deadline: Some(past),
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::TransactionExpired {}));
}

#[test]
fn remove_partial_rejects_non_owner() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();

    // Mock CW721 says pool_owner owns the position; attacker's
    // message_info disagrees.
    let attacker = cosmwasm_std::testing::MockApi::default().addr_make("attacker");
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&attacker, &[]),
        ExecuteMsg::RemovePartialLiquidity {
            position_id: "1".to_string(),
            liquidity_to_remove: pos.liquidity / Uint128::new(10),
            transaction_deadline: None,
            min_amount0: None,
            min_amount1: None,
            max_ratio_deviation_bps: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}
