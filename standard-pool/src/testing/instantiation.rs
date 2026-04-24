//! Standard-pool `instantiate` tests — success path + every error
//! branch in `standard_pool::contract::instantiate`.

use cosmwasm_std::testing::{mock_dependencies, mock_env, MockApi};
use cosmwasm_std::{Addr, Decimal, MessageInfo, Uint128};
use pool_core::asset::TokenType;
use pool_core::state::{
    COMMITFEEINFO, IS_THRESHOLD_HIT, NEXT_POSITION_ID, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO,
    POOL_SPECS, POOL_STATE,
};

use super::fixtures::{
    fixture_addrs, instantiate_default_pool, mock_deps_with_nft_owner, standard_instantiate_msg,
    BLUECHIP_DENOM,
};
use crate::contract::instantiate;
use crate::error::ContractError;

// -- Success path --------------------------------------------------------

#[test]
fn instantiate_saves_all_required_state() {
    let (deps, addrs) = instantiate_default_pool();

    let info = POOL_INFO.load(&deps.storage).unwrap();
    assert_eq!(info.pool_id, 1);
    assert_eq!(info.factory_addr, addrs.factory);
    assert_eq!(info.position_nft_address, addrs.position_nft);
    assert_eq!(info.token_address, addrs.creator_token);

    // Option X: nft_ownership_accepted starts false; first deposit flips it.
    let state = POOL_STATE.load(&deps.storage).unwrap();
    assert_eq!(state.reserve0, Uint128::zero());
    assert_eq!(state.reserve1, Uint128::zero());
    assert_eq!(state.total_liquidity, Uint128::zero());
    assert!(!state.nft_ownership_accepted);

    let fees = POOL_FEE_STATE.load(&deps.storage).unwrap();
    assert_eq!(fees.fee_reserve_0, Uint128::zero());
    assert_eq!(fees.fee_reserve_1, Uint128::zero());

    let specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(specs.lp_fee, Decimal::permille(3));
    assert_eq!(specs.min_commit_interval, 13);

    let analytics = POOL_ANALYTICS.load(&deps.storage).unwrap();
    assert_eq!(analytics.total_swap_count, 0);

    assert_eq!(NEXT_POSITION_ID.load(&deps.storage).unwrap(), 0);

    // Standard pools are "post-threshold" from birth — swap / liquidity
    // handlers open to traffic immediately.
    assert!(IS_THRESHOLD_HIT.load(&deps.storage).unwrap());

    // COMMITFEEINFO seeded as zero-valued placeholder with the factory
    // as drain recipient (emergency_withdraw_core_drain reads
    // bluechip_wallet_address off this Item).
    let fee_info = COMMITFEEINFO.load(&deps.storage).unwrap();
    assert_eq!(fee_info.bluechip_wallet_address, addrs.factory);
    assert_eq!(fee_info.commit_fee_bluechip, Decimal::zero());
}

// -- Error paths ---------------------------------------------------------

#[test]
fn instantiate_rejects_non_factory_sender() {
    let addrs = fixture_addrs();
    let mut deps = mock_deps_with_nft_owner(addrs.pool_owner.clone(), addrs.position_nft.clone());
    let info = MessageInfo {
        sender: MockApi::default().addr_make("attacker"),
        funds: vec![],
    };
    let err =
        instantiate(deps.as_mut(), mock_env(), info, standard_instantiate_msg(&addrs)).unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

#[test]
fn instantiate_rejects_duplicate_asset_sides() {
    let addrs = fixture_addrs();
    let mut msg = standard_instantiate_msg(&addrs);
    msg.pool_token_info = [
        TokenType::Native {
            denom: BLUECHIP_DENOM.to_string(),
        },
        TokenType::Native {
            denom: BLUECHIP_DENOM.to_string(),
        },
    ];
    let mut deps = mock_deps_with_nft_owner(addrs.pool_owner.clone(), addrs.position_nft.clone());
    let info = MessageInfo {
        sender: addrs.factory.clone(),
        funds: vec![],
    };
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();
    assert!(matches!(err, ContractError::DoublingAssets {}));
}

#[test]
fn instantiate_rejects_empty_native_denom() {
    let addrs = fixture_addrs();
    let mut msg = standard_instantiate_msg(&addrs);
    msg.pool_token_info[0] = TokenType::Native {
        denom: "   ".to_string(),
    };
    let mut deps = mock_deps_with_nft_owner(addrs.pool_owner.clone(), addrs.position_nft.clone());
    let info = MessageInfo {
        sender: addrs.factory.clone(),
        funds: vec![],
    };
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();
    match err {
        ContractError::Std(e) => assert!(e.to_string().contains("Native denom must be non-empty")),
        other => panic!("expected Std error, got {:?}", other),
    }
}

#[test]
fn instantiate_rejects_invalid_cw20_address() {
    let addrs = fixture_addrs();
    let mut msg = standard_instantiate_msg(&addrs);
    msg.pool_token_info[1] = TokenType::CreatorToken {
        contract_addr: Addr::unchecked(""),
    };
    let mut deps = mock_deps_with_nft_owner(addrs.pool_owner.clone(), addrs.position_nft.clone());
    let info = MessageInfo {
        sender: addrs.factory.clone(),
        funds: vec![],
    };
    let err = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap_err();
    assert!(matches!(err, ContractError::Std(_)));
}

#[test]
fn instantiate_accepts_native_native_pair() {
    // Native+Native is the factory anchor-pool shape (ATOM/bluechip).
    let api = MockApi::default();
    let factory = api.addr_make("factory_contract");
    let nft = api.addr_make("nft_contract");
    let mut msg = standard_instantiate_msg(&fixture_addrs());
    msg.used_factory_addr = factory.clone();
    msg.position_nft_address = nft.clone();
    msg.pool_token_info = [
        TokenType::Native {
            denom: BLUECHIP_DENOM.to_string(),
        },
        TokenType::Native {
            denom: "uatom".to_string(),
        },
    ];
    let mut deps = mock_dependencies();
    let info = MessageInfo {
        sender: factory.clone(),
        funds: vec![],
    };
    instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

    // Token address placeholder falls back to the factory address since
    // no CreatorToken side exists.
    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    assert_eq!(pool_info.token_address, factory);
}

#[test]
fn instantiate_accepts_cw20_cw20_pair() {
    let api = MockApi::default();
    let factory = api.addr_make("factory_contract");
    let nft = api.addr_make("nft_contract");
    let token_a = api.addr_make("token_a");
    let token_b = api.addr_make("token_b");

    let mut msg = standard_instantiate_msg(&fixture_addrs());
    msg.used_factory_addr = factory.clone();
    msg.position_nft_address = nft;
    msg.pool_token_info = [
        TokenType::CreatorToken {
            contract_addr: token_a.clone(),
        },
        TokenType::CreatorToken {
            contract_addr: token_b,
        },
    ];
    let mut deps = mock_dependencies();
    let info = MessageInfo {
        sender: factory,
        funds: vec![],
    };
    instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

    // Token address placeholder picks the first CreatorToken side.
    let pool_info = POOL_INFO.load(&deps.storage).unwrap();
    assert_eq!(pool_info.token_address, token_a);
}
