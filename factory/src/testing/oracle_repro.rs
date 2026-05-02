use crate::asset::TokenType;
use crate::internal_bluechip_price_oracle::{
    calculate_weighted_price_with_atom, PoolCumulativeSnapshot,
};
use crate::mock_querier::mock_dependencies;
use crate::pool_struct::PoolDetails;
use crate::state::{
    FactoryInstantiate, FACTORYINSTANTIATEINFO, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID,
};
use cosmwasm_std::testing::MockApi;
use cosmwasm_std::{Addr, Decimal, Uint128};
use pool_factory_interfaces::PoolStateResponseForFactory;

fn make_addr(label: &str) -> Addr {
    MockApi::default().addr_make(label)
}

#[test]
fn test_repro_token_sort_order_bug() {
    let mut deps = mock_dependencies(&[]);

    let atom_pool = make_addr("pool_atom_bluechip");

    // Setup Factory Config
    let config = FactoryInstantiate {
        factory_admin_address: make_addr("admin"),
        cw721_nft_contract_id: 1,
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth".to_string(),
        pyth_atom_usd_price_feed_id: "id".to_string(),
        cw20_token_contract_id: 1,
        create_pool_wasm_contract_id: 1,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: make_addr("wallet"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(1),
        max_bluechip_lock_per_pool: Uint128::zero(),
        creator_excess_liquidity_lock_days: 0,
        atom_bluechip_anchor_pool_address: atom_pool.clone(),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: cosmwasm_std::Uint128::new(1_000_000),
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Register pool in POOLS_BY_ID so the oracle can determine token ordering.
    // Pool 1: Bluechip is asset[0], CreatorToken (ATOM) is asset[1]
    let pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::Native {
                denom: "BC".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("atom_addr_123"),
            },
        ],
        creator_pool_addr: atom_pool.clone(),
        pool_kind: pool_factory_interfaces::PoolKind::Commit,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID
        .save(deps.as_mut().storage, 1, &pool_details)
        .unwrap();

    // Setup ATOM Pool. Cumulative price1 = (reserve0/reserve1) × time = 1 × 100 = 100,
    // so a TWAP over the next 100s yields 1.0 (1e6 precision). For the
    // inverted-order test below we'll repoint reserves and price0_cumulative
    // to exercise the is_bluechip_second branch.
    let atom_pool_state = PoolStateResponseForFactory {
        pool_contract_address: atom_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(100_000_000_000),
        reserve1: Uint128::new(100_000_000_000),
        total_liquidity: Uint128::new(200_000_000_000),
        block_time_last: 100,
        price0_cumulative_last: Uint128::new(100),
        price1_cumulative_last: Uint128::new(100),
        assets: vec!["BC".to_string(), "atom_addr_123".to_string()],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, atom_pool.clone(), &atom_pool_state)
        .unwrap();

    // Prior snapshot at t=0, cumulative=0 — yields cumulative_delta=100 over
    // time_delta=100, i.e. TWAP = 100 × 1e6 / 100 = 1_000_000.
    let prev_snapshots = vec![PoolCumulativeSnapshot {
        pool_address: atom_pool.to_string(),
        price0_cumulative: Uint128::zero(),
        block_time: 0,
    }];

    // Calculate Price - Expected 1.0 (1_000_000 precision). The function
    // returns Option<Uint128> for prices; unwrap. Anchor is canonical
    // (BC at index 0), so anchor_bluechip_index = 0.
    let pools = vec![atom_pool.to_string()];
    let (price, _, _) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pools, &prev_snapshots, 0).unwrap();
    let price = price.expect("anchor TWAP must be Some when cumulative advanced");
    assert_eq!(
        price.u128(),
        1_000_000,
        "Price should be 1.0 when reserves are equal"
    );

    // Simulate "Inverted" Pool
    // reserve0 = ATOM (200B), reserve1 = Bluechip (100B).
    // Update POOLS_BY_ID to reflect the inverted token order.
    let inverted_pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("atom_addr_123"),
            },
            TokenType::Native {
                denom: "BC".to_string(),
            },
        ],
        creator_pool_addr: atom_pool.clone(),
        pool_kind: pool_factory_interfaces::PoolKind::Commit,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID
        .save(deps.as_mut().storage, 1, &inverted_pool_details)
        .unwrap();

    // For inverted shape, oracle reads price0_cumulative_last (because
    // is_bluechip_second = true). Set it to 50 over a 100s window to yield
    // bluechip-per-atom TWAP of 0.5 (500_000 in 1e6 precision), matching
    // the 200B atom : 100B bluechip reserve ratio.
    let inverted_state = PoolStateResponseForFactory {
        pool_contract_address: atom_pool.clone(),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(200_000_000_000),
        reserve1: Uint128::new(100_000_000_000),
        total_liquidity: Uint128::new(300_000_000_000),
        block_time_last: 100,
        price0_cumulative_last: Uint128::new(50),
        price1_cumulative_last: Uint128::new(200),
        assets: vec!["atom_addr_123".to_string(), "BC".to_string()],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(deps.as_mut().storage, atom_pool.clone(), &inverted_state)
        .unwrap();

    // Inverted shape: BC is now at index 1, so anchor_bluechip_index = 1.
    // After the audit refactor the resolution is no longer at-query — it's
    // pinned on `BlueChipPriceInternalOracle.anchor_bluechip_index` at the
    // moment the anchor is set/changed. This test simulates an admin
    // who has set the index correctly for the inverted pool shape.
    let (price_inverted, _, _) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pools, &prev_snapshots, 1).unwrap();
    let price_inverted =
        price_inverted.expect("inverted-shape anchor TWAP must be Some when cumulative advanced");
    assert_eq!(
        price_inverted.u128(),
        500_000,
        "Oracle should correctly handle inverted token order"
    );
}
