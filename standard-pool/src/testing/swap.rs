//! Pool-core swap paths via standard-pool's execute dispatch.

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::{to_json_binary, Coin, CosmosMsg, Uint128, WasmMsg};
use cw20::Cw20ReceiveMsg;
use pool_core::asset::{TokenInfo, TokenType};
use pool_core::msg::Cw20HookMsg;
use pool_core::state::{IS_THRESHOLD_HIT, POOL_ANALYTICS, POOL_PAUSED, POOL_STATE};

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::error::ContractError;
use crate::msg::ExecuteMsg;

/// Seed a deposit so subsequent swaps have reserves to work with.
fn seed_pool(deps: &mut cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier,
>, user: &cosmwasm_std::Addr) {
    let funds = vec![Coin::new(1_000_000_000u128, BLUECHIP_DENOM)];
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(user, &funds),
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
fn simple_swap_native_to_cw20_returns_token_transfer() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    let funds = vec![Coin::new(10_000u128, BLUECHIP_DENOM)];
    let res = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&trader, &funds),
        ExecuteMsg::SimpleSwap {
            offer_asset: TokenInfo {
                info: TokenType::Native {
                    denom: BLUECHIP_DENOM.to_string(),
                },
                amount: Uint128::new(10_000),
            },
            belief_price: None,
            max_spread: None,
            allow_high_max_spread: None,
            to: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    // Should emit a CW20 Transfer to the trader for the ask token.
    let cw20_transfer = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Wasm(WasmMsg::Execute { contract_addr, msg, .. }) => {
            contract_addr == addrs.creator_token.as_str()
                && String::from_utf8_lossy(msg.as_slice()).contains("transfer")
        }
        _ => false,
    });
    assert!(cw20_transfer, "swap should emit CW20 Transfer of return amount");

    // Analytics bumped.
    let analytics = POOL_ANALYTICS.load(&deps.storage).unwrap();
    assert_eq!(analytics.total_swap_count, 1);
    assert!(analytics.total_volume_0 >= Uint128::new(10_000));
}

#[test]
fn cw20_hook_swap_dispatches_via_receive() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    // CW20 hook: sender is the CW20 contract address itself (simulating
    // the callback); cw20_msg.sender is the original caller.
    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");

    // audit: the swap path's synchronous CW20 balance verify checks
    // that the pool's actual CW20 balance covers
    // `reserve1 + fee_reserve_1 + creator_pot.amount_1 + cw20_msg.amount`.
    // The default fixture mock returns zero for every Cw20Balance query
    // (true in test-env where TransferFrom messages are collected but
    // never executed), which would trip the new check. Install a
    // per-test override that returns the post-Receive balance the
    // verify is expecting. `seed_pool` deposited 2_000_000_000 CW20;
    // this Receive claims another 10_000.
    let nft_contract = addrs.position_nft.to_string();
    let creator_token = addrs.creator_token.to_string();
    deps.querier.update_wasm(move |query| match query {
        cosmwasm_std::WasmQuery::Smart { contract_addr, msg } => {
            if *contract_addr == nft_contract {
                if let Ok(pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf { .. }) =
                    cosmwasm_std::from_json(msg)
                {
                    let resp = pool_factory_interfaces::cw721_msgs::OwnerOfResponse {
                        owner: addrs.pool_owner.to_string(),
                        approvals: vec![],
                    };
                    return cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Ok(
                        cosmwasm_std::to_json_binary(&resp).unwrap(),
                    ));
                }
            }
            if *contract_addr == creator_token {
                if let Ok(cw20::Cw20QueryMsg::Balance { .. }) = cosmwasm_std::from_json(msg) {
                    let resp = cw20::BalanceResponse {
                        balance: Uint128::new(2_000_000_000 + 10_000),
                    };
                    return cosmwasm_std::SystemResult::Ok(cosmwasm_std::ContractResult::Ok(
                        cosmwasm_std::to_json_binary(&resp).unwrap(),
                    ));
                }
            }
            cosmwasm_std::SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                error: format!("unexpected wasm query to {}", contract_addr),
                request: msg.clone(),
            })
        }
        _ => cosmwasm_std::SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
            kind: "non-Smart wasm query".to_string(),
        }),
    });

    let hook = Cw20HookMsg::Swap {
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    let cw20_msg = Cw20ReceiveMsg {
        sender: trader.to_string(),
        amount: Uint128::new(10_000),
        msg: to_json_binary(&hook).unwrap(),
    };
    let info = message_info(&addrs.creator_token, &[]);
    let res = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::Receive(cw20_msg),
    )
    .unwrap();

    // Should emit a BankMsg::Send of the ask amount (native) to the trader.
    let bank_send = res.messages.iter().any(|sub| match &sub.msg {
        CosmosMsg::Bank(cosmwasm_std::BankMsg::Send { to_address, amount }) => {
            to_address == trader.as_str()
                && amount.iter().any(|c| c.denom == BLUECHIP_DENOM && !c.amount.is_zero())
        }
        _ => false,
    });
    assert!(bank_send, "cw20 hook swap should emit BankMsg to trader");
}

#[test]
fn cw20_hook_rejects_unknown_cw20_contract() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    // Sender pretending to be a CW20 hook, but from an address that
    // isn't on the pair → Unauthorized.
    let rando = cosmwasm_std::testing::MockApi::default().addr_make("rando_cw20");
    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    let hook = Cw20HookMsg::Swap {
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    let cw20_msg = Cw20ReceiveMsg {
        sender: trader.to_string(),
        amount: Uint128::new(10_000),
        msg: to_json_binary(&hook).unwrap(),
    };
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&rando, &[]),
        ExecuteMsg::Receive(cw20_msg),
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn simple_swap_rejects_when_pool_paused() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    // Pause the pool by directly flipping the flag (admin pause goes
    // through another entry point; this test just isolates the swap
    // behavior on a paused pool).
    POOL_PAUSED.save(&mut deps.storage, &true).unwrap();

    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    let funds = vec![Coin::new(10_000u128, BLUECHIP_DENOM)];
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&trader, &funds),
        ExecuteMsg::SimpleSwap {
            offer_asset: TokenInfo {
                info: TokenType::Native {
                    denom: BLUECHIP_DENOM.to_string(),
                },
                amount: Uint128::new(10_000),
            },
            belief_price: None,
            max_spread: None,
            allow_high_max_spread: None,
            to: None,
            transaction_deadline: None,
        },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::PoolPausedLowLiquidity {}));
}

#[test]
fn simple_swap_rejects_wrong_asset() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    // Offer a denom that's not in the pair.
    let funds = vec![Coin::new(10_000u128, "uatom")];
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&trader, &funds),
        ExecuteMsg::SimpleSwap {
            offer_asset: TokenInfo {
                info: TokenType::Native {
                    denom: "uatom".to_string(),
                },
                amount: Uint128::new(10_000),
            },
            belief_price: None,
            max_spread: None,
            allow_high_max_spread: None,
            to: None,
            transaction_deadline: None,
        },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::AssetMismatch {}));
}

// IS_THRESHOLD_HIT is always true for standard pools at instantiate —
// verify the gate can't accidentally be flipped off and silently block
// swaps. (Defense-in-depth against future changes.)
#[test]
fn swap_rejected_if_threshold_flag_toggled_off() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    // Flip the flag off artificially — not something that should ever
    // happen in real flow, but we want the ShortOfThreshold error
    // surface to stay honest.
    IS_THRESHOLD_HIT.save(&mut deps.storage, &false).unwrap();

    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    let hook = Cw20HookMsg::Swap {
        belief_price: None,
        max_spread: None,
        allow_high_max_spread: None,
        to: None,
        transaction_deadline: None,
    };
    let cw20_msg = Cw20ReceiveMsg {
        sender: trader.to_string(),
        amount: Uint128::new(10_000),
        msg: to_json_binary(&hook).unwrap(),
    };
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&addrs.creator_token, &[]),
        ExecuteMsg::Receive(cw20_msg),
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::ShortOfThreshold {}));
}

#[test]
fn swap_updates_reserves_per_constant_product() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed_pool(&mut deps, &addrs.pool_owner);

    let reserves_before = POOL_STATE.load(&deps.storage).unwrap();
    let trader = cosmwasm_std::testing::MockApi::default().addr_make("trader");
    let funds = vec![Coin::new(100_000u128, BLUECHIP_DENOM)];
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&trader, &funds),
        ExecuteMsg::SimpleSwap {
            offer_asset: TokenInfo {
                info: TokenType::Native {
                    denom: BLUECHIP_DENOM.to_string(),
                },
                amount: Uint128::new(100_000),
            },
            belief_price: None,
            max_spread: None,
            allow_high_max_spread: None,
            to: None,
            transaction_deadline: None,
        },
    )
    .unwrap();

    let reserves_after = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(
        reserves_after.reserve0,
        reserves_before.reserve0 + Uint128::new(100_000),
        "native reserve grew by the offer amount"
    );
    assert!(
        reserves_after.reserve1 < reserves_before.reserve1,
        "cw20 reserve decreased"
    );
}
