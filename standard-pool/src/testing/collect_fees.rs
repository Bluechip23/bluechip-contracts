//! Pool-core `execute_collect_fees` via standard-pool's execute
//! dispatch. Tests seed non-zero `fee_growth_global_*` + `fee_reserve_*`
//! after deposit to simulate accumulated swap fees, then verify the
//! collect path.

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::{Coin, CosmosMsg, Decimal, Uint128, WasmMsg};
use pool_core::state::{
    CreatorFeePot, PoolFeeState, CREATOR_FEE_POT, LIQUIDITY_POSITIONS, POOL_FEE_STATE,
};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

/// Deposits 1B native + 2B cw20 as `pool_owner`, which `verify_position_
/// ownership`'s CW721 mock will accept as the owner for any token_id.
fn deposit(
    deps: &mut cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >,
    user: &cosmwasm_std::Addr,
) {
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

/// Directly seeds fee_growth_global + fee_reserve to simulate
/// swap-accumulated fees.
fn seed_fees(deps: &mut cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier,
>, growth: Decimal, reserve_0: Uint128, reserve_1: Uint128) {
    POOL_FEE_STATE
        .save(
            &mut deps.storage,
            &PoolFeeState {
                fee_growth_global_0: growth,
                fee_growth_global_1: growth,
                total_fees_collected_0: Uint128::zero(),
                total_fees_collected_1: Uint128::zero(),
                fee_reserve_0: reserve_0,
                fee_reserve_1: reserve_1,
            },
        )
        .unwrap();
}

#[test]
fn collect_fees_emits_transfers_and_debits_reserve() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);

    // Seed a non-trivial fee delta. Position 1's liquidity (from the
    // 1B x 2B deposit) is ~sqrt(2e18) - 1000 minimum ≈ 1.414e9. With
    // fee_growth_global = 0.001 (permille), owed = liquidity * 0.001 ≈
    // 1.414M on each side. Reserve is large enough to cover.
    seed_fees(
        &mut deps,
        Decimal::permille(1),
        Uint128::new(10_000_000),
        Uint128::new(10_000_000),
    );

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::CollectFees {
            position_id: "1".to_string(),
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Both transfer messages present (BankMsg for native, CW20 Transfer
    // for creator token).
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
    assert!(bank_send, "collect_fees should emit native transfer");
    assert!(cw20_transfer, "collect_fees should emit CW20 transfer");

    // Position's fee_growth_inside_*_last bumped to global (accounting
    // reset).
    let pos = LIQUIDITY_POSITIONS.load(&deps.storage, "1").unwrap();
    assert_eq!(pos.fee_growth_inside_0_last, Decimal::permille(1));
    assert_eq!(pos.fee_growth_inside_1_last, Decimal::permille(1));
    assert_eq!(pos.unclaimed_fees_0, Uint128::zero());
    assert_eq!(pos.unclaimed_fees_1, Uint128::zero());

    // fee_reserve debited (both LP portion and any clip slice).
    let fees_after = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert!(fees_after.fee_reserve_0 < Uint128::new(10_000_000));
    assert!(fees_after.fee_reserve_1 < Uint128::new(10_000_000));
}

/// Standard pools bypass the `fee_size_multiplier` dust-griefing
/// penalty (the multiplier is pinned at `Decimal::one()` at deposit /
/// add / remove time on standard pools via `APPLY_DUST_MULTIPLIER ==
/// false`). The clipped-slice flow that would otherwise route fees to
/// the inaccessible-on-standard `CREATOR_FEE_POT` does NOT fire.
///
/// Dust-griefing protection on standard pools is enforced by the
/// `MIN_STANDARD_POOL_POSITION_LIQUIDITY` deposit floor — verified
/// below.
#[test]
fn standard_pool_collect_fees_does_not_clip_to_creator_pot() {
    let (mut deps, addrs) = instantiate_default_pool();

    // Deposit JUST above the dust floor. `sqrt(amount0 * amount1)`
    // produces the LP units; we need ≥ 100_000 LP units after the
    // first-deposit lock of 1000. amount0 = amount1 = 200_000 gives
    // sqrt = 200_000 — well above the floor and well below the legacy
    // OPTIMAL_LIQUIDITY = 1_000_000 (which is where the old multiplier
    // would have hit 100%). Pre-fix this would have clipped most of
    // the fees to CREATOR_FEE_POT; post-fix the multiplier stays at
    // 1.0 and no clip happens.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[Coin::new(200_000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(200_000),
            amount1: Uint128::new(200_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    seed_fees(
        &mut deps,
        Decimal::permille(10),
        Uint128::new(100_000),
        Uint128::new(100_000),
    );

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::CollectFees {
            position_id: "1".to_string(),
            transaction_deadline: None,
        },
    )
    .unwrap();

    // With multiplier pinned at 1.0, no clip slice is produced. Pot
    // stays empty (or unset) regardless of position size.
    let pot = CREATOR_FEE_POT
        .may_load(&deps.storage)
        .unwrap()
        .unwrap_or_else(CreatorFeePot::default);
    assert_eq!(
        pot.amount_0,
        Uint128::zero(),
        "standard pool collect_fees must not credit CREATOR_FEE_POT side 0"
    );
    assert_eq!(
        pot.amount_1,
        Uint128::zero(),
        "standard pool collect_fees must not credit CREATOR_FEE_POT side 1"
    );
}

/// Standard-pool dust-griefing protection: a deposit whose produced LP
/// units fall below `MIN_STANDARD_POOL_POSITION_LIQUIDITY` is rejected
/// with `DustStandardPoolDeposit`. Replaces the multiplier-based
/// deterrent that pre-fix routed small-position fees to an
/// inaccessible CREATOR_FEE_POT.
#[test]
fn standard_pool_rejects_dust_deposit() {
    let (mut deps, addrs) = instantiate_default_pool();

    // sqrt(1000 * 2000) ≈ 1414 LP units — far below the 100_000 floor.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[Coin::new(1000u128, BLUECHIP_DENOM)]),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1000),
            amount1: Uint128::new(2000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap_err();
    match err {
        ContractError::DustStandardPoolDeposit { liquidity, minimum } => {
            assert!(liquidity < minimum, "error should report sub-floor liquidity");
            assert_eq!(
                minimum,
                pool_core::liquidity_helpers::MIN_STANDARD_POOL_POSITION_LIQUIDITY,
                "error should report the canonical floor",
            );
        }
        other => panic!("expected DustStandardPoolDeposit, got: {:?}", other),
    }
}

#[test]
fn collect_fees_rejects_non_owner() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);
    seed_fees(
        &mut deps,
        Decimal::permille(1),
        Uint128::new(10_000_000),
        Uint128::new(10_000_000),
    );

    // CW721 mock returns pool_owner as the owner regardless of caller;
    // verify_position_ownership sees expected_owner = attacker !=
    // query_response.owner = pool_owner → Unauthorized.
    let attacker = cosmwasm_std::testing::MockApi::default().addr_make("attacker");
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&attacker, &[]),
        ExecuteMsg::CollectFees {
            position_id: "1".to_string(),
            transaction_deadline: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn collect_fees_on_zero_growth_returns_zero_transfers() {
    let (mut deps, addrs) = instantiate_default_pool();
    deposit(&mut deps, &addrs.pool_owner);

    // No fee growth beyond what position's fee_growth_inside_last
    // already captured → no transfers.
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::CollectFees {
            position_id: "1".to_string(),
            transaction_deadline: None,
        },
    )
    .unwrap();

    let transfer_msgs = res.messages.iter().filter(|sub| {
        matches!(
            sub.msg,
            CosmosMsg::Bank(_) | CosmosMsg::Wasm(WasmMsg::Execute { .. })
        )
    }).count();
    // build_fee_transfer_msgs skips zero amounts, so no transfer messages.
    assert_eq!(transfer_msgs, 0, "zero-growth collect emits no transfers");
}
