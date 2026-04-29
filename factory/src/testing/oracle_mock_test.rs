#![cfg(feature = "mock")]
// Exercises the mock-feature oracle-update path: in mock builds,
// UpdateOraclePrice reads the current bluechip price directly from the
// configured mock oracle (keyed under "BLUECHIP_USD") and pays the keeper
// bounty, without touching any pool contracts.

use cosmwasm_std::testing::{message_info, mock_env, MockApi};
use cosmwasm_std::{BankMsg, Coin, CosmosMsg, Decimal, Uint128};

use crate::execute::instantiate;
use crate::internal_bluechip_price_oracle::{
    update_internal_oracle_price, INTERNAL_ORACLE,
};
use crate::mock_querier::mock_dependencies;
use crate::state::{FactoryInstantiate, ORACLE_BOUNTY_DENOM, ORACLE_UPDATE_BOUNTY_USD};

fn addr(label: &str) -> cosmwasm_std::Addr {
    MockApi::default().addr_make(label)
}

fn default_init() -> FactoryInstantiate {
    FactoryInstantiate {
        factory_admin_address: addr("admin"),
        cw721_nft_contract_id: 58,
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        // The factory queries this contract for BLUECHIP_USD in mock mode.
        // Address just has to be valid bech32; the mock querier handles the call.
        pyth_contract_addr_for_conversions: addr("mock_oracle").to_string(),
        pyth_atom_usd_price_feed_id: "ATOM_USD".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: addr("bluechip_wallet"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(1),
        creator_excess_liquidity_lock_days: 7,
        // Under mock, the anchor pool is not queried during UpdateOraclePrice.
        atom_bluechip_anchor_pool_address: addr("unused_anchor"),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    }
}

#[test]
fn mock_path_reads_price_and_pays_bounty_without_any_pool() {
    // Factory pre-funded with 10 bluechip.
    let mut deps = mock_dependencies(&[Coin {
        denom: ORACLE_BOUNTY_DENOM.to_string(),
        amount: Uint128::new(10_000_000),
    }]);

    // Wire the querier: BLUECHIP_USD = $1.00 (6dp).
    deps.querier.mock_bluechip_usd_price = Some(Uint128::new(1_000_000));

    let admin = addr("admin");
    let keeper = addr("keeper");

    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&admin, &[]),
        default_init(),
    )
    .unwrap();

    // Enable the $0.005 oracle bounty.
    ORACLE_UPDATE_BOUNTY_USD
        .save(deps.as_mut().storage, &Uint128::new(5_000))
        .unwrap();

    // Advance past the 5-min cooldown.
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(301);

    let res = update_internal_oracle_price(
        deps.as_mut(),
        env,
        message_info(&keeper, &[]),
    )
    .expect("mock update should succeed");

    // Price got persisted.
    let oracle = INTERNAL_ORACLE.load(&deps.storage).unwrap();
    assert_eq!(
        oracle.bluechip_price_cache.last_price,
        Uint128::new(1_000_000),
        "mock path should write price read from mock oracle"
    );

    // Exactly one BankMsg::Send to the keeper with the bounty.
    let bank_sends: Vec<&BankMsg> = res
        .messages
        .iter()
        .filter_map(|m| {
            if let CosmosMsg::Bank(b) = &m.msg {
                Some(b)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(bank_sends.len(), 1, "expected exactly one bounty BankMsg");
    match bank_sends[0] {
        BankMsg::Send { to_address, amount } => {
            assert_eq!(to_address, keeper.as_str());
            assert_eq!(amount.len(), 1);
            assert_eq!(amount[0].denom, ORACLE_BOUNTY_DENOM);
            // $0.005 at bluechip=$1.00 → 0.005 bluechip = 5000 ubluechip.
            assert_eq!(amount[0].amount, Uint128::new(5_000));
        }
        _ => panic!("expected BankMsg::Send"),
    }
}

#[test]
fn mock_path_enforces_cooldown() {
    let mut deps = mock_dependencies(&[Coin {
        denom: ORACLE_BOUNTY_DENOM.to_string(),
        amount: Uint128::new(10_000_000),
    }]);
    deps.querier.mock_bluechip_usd_price = Some(Uint128::new(1_000_000));

    let admin = addr("admin");
    let keeper = addr("keeper");
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&admin, &[]),
        default_init(),
    )
    .unwrap();

    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(301);
    update_internal_oracle_price(deps.as_mut(), env.clone(), message_info(&keeper, &[]))
        .expect("first call ok");

    // Second call immediately — must be UpdateTooSoon.
    let err = update_internal_oracle_price(deps.as_mut(), env, message_info(&keeper, &[]))
        .expect_err("second call should be rejected");
    match err {
        crate::error::ContractError::UpdateTooSoon { .. } => {}
        other => panic!("expected UpdateTooSoon, got {:?}", other),
    }
}
