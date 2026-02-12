use crate::asset::TokenType;
use crate::internal_bluechip_price_oracle::calculate_weighted_price_with_atom;
use crate::mock_querier::mock_dependencies;
use crate::pool_struct::PoolDetails;
use crate::state::{
    FactoryInstantiate, FACTORYINSTANTIATEINFO, POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID,
};
use cosmwasm_std::{Addr, Decimal, Uint128};
use pool_factory_interfaces::PoolStateResponseForFactory;

const ATOM_BLUECHIP_ANCHOR_POOL: &str = "pool_atom_bluechip";

#[test]
fn test_repro_token_sort_order_bug() {
    let mut deps = mock_dependencies(&[]);

    // Setup Factory Config
    let config = FactoryInstantiate {
        factory_admin_address: Addr::unchecked("admin"),
        cw721_nft_contract_id: 1,
        commit_amount_for_threshold_bluechip: Uint128::zero(),
        commit_threshold_limit_usd: Uint128::new(100),
        pyth_contract_addr_for_conversions: "pyth".to_string(),
        pyth_atom_usd_price_feed_id: "id".to_string(),
        cw20_token_contract_id: 1,
        create_pool_wasm_contract_id: 1,
        bluechip_wallet_address: Addr::unchecked("wallet"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(1),
        max_bluechip_lock_per_pool: Uint128::zero(),
        creator_excess_liquidity_lock_days: 0,
        atom_bluechip_anchor_pool_address: Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
        bluechip_mint_contract_address: None,
    };
    FACTORYINSTANTIATEINFO
        .save(deps.as_mut().storage, &config)
        .unwrap();

    // Register pool in POOLS_BY_ID so the oracle can determine token ordering.
    // Pool 1: Bluechip is asset[0], CreatorToken (ATOM) is asset[1]
    let pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::Bluechip {
                denom: "BC".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("atom_addr_123"),
            },
        ],
        creator_pool_addr: Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
    };
    POOLS_BY_ID
        .save(deps.as_mut().storage, 1, &pool_details)
        .unwrap();

    // Setup ATOM Pool
    let atom_pool_state = PoolStateResponseForFactory {
        pool_contract_address: Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(100_000_000_000),
        reserve1: Uint128::new(100_000_000_000),
        total_liquidity: Uint128::new(200_000_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec!["BC".to_string(), "atom_addr_123".to_string()],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
            &atom_pool_state,
        )
        .unwrap();

    // Calculate Price - Expected 1.0 (1_000_000 precision)
    let pools = vec![ATOM_BLUECHIP_ANCHOR_POOL.to_string()];
    let (price, _, _) = calculate_weighted_price_with_atom(deps.as_ref(), &pools, &[]).unwrap();
    assert_eq!(
        price.u128(),
        1_000_000,
        "Price should be 1.0 when reserves are equal"
    );

    // NOW: Simulate "Inverted" Pool
    // reserve0 = ATOM (200B), reserve1 = Bluechip (100B).
    // Update POOLS_BY_ID to reflect the inverted token order.
    let inverted_pool_details = PoolDetails {
        pool_id: 1,
        pool_token_info: [
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked("atom_addr_123"),
            },
            TokenType::Bluechip {
                denom: "BC".to_string(),
            },
        ],
        creator_pool_addr: Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
    };
    POOLS_BY_ID
        .save(deps.as_mut().storage, 1, &inverted_pool_details)
        .unwrap();

    let inverted_state = PoolStateResponseForFactory {
        pool_contract_address: Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(200_000_000_000),
        reserve1: Uint128::new(100_000_000_000),
        total_liquidity: Uint128::new(300_000_000_000),
        block_time_last: 0,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        assets: vec!["atom_addr_123".to_string(), "BC".to_string()],
    };
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            Addr::unchecked(ATOM_BLUECHIP_ANCHOR_POOL),
            &inverted_state,
        )
        .unwrap();

    let (price_inverted, _, _) =
        calculate_weighted_price_with_atom(deps.as_ref(), &pools, &[]).unwrap();

    // With the fix, the oracle looks up POOLS_BY_ID to determine that asset[0] is
    // CreatorToken => is_bluechip_second = true => bluechip is reserve1 (100B).
    // Price = 100 / 200 = 0.5 (500_000).
    assert_eq!(
        price_inverted.u128(),
        500_000,
        "Oracle should correctly handle inverted token order"
    );
}
