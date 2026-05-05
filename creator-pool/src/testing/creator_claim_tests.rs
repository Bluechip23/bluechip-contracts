//! Commit-only creator claim handlers:
//!   - `execute_claim_creator_fees` — creator sweeps CREATOR_FEE_POT
//!     (the clip-slice accumulated from fee_size_multiplier penalties
//!     on small LP positions).
//!   - `execute_retry_factory_notify` — re-sends NotifyThresholdCrossed
//!     to the factory when the initial submsg's reply_on_error handler
//!     set PENDING_FACTORY_NOTIFY=true.

use cosmwasm_std::testing::{message_info, mock_dependencies, mock_env};
use cosmwasm_std::{Addr, CosmosMsg, SubMsg, Uint128, WasmMsg};
use pool_core::state::{CreatorFeePot, CREATOR_FEE_POT};
use crate::state::PENDING_FACTORY_NOTIFY;

use crate::contract::{execute, execute_retry_factory_notify};
use crate::error::ContractError;
use crate::msg::ExecuteMsg;
use crate::testing::liquidity_tests::setup_pool_storage;

// -- execute_claim_creator_fees -----------------------------------------

#[test]
fn claim_creator_fees_empties_pot_and_emits_transfers() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Seed the creator fee pot.
    CREATOR_FEE_POT
        .save(
            &mut deps.storage,
            &CreatorFeePot {
                amount_0: Uint128::new(10_000),
                amount_1: Uint128::new(20_000),
            },
        )
        .unwrap();

    // Caller must equal COMMITFEEINFO.creator_wallet_address (set by
    // setup_pool_storage to "creator_wallet").
    let info = message_info(&Addr::unchecked("creator_wallet"), &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::ClaimCreatorFees {
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Response carries both transfer messages (BankMsg native +
    // Cw20 Transfer).
    let bank_sent = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, amount }) => {
            to_address == "creator_wallet"
                && amount.iter().any(|c| c.denom == "ubluechip" && c.amount == Uint128::new(10_000))
        }
        _ => false,
    });
    let cw20_sent = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == "token_contract"
                && String::from_utf8_lossy(msg.as_slice()).contains("transfer")
                && String::from_utf8_lossy(msg.as_slice()).contains("20000")
        }
        _ => false,
    });
    assert!(bank_sent, "should emit BankMsg for native pot slice");
    assert!(cw20_sent, "should emit CW20 Transfer for creator-token pot slice");

    // Pot reset to zero (after messages are built).
    let pot = CREATOR_FEE_POT.load(&deps.storage).unwrap();
    assert_eq!(pot.amount_0, Uint128::zero());
    assert_eq!(pot.amount_1, Uint128::zero());
}

#[test]
fn claim_creator_fees_rejects_non_creator() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    CREATOR_FEE_POT
        .save(
            &mut deps.storage,
            &CreatorFeePot {
                amount_0: Uint128::new(10_000),
                amount_1: Uint128::new(20_000),
            },
        )
        .unwrap();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("attacker"), &[]),
        ExecuteMsg::ClaimCreatorFees {
            transaction_deadline: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn claim_creator_fees_rejects_empty_pot() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    // Pot never seeded — returns ZeroAmount when both sides are zero.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("creator_wallet"), &[]),
        ExecuteMsg::ClaimCreatorFees {
            transaction_deadline: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::ZeroAmount {}));
}

#[test]
fn claim_creator_fees_rejects_past_deadline() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    CREATOR_FEE_POT
        .save(
            &mut deps.storage,
            &CreatorFeePot {
                amount_0: Uint128::new(10_000),
                amount_1: Uint128::zero(),
            },
        )
        .unwrap();

    let env = mock_env();
    let past = env.block.time.minus_seconds(1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&Addr::unchecked("creator_wallet"), &[]),
        ExecuteMsg::ClaimCreatorFees {
            transaction_deadline: Some(past),
        },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::TransactionExpired {}));
}

#[test]
fn claim_creator_fees_with_only_native_side() {
    // Pot has amount_0 > 0 but amount_1 == 0 — response has only the
    // BankMsg, no CW20 Transfer. Pot still zeroes out.
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    CREATOR_FEE_POT
        .save(
            &mut deps.storage,
            &CreatorFeePot {
                amount_0: Uint128::new(10_000),
                amount_1: Uint128::zero(),
            },
        )
        .unwrap();

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&Addr::unchecked("creator_wallet"), &[]),
        ExecuteMsg::ClaimCreatorFees {
            transaction_deadline: None,
        },
    )
    .unwrap();

    let bank_count = res.messages.iter().filter(|sub| matches!(sub.msg, CosmosMsg::Bank(_))).count();
    let cw20_count = res.messages.iter().filter(|sub| matches!(sub.msg, CosmosMsg::Wasm(_))).count();
    assert_eq!(bank_count, 1);
    assert_eq!(cw20_count, 0);
}

// -- execute_retry_factory_notify ---------------------------------------

#[test]
fn retry_factory_notify_dispatches_submsg_when_pending() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    // Arm the pending flag (production flow sets this from the
    // reply_on_error handler when the initial factory notify fails).
    PENDING_FACTORY_NOTIFY.save(&mut deps.storage, &true).unwrap();

    // Anyone can call RetryFactoryNotify — factory's POOL_THRESHOLD_
    // MINTED idempotency gates double-mints.
    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let res = execute_retry_factory_notify(deps.as_mut(), mock_env(), info).unwrap();

    // Response carries one submessage targeting the factory contract.
    assert_eq!(res.messages.len(), 1);
    let sub: &SubMsg = &res.messages[0];
    match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            assert_eq!(contract_addr, "factory_contract");
            let body = String::from_utf8_lossy(msg.as_slice());
            assert!(body.contains("notify_threshold_crossed"));
            assert!(body.contains("\"pool_id\":1"));
        }
        other => panic!("expected WasmMsg::Execute, got {:?}", other),
    }

    // Pool-id attribute surfaces for ops visibility.
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "retry_factory_notify"));
}

#[test]
fn retry_factory_notify_rejects_when_no_pending() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    // PENDING_FACTORY_NOTIFY unset — default reads as `false`.

    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let err = execute_retry_factory_notify(deps.as_mut(), mock_env(), info).unwrap_err();
    assert!(matches!(err, ContractError::NoPendingFactoryNotify));
}

#[test]
fn retry_factory_notify_rejects_when_flag_false() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);
    PENDING_FACTORY_NOTIFY.save(&mut deps.storage, &false).unwrap();

    let info = message_info(&Addr::unchecked("anyone"), &[]);
    let err = execute_retry_factory_notify(deps.as_mut(), mock_env(), info).unwrap_err();
    assert!(matches!(err, ContractError::NoPendingFactoryNotify));
}

// -- reply handler -------------------------------------------------------
//
// The pool's `reply` entry point handles two reply IDs:
//   - REPLY_ID_FACTORY_NOTIFY_INITIAL (reply_on_error from
//     trigger_threshold_payout): on Err, sets PENDING_FACTORY_NOTIFY
//     so RetryFactoryNotify can be invoked later. On Ok, no-op
//     (reply_on_error shouldn't fire on success but defensive).
//   - REPLY_ID_FACTORY_NOTIFY_RETRY (reply_always from
//     execute_retry_factory_notify): on Ok, clears PENDING_FACTORY_NOTIFY.
//     On Err, keeps the flag set so another retry can be attempted.
//
// These tests build a synthetic Reply and invoke the handler directly,
// exercising every branch of the matrix.

mod reply_handler_tests {
    use super::*;
    use crate::contract::reply;
    use crate::state::{REPLY_ID_FACTORY_NOTIFY_INITIAL, REPLY_ID_FACTORY_NOTIFY_RETRY};
    use cosmwasm_std::{Binary, Reply, SubMsgResponse, SubMsgResult};

    fn synthetic_reply(id: u64, ok: bool, err_msg: Option<&str>) -> Reply {
        // SubMsgResponse.data is deprecated in favor of msg_responses on
        // CosmWasm 2.0+, but the struct still requires the field for
        // construction. Mirror the `#[allow(deprecated)]` pattern the
        // factory's `pool_create_cleanup` uses where it parses replies.
        #[allow(deprecated)]
        let ok_response = SubMsgResponse {
            events: vec![],
            data: None,
            msg_responses: vec![],
        };
        Reply {
            id,
            payload: Binary::default(),
            gas_used: 0,
            result: if ok {
                SubMsgResult::Ok(ok_response)
            } else {
                SubMsgResult::Err(err_msg.unwrap_or("synthetic failure").to_string())
            },
        }
    }

    /// INITIAL_NOTIFY on Err: handler must set PENDING_FACTORY_NOTIFY=true
    /// and emit `factory_notify_deferred` action attribute. Crucially must
    /// return Ok (the parent commit tx must NOT revert just because the
    /// factory notify failed — that's the entire point of reply_on_error).
    #[test]
    fn reply_initial_notify_on_error_sets_pending_flag() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        // Confirm baseline: pending flag is unset / false.
        assert!(!PENDING_FACTORY_NOTIFY
            .may_load(&deps.storage)
            .unwrap()
            .unwrap_or(false));

        let r = synthetic_reply(
            REPLY_ID_FACTORY_NOTIFY_INITIAL,
            false,
            Some("factory rejected: pool not registered"),
        );
        let res = reply(deps.as_mut(), mock_env(), r).expect("reply must Ok on Err result");

        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "factory_notify_deferred"));
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "reason"
                && a.value.contains("factory rejected: pool not registered")));

        // Pending flag is now armed — RetryFactoryNotify can be invoked.
        assert!(PENDING_FACTORY_NOTIFY.load(&deps.storage).unwrap());
    }

    /// INITIAL_NOTIFY on Ok: defensive no-op path. reply_on_error
    /// shouldn't normally produce Ok, but if a future runtime change
    /// alters delivery semantics the handler must not panic. Returns
    /// empty Response, leaves PENDING_FACTORY_NOTIFY untouched.
    #[test]
    fn reply_initial_notify_on_ok_is_noop() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        // Pre-set to true to verify it's NOT touched on Ok.
        PENDING_FACTORY_NOTIFY.save(&mut deps.storage, &true).unwrap();

        let r = synthetic_reply(REPLY_ID_FACTORY_NOTIFY_INITIAL, true, None);
        let res = reply(deps.as_mut(), mock_env(), r).expect("Ok branch must return Ok response");

        // Empty response — no action attribute.
        assert!(res.attributes.is_empty());
        // Flag preserved.
        assert!(PENDING_FACTORY_NOTIFY.load(&deps.storage).unwrap());
    }

    /// RETRY on Ok: handler clears PENDING_FACTORY_NOTIFY (the retry
    /// succeeded) and emits `factory_notify_retry_succeeded`.
    #[test]
    fn reply_retry_on_ok_clears_pending_flag() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        PENDING_FACTORY_NOTIFY.save(&mut deps.storage, &true).unwrap();

        let r = synthetic_reply(REPLY_ID_FACTORY_NOTIFY_RETRY, true, None);
        let res = reply(deps.as_mut(), mock_env(), r).expect("retry success path must Ok");

        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "factory_notify_retry_succeeded"));
        assert!(!PENDING_FACTORY_NOTIFY.load(&deps.storage).unwrap());
    }

    /// RETRY on Err: handler must NOT propagate the error (would trap
    /// gas in retry loop). Returns Ok, keeps the pending flag set so a
    /// future retry can be attempted, emits `factory_notify_retry_failed`
    /// with the failure reason for ops visibility.
    #[test]
    fn reply_retry_on_error_keeps_pending_flag() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);
        PENDING_FACTORY_NOTIFY.save(&mut deps.storage, &true).unwrap();

        let r = synthetic_reply(
            REPLY_ID_FACTORY_NOTIFY_RETRY,
            false,
            Some("factory paused"),
        );
        let res = reply(deps.as_mut(), mock_env(), r)
            .expect("retry failure must NOT propagate as Err — gas-trap risk");

        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "action" && a.value == "factory_notify_retry_failed"));
        assert!(res
            .attributes
            .iter()
            .any(|a| a.key == "reason" && a.value.contains("factory paused")));
        // Flag stays set — caller can retry again later.
        assert!(PENDING_FACTORY_NOTIFY.load(&deps.storage).unwrap());
    }

    /// Unknown reply ID returns Err. Defends against a future SubMsg
    /// dispatch site forgetting to wire its reply handler — surfaces
    /// the bug immediately rather than silently dropping the result.
    #[test]
    fn reply_unknown_id_returns_error() {
        let mut deps = mock_dependencies();
        setup_pool_storage(&mut deps);

        let r = synthetic_reply(0xDEADBEEF, true, None);
        let err = reply(deps.as_mut(), mock_env(), r).unwrap_err();
        // The reply handler returns `StdResult<Response>`, so the unknown-id
        // path emits a `StdError::generic_err` whose Display contains the
        // canonical phrase for off-chain log scrapers.
        assert!(err.to_string().contains("unknown reply id"));
    }
}
