//! SubMsg-based deposit balance verification on standard-pool.
//!
//! Standard pools wrap arbitrary CW20s. A fee-on-transfer or rebasing
//! token would otherwise let `actual_amount` (what the pool credits)
//! drift away from the pool's real CW20 balance, leaving a swap-then-
//! drain vector. The verify path snapshots the pool's CW20 balance
//! pre-TransferFrom, dispatches the final outgoing message as a
//! `SubMsg::reply_on_success(.., DEPOSIT_VERIFY_REPLY_ID)`, and the
//! `reply` entry point asserts `post - pre == credited`. A mismatch
//! returns `Err`, which propagates as a chain-level failure and rolls
//! back the entire transaction (position save, NFT mint, reserve
//! update, and so on).
//!
//! This file covers:
//! - The deposit/add response carries a `reply_on_success` SubMsg
//! with the right reply id.
//! - DEPOSIT_VERIFY_CTX persists the pre-balance + credited delta
//! for the reply to consume.
//! - The reply succeeds when the post-balance matches.
//! - The reply rejects shortfall (fee-on-transfer / rebase down).
//! - The reply rejects overage (inflation / rebase up).
//! - Unknown reply ids and missing context both error.

use cosmwasm_std::testing::{message_info, mock_env, MockApi};
use cosmwasm_std::{
    to_json_binary, Addr, Binary, Coin, ContractResult, Reply, ReplyOn, SubMsgResponse,
    SubMsgResult, SystemResult, Uint128, WasmQuery,
};
use cw20::BalanceResponse as Cw20BalanceResponse;
use pool_core::state::{DepositVerifyContext, DEPOSIT_VERIFY_CTX, DEPOSIT_VERIFY_REPLY_ID};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::{execute, reply};
use crate::msg::ExecuteMsg;

/// Helper to install a CW20 balance querier that returns a specific
/// balance for the pool's address. Used to exercise the reply path
/// with controlled deltas.
fn install_cw20_balance_querier(
    deps: &mut cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >,
    nft_contract: Addr,
    nft_owner: Addr,
    cw20_balance: Uint128,
) {
    let nft_str = nft_contract.to_string();
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if *contract_addr == nft_str {
                if let Ok(pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf { .. }) =
                    cosmwasm_std::from_json(msg)
                {
                    let resp = pool_factory_interfaces::cw721_msgs::OwnerOfResponse {
                        owner: nft_owner.to_string(),
                        approvals: vec![],
                    };
                    return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
                }
            }
            if let Ok(cw20::Cw20QueryMsg::Balance { .. }) = cosmwasm_std::from_json(msg) {
                let resp = Cw20BalanceResponse {
                    balance: cw20_balance,
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

/// Runs a first deposit (1_000_000 native + 2_000_000 cw20) and returns
/// the response so callers can inspect the SubMsg shape and the
/// transient verify context.
fn run_first_deposit() -> (
    cosmwasm_std::OwnedDeps<
        cosmwasm_std::testing::MockStorage,
        cosmwasm_std::testing::MockApi,
        cosmwasm_std::testing::MockQuerier,
    >,
    super::fixtures::FixtureAddrs,
    cosmwasm_std::Response,
) {
    let (mut deps, addrs) = instantiate_default_pool();
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
    (deps, addrs, res)
}

#[test]
fn deposit_emits_reply_on_success_submsg_with_verify_id() {
    let (_deps, _addrs, res) = run_first_deposit();

    // The LAST SubMsg in the response must be tagged reply_on_success
    // with DEPOSIT_VERIFY_REPLY_ID — that's the anchor for the reply
    // handler to verify post-balances.
    assert!(!res.messages.is_empty(), "deposit must emit messages");
    let last = res.messages.last().unwrap();
    assert_eq!(
        last.id, DEPOSIT_VERIFY_REPLY_ID,
        "last SubMsg must carry DEPOSIT_VERIFY_REPLY_ID so its reply triggers \
         the balance verification handler"
    );
    assert!(
        matches!(last.reply_on, ReplyOn::Success),
        "DEPOSIT_VERIFY_REPLY_ID anchor must be reply_on_success — error/always \
         would let a downstream rollback be silently absorbed"
    );

    // Every other SubMsg in the response is a fire-and-forget; they MUST
    // NOT carry a reply id (would dispatch into the reply handler and
    // hit the unknown-id branch).
    for sub in res.messages.iter().take(res.messages.len() - 1) {
        assert_eq!(
            sub.id, 0,
            "non-anchor SubMsgs must not carry a reply id; got id={}",
            sub.id
        );
    }
}

#[test]
fn deposit_saves_verify_context_with_pre_balance_and_expected_delta() {
    let (deps, addrs, _res) = run_first_deposit();
    let ctx = DEPOSIT_VERIFY_CTX
        .may_load(&deps.storage)
        .unwrap()
        .expect("verify context must be saved on deposit");

    // Side 0 is the Native bluechip side — no CW20 to verify.
    assert!(ctx.cw20_side0_addr.is_none());
    assert_eq!(ctx.pre_balance0, Uint128::zero());
    // expected_delta0 carries the credited native amount; the reply
    // path ignores it for native sides but the field is preserved.
    assert_eq!(ctx.expected_delta0, Uint128::new(1_000_000));

    // Side 1 is the CreatorToken — this is what the reply verifies.
    assert_eq!(
        ctx.cw20_side1_addr.as_ref().unwrap(),
        &addrs.creator_token,
        "side-1 cw20 address must match the deposited CreatorToken contract"
    );
    // Mock querier returned 0 for the pre-balance (pool has no CW20
    // before the TransferFrom processes).
    assert_eq!(ctx.pre_balance1, Uint128::zero());
    assert_eq!(ctx.expected_delta1, Uint128::new(2_000_000));
}

/// Build a synthetic Reply payload mirroring what cosmwasm dispatches
/// after a `reply_on_success` SubMsg completes successfully.
fn ok_reply() -> Reply {
    #[allow(deprecated)]
    let result = SubMsgResult::Ok(SubMsgResponse {
        events: vec![],
        // deprecated in CW 2.x in favor of msg_responses, but kept here
        // for completeness — the verify reply handler ignores both fields.
        data: None,
        msg_responses: vec![],
    });
    Reply {
        id: DEPOSIT_VERIFY_REPLY_ID,
        result,
        gas_used: 0,
        payload: Binary::default(),
    }
}

#[test]
fn reply_succeeds_and_clears_context_when_post_balance_matches_expected() {
    let (mut deps, addrs, _res) = run_first_deposit();

    // Simulate the TransferFrom landing exactly the credited amount on
    // the pool: post-balance == pre + expected == 0 + 2_000_000.
    install_cw20_balance_querier(
        &mut deps,
        addrs.position_nft.clone(),
        addrs.pool_owner.clone(),
        Uint128::new(2_000_000),
    );

    let resp = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap();
    assert!(
        resp.attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "deposit_balance_verified"),
        "successful verify must emit a deposit_balance_verified action attribute"
    );

    // Context must be cleared — leaving it would let a future deposit's
    // reply consume a stale snapshot.
    assert!(
        DEPOSIT_VERIFY_CTX.may_load(&deps.storage).unwrap().is_none(),
        "transient verify context must be removed on success"
    );
}

#[test]
fn reply_rejects_fee_on_transfer_shortfall() {
    let (mut deps, addrs, _res) = run_first_deposit();

    // Fee-on-transfer simulation: the CW20 took a 1% tax, so only
    // 1_980_000 actually landed on the pool (pre 0 → post 1_980_000,
    // delta 1_980_000), but the deposit handler credited 2_000_000.
    install_cw20_balance_querier(
        &mut deps,
        addrs.position_nft.clone(),
        addrs.pool_owner.clone(),
        Uint128::new(1_980_000),
    );

    let err = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("net-balance invariant violated"),
        "shortfall must surface a clear net-balance-invariant error, got: {}",
        msg
    );

    // On Err, the context can be cleared or preserved — the rollback
    // behavior at chain level discards the entire tx including any
    // storage writes here. We don't assert either way.
}

#[test]
fn reply_rejects_inflation_overage() {
    let (mut deps, addrs, _res) = run_first_deposit();

    // Mint-on-transfer simulation: post balance is HIGHER than expected,
    // (e.g. positive rebase mid-transfer). Crediting the full delta would
    // grow the pool's reserves without payment; reject.
    install_cw20_balance_querier(
        &mut deps,
        addrs.position_nft.clone(),
        addrs.pool_owner.clone(),
        Uint128::new(2_500_000),
    );

    let err = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap_err();
    assert!(
        err.to_string().contains("net-balance invariant violated"),
        "overage must surface the same net-balance-invariant error: {}",
        err
    );
}

#[test]
fn reply_rejects_when_no_context_was_saved() {
    let (mut deps, _addrs) = instantiate_default_pool();
    let err = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap_err();
    assert!(
        err.to_string().contains("without a saved context"),
        "missing-context branch must surface a self-explanatory error: {}",
        err
    );
}

#[test]
fn reply_rejects_unknown_reply_id() {
    let (mut deps, _addrs) = instantiate_default_pool();
    let mut wrong = ok_reply();
    wrong.id = 0xBAD_BAD;
    let err = reply(deps.as_mut(), mock_env(), wrong).unwrap_err();
    assert!(
        err.to_string().contains("unknown reply id"),
        "unknown reply id must be rejected: {}",
        err
    );
}

#[test]
fn add_to_position_emits_reply_on_success_anchor() {
    let (mut deps, addrs, _res) = run_first_deposit();
    DEPOSIT_VERIFY_CTX.remove(&mut deps.storage);

    // Advance past rate limit and deposit again on the same position.
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(60);
    let funds = vec![Coin::new(500_000u128, BLUECHIP_DENOM)];
    let info = message_info(&addrs.pool_owner, &funds);
    let res = execute(
        deps.as_mut(),
        env,
        info,
        ExecuteMsg::AddToPosition {
            position_id: "1".to_string(),
            amount0: Uint128::new(500_000),
            amount1: Uint128::new(1_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Same anchor pattern as DepositLiquidity.
    let last = res.messages.last().unwrap();
    assert_eq!(
        last.id, DEPOSIT_VERIFY_REPLY_ID,
        "add_to_position must also wire the verify SubMsg"
    );
    assert!(matches!(last.reply_on, ReplyOn::Success));

    let ctx = DEPOSIT_VERIFY_CTX
        .may_load(&deps.storage)
        .unwrap()
        .expect("add_to_position must save verify context");
    assert_eq!(ctx.cw20_side1_addr.as_ref().unwrap(), &addrs.creator_token);
    // Pre-balance is 0 in this in-process flow (the first deposit's
    // TransferFrom never actually executed); expected_delta1 captures
    // the credited amount of this add-to-position call.
    assert_eq!(ctx.expected_delta1, Uint128::new(1_000_000));
}

/// Defense-in-depth: if a future refactor accidentally seeds
/// DEPOSIT_VERIFY_CTX with a cw20 address that doesn't resolve, the
/// reply path must surface a real error rather than silently passing.
#[test]
fn reply_propagates_query_failure_on_missing_cw20() {
    let (mut deps, addrs) = instantiate_default_pool();
    // Save a context that points at a contract address with no querier
    // entry — the strict query helper will return a SystemError, which
    // the reply must propagate.
    let bogus_cw20 = MockApi::default().addr_make("not_a_real_cw20");
    DEPOSIT_VERIFY_CTX
        .save(
            &mut deps.storage,
            &DepositVerifyContext {
                pool_addr: addrs.factory.clone(),
                cw20_side0_addr: None,
                cw20_side1_addr: Some(bogus_cw20),
                pre_balance0: Uint128::zero(),
                pre_balance1: Uint128::zero(),
                expected_delta0: Uint128::zero(),
                expected_delta1: Uint128::new(1),
                outgoing_amount0: Uint128::zero(),
                outgoing_amount1: Uint128::zero(),
            },
        )
        .unwrap();
    let err = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap_err();
    // Either the strict query bubbles up, OR the delta check fires —
    // both keep the invariant. We just assert it's an error.
    assert!(!err.to_string().is_empty());
}

/// Regression for Finding 12.1 — `AddToPosition` with prior CW20-side
/// fee accrual must NOT trip the balance-verify reply.
///
/// Pre-fix, the verify reply enforced `delta == actual_amount`. On
/// `add_to_position` with non-zero `fees_owed_1`, the LAST message in
/// the Response was the CW20 fee transfer OUT (after the TransferFrom
/// in). Post-balance reflected `pre + deposited - fee_out`, so
/// `delta = deposited - fee_out != deposited`, and the reply rejected
/// every add-to-position whose position had any prior CW20-side fee
/// accrual.
///
/// Post-fix: the reply enforces
/// `post + outgoing == pre + actual_amount`. With `outgoing = fee_out`,
/// the invariant holds exactly when the CW20 behaves honestly. This
/// test exercises the reply directly against a context that mirrors
/// what `add_to_position_internal` would save when `fees_owed_1 > 0`.
#[test]
fn add_to_position_verify_accepts_when_post_balance_reflects_fee_outflow() {
    let (mut deps, addrs, _res) = run_first_deposit();
    DEPOSIT_VERIFY_CTX.remove(&mut deps.storage);

    // Simulate `add_to_position` with:
    //   pre_balance1 = 500_000 (existing CW20 reserve from prior deposit)
    //   expected_delta1 = 200_000 (CW20 amount being added in this add)
    //   outgoing_amount1 = 5_000 (CW20-side fee being paid out as part of the same tx)
    // Honest CW20 → post = pre + 200_000 - 5_000 = 695_000.
    // Invariant: post + outgoing == pre + actual_in
    //            695_000 + 5_000 == 500_000 + 200_000 → 700_000 == 700_000. ✓
    DEPOSIT_VERIFY_CTX
        .save(
            &mut deps.storage,
            &DepositVerifyContext {
                pool_addr: addrs.factory.clone(),
                cw20_side0_addr: None,
                cw20_side1_addr: Some(addrs.creator_token.clone()),
                pre_balance0: Uint128::zero(),
                pre_balance1: Uint128::new(500_000),
                expected_delta0: Uint128::zero(),
                expected_delta1: Uint128::new(200_000),
                outgoing_amount0: Uint128::zero(),
                outgoing_amount1: Uint128::new(5_000),
            },
        )
        .unwrap();
    install_cw20_balance_querier(
        &mut deps,
        addrs.position_nft.clone(),
        addrs.pool_owner.clone(),
        Uint128::new(695_000),
    );

    let res = reply(deps.as_mut(), mock_env(), ok_reply())
        .expect("verify must accept the honest add-with-fee-outflow scenario");
    assert!(
        res.attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "deposit_balance_verified"),
        "must hit success path; got: {:?}",
        res.attributes
    );

    // Verify the invariant catches a fee-on-transfer SHORTFALL even with
    // outgoing accounted for: if the CW20 ALSO took a 1% tax on the
    // TransferFrom inflow, post would be 695_000 - 2_000 = 693_000,
    // and the invariant 693_000 + 5_000 == 500_000 + 200_000 fails.
    DEPOSIT_VERIFY_CTX
        .save(
            &mut deps.storage,
            &DepositVerifyContext {
                pool_addr: addrs.factory.clone(),
                cw20_side0_addr: None,
                cw20_side1_addr: Some(addrs.creator_token.clone()),
                pre_balance0: Uint128::zero(),
                pre_balance1: Uint128::new(500_000),
                expected_delta0: Uint128::zero(),
                expected_delta1: Uint128::new(200_000),
                outgoing_amount0: Uint128::zero(),
                outgoing_amount1: Uint128::new(5_000),
            },
        )
        .unwrap();
    install_cw20_balance_querier(
        &mut deps,
        addrs.position_nft.clone(),
        addrs.pool_owner.clone(),
        Uint128::new(693_000),
    );
    let err = reply(deps.as_mut(), mock_env(), ok_reply()).unwrap_err();
    assert!(
        err.to_string().contains("net-balance invariant violated"),
        "fee-on-transfer shortfall must still be caught even with outgoing accounted for; got: {}",
        err
    );
}
