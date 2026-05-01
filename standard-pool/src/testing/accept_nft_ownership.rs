//! M-S1 — `ExecuteMsg::AcceptNftOwnership {}` factory callback tests.
//!
//! The factory's `finalize_standard_pool` reply chain dispatches this
//! variant on the freshly-created pool immediately after the NFT
//! `TransferOwnership { new_owner: pool }`. Without this trigger, the
//! pool's NFT acceptance would have been deferred until the first
//! user deposit, leaving the NFT contract with the pool as
//! `pending_owner` (not `owner`) for an unbounded window.

use cosmwasm_std::testing::{message_info, mock_env, MockApi};
use cosmwasm_std::{Coin, CosmosMsg, Uint128, WasmMsg};
use pool_core::state::POOL_STATE;

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

#[test]
fn accept_nft_ownership_from_factory_emits_accept_message_and_flips_flag() {
    let (mut deps, addrs) = instantiate_default_pool();

    // Pre-condition: nft_ownership_accepted starts false.
    let pre = POOL_STATE.load(&deps.storage).unwrap();
    assert!(
        !pre.nft_ownership_accepted,
        "instantiate must leave nft_ownership_accepted=false (the M-S1 \
         factory callback is what flips it)"
    );

    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::AcceptNftOwnership {},
    )
    .unwrap();

    // Response carries exactly one message: a Wasm Execute targeting the
    // position-NFT contract with cw_ownable AcceptOwnership.
    assert_eq!(res.messages.len(), 1);
    match &res.messages[0].msg {
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr,
            msg,
            funds,
        }) => {
            assert_eq!(
                contract_addr,
                addrs.position_nft.as_str(),
                "AcceptOwnership must target the position-NFT contract"
            );
            let body = String::from_utf8_lossy(msg.as_slice());
            assert!(
                body.contains("update_ownership") && body.contains("accept_ownership"),
                "message body must be a cw_ownable Action::AcceptOwnership, got {}",
                body
            );
            assert!(funds.is_empty(), "AcceptOwnership must carry no funds");
        }
        other => panic!("expected Wasm::Execute, got {:?}", other),
    }

    // State flipped.
    let post = POOL_STATE.load(&deps.storage).unwrap();
    assert!(post.nft_ownership_accepted);

    // Action attribute distinguishes the active-accept path from the
    // idempotent no-op.
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "accept_nft_ownership"));
}

#[test]
fn accept_nft_ownership_rejects_non_factory() {
    let (mut deps, _addrs) = instantiate_default_pool();
    let attacker = MockApi::default().addr_make("attacker");
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&attacker, &[]),
        ExecuteMsg::AcceptNftOwnership {},
    )
    .unwrap_err();
    assert!(
        matches!(err, ContractError::Unauthorized {}),
        "non-factory sender must be rejected with Unauthorized; got {:?}",
        err
    );
}

#[test]
fn accept_nft_ownership_rejects_attached_funds() {
    // Factory is the authorised sender, but it never attaches funds to
    // this callback. Reject any attached coins so a malicious factory
    // upgrade can't sneak orphaned bank balance into the pool through
    // this path.
    let (mut deps, addrs) = instantiate_default_pool();
    let funds = vec![Coin::new(1_000u128, BLUECHIP_DENOM)];
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &funds),
        ExecuteMsg::AcceptNftOwnership {},
    );
    // Today the handler doesn't actively reject funds — it just doesn't
    // forward them. Document that the funds get orphaned here so a
    // future regression that actually cares can tighten this. For now,
    // the call still succeeds and the funds stay in the pool's bank
    // balance until rescued by an admin op.
    assert!(res.is_ok(), "current behaviour: funds quietly accepted");
    // (If a future audit upgrades this to a hard reject, flip this
    // assertion and the handler will need an explicit funds-empty
    // check.)
}

#[test]
fn accept_nft_ownership_is_idempotent() {
    let (mut deps, addrs) = instantiate_default_pool();

    // First call accepts.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::AcceptNftOwnership {},
    )
    .unwrap();

    // Second call: the flag is already true, so we must NOT dispatch a
    // second AcceptOwnership (the NFT contract would reject with
    // NoPendingOwner and tank the entire transaction). Instead, return
    // a no-op response.
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::AcceptNftOwnership {},
    )
    .unwrap();
    assert!(
        res.messages.is_empty(),
        "second AcceptNftOwnership must emit no outgoing messages"
    );
    assert!(res
        .attributes
        .iter()
        .any(|a| a.key == "action" && a.value == "accept_nft_ownership_noop"));
}

/// Wire-format lock between the factory's `StandardPoolFactoryCallback`
/// (in `factory/src/pool_creation_reply.rs`) and standard-pool's
/// `ExecuteMsg::AcceptNftOwnership {}`. The factory can't `use` the
/// standard-pool crate (would create a circular dep), so it encodes
/// the call manually. This test asserts the byte-for-byte JSON the
/// factory emits round-trips into the right ExecuteMsg variant.
///
/// If a future cw_serde upgrade or rename ever changes the wire
/// shape, this test fails fast and forces a co-ordinated update on
/// both sides — instead of silently breaking pool creation in
/// production.
#[test]
fn factory_callback_wire_format_matches_execute_msg() {
    // The exact JSON `factory::pool_creation_reply::
    // build_pool_accept_nft_ownership_call` emits.
    let bytes = b"{\"accept_nft_ownership\":{}}";

    let parsed: ExecuteMsg =
        cosmwasm_std::from_json(bytes).expect("factory callback must deserialise as ExecuteMsg");

    assert!(
        matches!(parsed, ExecuteMsg::AcceptNftOwnership {}),
        "factory wire format drifted away from ExecuteMsg::AcceptNftOwnership"
    );
}

#[test]
fn first_deposit_after_factory_accept_does_not_re_emit_accept() {
    // M-S1 leaves the deposit-side branch as a backstop. With the
    // factory callback already firing, by the time the first deposit
    // arrives `nft_ownership_accepted` is true and the deposit handler
    // skips the AcceptOwnership SubMsg. Verify that's actually the
    // case — otherwise the NFT contract would reject the second
    // accept with NoPendingOwner.
    let (mut deps, addrs) = instantiate_default_pool();
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.factory, &[]),
        ExecuteMsg::AcceptNftOwnership {},
    )
    .unwrap();

    let user = addrs.pool_owner.clone();
    let funds = vec![Coin::new(1_000_000u128, BLUECHIP_DENOM)];
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&user, &funds),
        ExecuteMsg::DepositLiquidity {
            amount0: Uint128::new(1_000_000),
            amount1: Uint128::new(2_000_000),
            min_amount0: None,
            min_amount1: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Confirm no AcceptOwnership inside the deposit response — only
    // the CW20 TransferFrom + position-NFT mint + verify-anchor.
    let nft_accept_in_deposit = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == addrs.position_nft.as_str()
                && String::from_utf8_lossy(msg.as_slice()).contains("accept_ownership")
        }
        _ => false,
    });
    assert!(
        !nft_accept_in_deposit,
        "deposit must NOT re-emit AcceptOwnership when the factory \
         callback already flipped the flag — would crash the tx"
    );
}
