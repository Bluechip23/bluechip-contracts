//! `ClaimEmergencyShare` + `SweepUnclaimedEmergencyShares` coverage.
//!
//! Post-drain, LP funds escrow in `EMERGENCY_DRAIN_SNAPSHOT`. Each LP
//! position can claim its pro-rata share at any time inside the
//! `EMERGENCY_CLAIM_DORMANCY_SECONDS` (1 year) window. After dormancy,
//! the factory may sweep the unclaimed residual to `bluechip_wallet`,
//! which hard-closes the claim window.

use cosmwasm_std::testing::{message_info, mock_env, MockApi};
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, ContractResult, CosmosMsg, SystemResult, Uint128,
    WasmQuery,
};
use cw20::BalanceResponse as Cw20BalanceResponse;
use pool_core::state::{
    EMERGENCY_CLAIM_DORMANCY_SECONDS, EMERGENCY_DRAIN_SNAPSHOT, LIQUIDITY_POSITIONS,
};
use pool_factory_interfaces::cw721_msgs::{Cw721QueryMsg, OwnerOfResponse};

use super::fixtures::{instantiate_default_pool, FixtureAddrs, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

type Deps = cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier,
>;

/// Re-wires `deps.querier` so:
/// - the position-NFT contract reports `nft_owner` as the holder of every
///   token (so `verify_position_ownership` accepts that sender),
/// - factory queries (`EmergencyWithdrawDelaySeconds`, `BluechipWalletAddress`)
///   return canonical values used by drain + sweep,
/// - CW20 `Balance` queries return zero (deposit balance-verify path).
fn rewire_querier(deps: &mut Deps, nft_owner: Addr, nft_contract: Addr, bluechip_wallet: Addr) {
    let nft_contract = nft_contract.to_string();
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if *contract_addr == nft_contract {
                if let Ok(Cw721QueryMsg::OwnerOf { .. }) = cosmwasm_std::from_json(msg) {
                    let resp = OwnerOfResponse {
                        owner: nft_owner.to_string(),
                        approvals: vec![],
                    };
                    return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
                }
            }
            if let Ok(pool_factory_interfaces::FactoryQueryMsg::EmergencyWithdrawDelaySeconds {}) =
                cosmwasm_std::from_json(msg)
            {
                let resp = pool_factory_interfaces::EmergencyWithdrawDelayResponse {
                    delay_seconds: 86_400,
                };
                return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
            }
            if let Ok(pool_factory_interfaces::FactoryQueryMsg::BluechipWalletAddress {}) =
                cosmwasm_std::from_json(msg)
            {
                let resp = pool_factory_interfaces::BluechipWalletResponse {
                    address: bluechip_wallet.clone(),
                };
                return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
            }
            if let Ok(cw20::Cw20QueryMsg::Balance { .. }) = cosmwasm_std::from_json(msg) {
                let resp = Cw20BalanceResponse {
                    balance: Uint128::zero(),
                };
                return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
            }
            SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                error: format!("unexpected wasm query to {}", contract_addr),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
            kind: "non-Smart wasm query".to_string(),
        }),
    });
}

/// Deposits one position from `addrs.pool_owner`, then runs Phase 1 +
/// Phase 2 of emergency withdraw — leaving the pool in the drained
/// state with `EMERGENCY_DRAIN_SNAPSHOT` populated. Returns the
/// `position_id` minted on the deposit.
fn setup_drained_pool() -> (Deps, FixtureAddrs, String) {
    let (mut deps, addrs) = instantiate_default_pool();
    rewire_querier(
        &mut deps,
        addrs.pool_owner.clone(),
        addrs.position_nft.clone(),
        addrs.bluechip_wallet.clone(),
    );

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(
            &addrs.pool_owner,
            &[Coin::new(1_000_000_000u128, BLUECHIP_DENOM)],
        ),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000_000),
            amount1: Uint128::new(2_000_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Phase 1.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    // Phase 2 — 25h past instantiate.
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(25 * 3600);
    execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::EmergencyWithdraw {},
    )
    .unwrap();

    (deps, addrs, "1".to_string())
}

// ---------------------------------------------------------------------------
// ClaimEmergencyShare
// ---------------------------------------------------------------------------

#[test]
fn claim_emergency_share_happy_path_sends_pro_rata_and_bumps_snapshot() {
    let (mut deps, addrs, position_id) = setup_drained_pool();

    // Snapshot pre-claim. The lone position holds 100% of total_liquidity,
    // so it must receive the full drained amounts on both sides.
    let pre = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
    assert_eq!(pre.total_claimed_0, Uint128::zero());
    assert_eq!(pre.total_claimed_1, Uint128::zero());
    assert!(!pre.residual_swept);
    let pre_position = LIQUIDITY_POSITIONS
        .load(&deps.storage, &position_id)
        .unwrap();
    assert!(!pre_position.liquidity.is_zero());

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::ClaimEmergencyShare {
            position_id: position_id.clone(),
        },
    )
    .unwrap();

    // Position economically spent.
    let post_position = LIQUIDITY_POSITIONS
        .load(&deps.storage, &position_id)
        .unwrap();
    assert_eq!(post_position.liquidity, Uint128::zero());
    assert_eq!(post_position.unclaimed_fees_0, Uint128::zero());
    assert_eq!(post_position.unclaimed_fees_1, Uint128::zero());

    // Snapshot tally bumped to exactly the drained-side totals (sole LP
    // owns 100% of total_liquidity_at_drain).
    let post = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
    assert_eq!(post.total_claimed_0, pre.reserve0_at_drain + pre.fee_reserve_0_at_drain);
    assert_eq!(post.total_claimed_1, pre.reserve1_at_drain + pre.fee_reserve_1_at_drain);
    assert!(!post.residual_swept, "single claim must not flip the sweep flag");

    // Outgoing transfers: a Bank Send for the native side, a CW20
    // Transfer for the creator-token side, both addressed to the
    // claimant (pool_owner).
    let bank_to_owner_native = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
            to_address == addrs.pool_owner.as_str()
                && amount.iter().any(|c| c.denom == BLUECHIP_DENOM && !c.amount.is_zero())
        }
        _ => false,
    });
    assert!(bank_to_owner_native, "expected native pro-rata bank send to claimant");

    let cw20_to_owner = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == &addrs.creator_token.to_string()
                && String::from_utf8_lossy(msg.as_slice()).contains(addrs.pool_owner.as_str())
        }
        _ => false,
    });
    assert!(cw20_to_owner, "expected CW20 pro-rata transfer to claimant");
}

#[test]
fn claim_emergency_share_rejects_before_drain() {
    let (mut deps, addrs) = instantiate_default_pool();
    rewire_querier(
        &mut deps,
        addrs.pool_owner.clone(),
        addrs.position_nft.clone(),
        addrs.bluechip_wallet.clone(),
    );

    // Deposit, no emergency-drain.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(
            &addrs.pool_owner,
            &[Coin::new(1_000_000_000u128, BLUECHIP_DENOM)],
        ),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000_000),
            amount1: Uint128::new(2_000_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::ClaimEmergencyShare {
            position_id: "1".to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::NoEmergencyDrainSnapshot));
}

#[test]
fn claim_emergency_share_rejects_wrong_nft_owner() {
    let (mut deps, _addrs, position_id) = setup_drained_pool();
    let attacker = MockApi::default().addr_make("attacker");

    // The querier still says `pool_owner` owns the NFT, so a claim sent
    // by `attacker` must fail the ownership gate.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&attacker, &[]),
        ExecuteMsg::ClaimEmergencyShare { position_id },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn claim_emergency_share_rejects_after_claim_zeroes_liquidity() {
    let (mut deps, addrs, position_id) = setup_drained_pool();

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::ClaimEmergencyShare {
            position_id: position_id.clone(),
        },
    )
    .unwrap();

    // Position liquidity is now zero — second claim must reject.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::ClaimEmergencyShare { position_id },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::NoClaimableEmergencyShare { .. }
    ));
}

#[test]
fn claim_emergency_share_rejects_post_sweep() {
    let (mut deps, addrs, position_id) = setup_drained_pool();

    // Advance past dormancy and sweep first.
    let mut env = mock_env();
    env.block.time = env
        .block
        .time
        .plus_seconds(25 * 3600 + EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap();

    // Now a (late) claim must hard-fail with the post-sweep gate, NOT
    // silently succeed and bump total_claimed_* beyond drainable.
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.pool_owner, &[]),
        ExecuteMsg::ClaimEmergencyShare { position_id },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::EmergencyClaimsClosedPostSweep));
}

// ---------------------------------------------------------------------------
// SweepUnclaimedEmergencyShares
// ---------------------------------------------------------------------------

#[test]
fn sweep_rejects_non_factory() {
    let (mut deps, _addrs, _) = setup_drained_pool();
    let attacker = MockApi::default().addr_make("attacker");

    let mut env = mock_env();
    env.block.time = env
        .block
        .time
        .plus_seconds(25 * 3600 + EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&attacker, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn sweep_rejects_before_dormancy_elapsed() {
    let (mut deps, addrs, _) = setup_drained_pool();

    // 23h past drain — well inside the 1-year dormancy.
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(25 * 3600 + 23 * 3600);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::EmergencyClaimDormancyNotElapsed { .. }
    ));
}

#[test]
fn sweep_post_dormancy_sends_residual_to_wallet_and_flips_flag() {
    let (mut deps, addrs, _) = setup_drained_pool();

    // No prior claims, so the entire drained amount is residual.
    let pre = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
    assert_eq!(pre.total_claimed_0, Uint128::zero());
    assert_eq!(pre.total_claimed_1, Uint128::zero());
    assert!(!pre.residual_swept);

    let mut env = mock_env();
    env.block.time = env
        .block
        .time
        .plus_seconds(25 * 3600 + EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);
    let res = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap();

    let post = EMERGENCY_DRAIN_SNAPSHOT.load(&deps.storage).unwrap();
    assert!(post.residual_swept, "sweep must flip the residual_swept flag");

    // Every outgoing transfer must address the bluechip wallet (the
    // factory's live-queried wallet — rewired by the fixture).
    let bank_to_wallet = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(BankMsg::Send { to_address, .. }) => {
            to_address == addrs.bluechip_wallet.as_str()
        }
        _ => false,
    });
    let cw20_to_wallet = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == &addrs.creator_token.to_string()
                && String::from_utf8_lossy(msg.as_slice()).contains(addrs.bluechip_wallet.as_str())
        }
        _ => false,
    });
    assert!(bank_to_wallet, "sweep must Bank-send the native residual to bluechip wallet");
    assert!(cw20_to_wallet, "sweep must CW20-transfer the creator-token residual to bluechip wallet");
}

#[test]
fn sweep_rejects_double_call() {
    let (mut deps, addrs, _) = setup_drained_pool();

    let mut env = mock_env();
    env.block.time = env
        .block
        .time
        .plus_seconds(25 * 3600 + EMERGENCY_CLAIM_DORMANCY_SECONDS + 1);
    execute(
        deps.as_mut(),
        env.clone(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        env,
        message_info(&addrs.factory, &[]),
        ExecuteMsg::SweepUnclaimedEmergencyShares {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::NoUnclaimedEmergencyResidual));
}
