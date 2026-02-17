/// Tests for pool contract query handlers.
///
/// Coverage:
/// - Simulation query (swap preview)
/// - ReverseSimulation query
/// - PositionsByOwner query (H-5 audit fix)
/// - PoolInfo combined query
/// - FeeState query
/// - FeeInfo query
/// - CommitingInfo query
/// - LastCommited query
/// - IsFullyCommited query (CommitStatus)

use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage},
    from_json, to_json_binary, Addr, Coin, Decimal, OwnedDeps, Timestamp, Uint128,
};
use std::str::FromStr;

use crate::asset::{TokenInfo, TokenType};
use crate::mock_querier;
use crate::msg::{
    CommitStatus, CumulativePricesResponse, FeeInfoResponse, LastCommitedResponse,
    PoolFeeStateResponse, PoolInfoResponse, PoolStateResponse, PositionResponse,
    PositionsResponse, QueryMsg, ReverseSimulationResponse, SimulationResponse,
};
use crate::query::query;
use crate::state::{
    Commiting, COMMITFEEINFO, COMMIT_INFO, COMMIT_LIMIT_INFO, CommitLimitInfo,
    IS_THRESHOLD_HIT, NEXT_POSITION_ID, OWNER_POSITIONS, POOL_FEE_STATE, POOL_INFO,
    POOL_SPECS, POOL_STATE, PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, PoolState,
    USD_RAISED_FROM_COMMIT,
};
use crate::testing::liquidity_tests::{create_test_position, setup_pool_post_threshold, setup_pool_storage};

/// Setup pool storage on the custom mock querier that supports simulation queries.
/// Simulation queries call `query_pools()` which needs bank balance + CW20 balance queries.
fn setup_pool_with_querier() -> OwnedDeps<MockStorage, MockApi, mock_querier::WasmMockQuerier> {
    let mut deps = mock_querier::mock_dependencies(&[
        Coin { denom: "ubluechip".to_string(), amount: Uint128::new(23_500_000_000) },
    ]);

    // Reuse setup_pool_post_threshold logic but on custom querier deps
    use crate::asset::PoolPairType;
    use crate::state::*;
    use crate::msg::CommitFeeInfo;

    let pool_info = PoolInfo {
        pool_id: 1u64,
        pool_info: PoolDetails {
            asset_infos: [
                TokenType::Bluechip { denom: "ubluechip".to_string() },
                TokenType::CreatorToken { contract_addr: Addr::unchecked("token_contract") },
            ],
            contract_addr: Addr::unchecked(cosmwasm_std::testing::MOCK_CONTRACT_ADDR),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: Addr::unchecked("factory"),
        token_address: Addr::unchecked("token_contract"),
        position_nft_address: Addr::unchecked("nft_contract"),
    };
    POOL_INFO.save(&mut deps.storage, &pool_info).unwrap();

    let pool_state = PoolState {
        pool_contract_address: Addr::unchecked(cosmwasm_std::testing::MOCK_CONTRACT_ADDR),
        nft_ownership_accepted: true,
        reserve0: Uint128::new(23_500_000_000),
        reserve1: Uint128::new(350_000_000_000),
        total_liquidity: Uint128::new(91_104_335_791),
        block_time_last: 1_600_000_000,
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
    };
    POOL_STATE.save(&mut deps.storage, &pool_state).unwrap();

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    };
    POOL_FEE_STATE.save(&mut deps.storage, &pool_fee_state).unwrap();

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::percent(3) / Uint128::new(10),
        min_commit_interval: 60,
        usd_payment_tolerance_bps: 100,
    };
    POOL_SPECS.save(&mut deps.storage, &pool_specs).unwrap();

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold: Uint128::new(100_000_000),
        commit_amount_for_threshold_usd: Uint128::new(25_000_000_000),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 7,
    };
    COMMIT_LIMIT_INFO.save(&mut deps.storage, &commit_config).unwrap();

    let commit_fee_info = CommitFeeInfo {
        bluechip_wallet_address: Addr::unchecked("bluechip_treasury"),
        creator_wallet_address: Addr::unchecked("creator_wallet"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
    };
    COMMITFEEINFO.save(&mut deps.storage, &commit_fee_info).unwrap();

    IS_THRESHOLD_HIT.save(&mut deps.storage, &true).unwrap();
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(25_000_000_000)).unwrap();
    NEXT_POSITION_ID.save(&mut deps.storage, &1u64).unwrap();

    // Seed CW20 balances so query_pools() works
    deps.querier.with_token_balances(&[(
        &"token_contract".to_string(),
        &[(&cosmwasm_std::testing::MOCK_CONTRACT_ADDR.to_string(), &Uint128::new(350_000_000_000))],
    )]);

    deps
}

// ============================================================================
// Simulation Query
// ============================================================================

#[test]
fn test_query_simulation_bluechip_to_token() {
    let deps = setup_pool_with_querier();

    let env = mock_env();

    // Simulate swapping 1k bluechip for creator tokens
    let offer = TokenInfo {
        info: TokenType::Bluechip { denom: "ubluechip".to_string() },
        amount: Uint128::new(1_000_000_000),
    };

    let msg = QueryMsg::Simulation { offer_asset: offer };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let sim: SimulationResponse = from_json(&res).unwrap();

    // With 23.5k bluechip and 350k token reserves:
    // return_amount should be positive
    assert!(sim.return_amount > Uint128::zero(), "return_amount should be > 0");
    // spread should exist
    assert!(sim.spread_amount > Uint128::zero(), "spread_amount should be > 0");
    // commission should exist (0.3% fee)
    assert!(sim.commission_amount > Uint128::zero(), "commission_amount should be > 0");
    // return_amount + spread + commission should approximate the "ideal" swap output
    let total = sim.return_amount + sim.spread_amount + sim.commission_amount;
    assert!(total > Uint128::zero());
}

#[test]
fn test_query_simulation_token_to_bluechip() {
    let deps = setup_pool_with_querier();

    let env = mock_env();

    // Simulate swapping creator tokens for bluechip
    let offer = TokenInfo {
        info: TokenType::CreatorToken { contract_addr: Addr::unchecked("token_contract") },
        amount: Uint128::new(10_000_000_000), // 10k tokens
    };

    let msg = QueryMsg::Simulation { offer_asset: offer };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let sim: SimulationResponse = from_json(&res).unwrap();

    assert!(sim.return_amount > Uint128::zero());
    assert!(sim.commission_amount > Uint128::zero());
}

#[test]
fn test_query_simulation_wrong_asset() {
    let deps = setup_pool_with_querier();

    let env = mock_env();

    // Unknown asset should fail
    let offer = TokenInfo {
        info: TokenType::Bluechip { denom: "uatom".to_string() }, // wrong denom
        amount: Uint128::new(1_000_000_000),
    };

    let msg = QueryMsg::Simulation { offer_asset: offer };
    let err = query(deps.as_ref(), env, msg).unwrap_err();
    assert!(err.to_string().contains("does not belong"));
}

// ============================================================================
// ReverseSimulation Query
// ============================================================================

#[test]
fn test_query_reverse_simulation() {
    let deps = setup_pool_with_querier();

    let env = mock_env();

    // "I want 5k creator tokens, how much bluechip do I need?"
    let ask = TokenInfo {
        info: TokenType::CreatorToken { contract_addr: Addr::unchecked("token_contract") },
        amount: Uint128::new(5_000_000_000),
    };

    let msg = QueryMsg::ReverseSimulation { ask_asset: ask };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let rsim: ReverseSimulationResponse = from_json(&res).unwrap();

    assert!(rsim.offer_amount > Uint128::zero(), "offer_amount should be > 0");
    assert!(rsim.spread_amount > Uint128::zero(), "spread_amount should be > 0");
    assert!(rsim.commission_amount > Uint128::zero(), "commission_amount should be > 0");
}

// ============================================================================
// PositionsByOwner Query (H-5 audit optimization)
// ============================================================================

#[test]
fn test_query_positions_by_owner() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Create positions for different owners
    create_test_position(&mut deps, 1, "alice", Uint128::new(1_000_000));
    create_test_position(&mut deps, 2, "bob", Uint128::new(2_000_000));
    create_test_position(&mut deps, 3, "alice", Uint128::new(3_000_000));
    create_test_position(&mut deps, 4, "charlie", Uint128::new(500_000));

    // Register in OWNER_POSITIONS index (H-5 secondary index)
    let alice = Addr::unchecked("alice");
    let bob = Addr::unchecked("bob");
    let charlie = Addr::unchecked("charlie");
    OWNER_POSITIONS.save(&mut deps.storage, (&alice, "1"), &true).unwrap();
    OWNER_POSITIONS.save(&mut deps.storage, (&bob, "2"), &true).unwrap();
    OWNER_POSITIONS.save(&mut deps.storage, (&alice, "3"), &true).unwrap();
    OWNER_POSITIONS.save(&mut deps.storage, (&charlie, "4"), &true).unwrap();

    let env = mock_env();

    // Query Alice's positions
    let msg = QueryMsg::PositionsByOwner {
        owner: "alice".to_string(),
        start_after: None,
        limit: None,
    };
    let res = query(deps.as_ref(), env.clone(), msg).unwrap();
    let positions: PositionsResponse = from_json(&res).unwrap();

    assert_eq!(positions.positions.len(), 2, "Alice should have 2 positions");

    // Verify both are Alice's
    for pos in &positions.positions {
        assert_eq!(pos.owner, Addr::unchecked("alice"));
    }

    // Query Bob's positions
    let msg = QueryMsg::PositionsByOwner {
        owner: "bob".to_string(),
        start_after: None,
        limit: None,
    };
    let res = query(deps.as_ref(), env.clone(), msg).unwrap();
    let positions: PositionsResponse = from_json(&res).unwrap();

    assert_eq!(positions.positions.len(), 1, "Bob should have 1 position");
    assert_eq!(positions.positions[0].owner, Addr::unchecked("bob"));
}

#[test]
fn test_query_positions_by_owner_empty() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();

    // Query for user with no positions
    let msg = QueryMsg::PositionsByOwner {
        owner: "nobody".to_string(),
        start_after: None,
        limit: None,
    };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let positions: PositionsResponse = from_json(&res).unwrap();

    assert_eq!(positions.positions.len(), 0);
}

#[test]
fn test_query_positions_by_owner_pagination() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let alice = Addr::unchecked("alice");

    // Create 5 positions for Alice
    for i in 1..=5 {
        create_test_position(&mut deps, i, "alice", Uint128::new(1_000_000));
        OWNER_POSITIONS.save(&mut deps.storage, (&alice, &i.to_string()), &true).unwrap();
    }

    let env = mock_env();

    // Get first 2
    let msg = QueryMsg::PositionsByOwner {
        owner: "alice".to_string(),
        start_after: None,
        limit: Some(2),
    };
    let res = query(deps.as_ref(), env.clone(), msg).unwrap();
    let page1: PositionsResponse = from_json(&res).unwrap();
    assert_eq!(page1.positions.len(), 2);

    // Get next page starting after the last position ID from page 1
    let last_id = &page1.positions.last().unwrap().position_id;
    let msg = QueryMsg::PositionsByOwner {
        owner: "alice".to_string(),
        start_after: Some(last_id.clone()),
        limit: Some(2),
    };
    let res = query(deps.as_ref(), env.clone(), msg).unwrap();
    let page2: PositionsResponse = from_json(&res).unwrap();
    assert_eq!(page2.positions.len(), 2);

    // Verify no overlap between pages
    let page1_ids: Vec<_> = page1.positions.iter().map(|p| &p.position_id).collect();
    let page2_ids: Vec<_> = page2.positions.iter().map(|p| &p.position_id).collect();
    for id in &page2_ids {
        assert!(!page1_ids.contains(id), "Pages should not overlap");
    }
}

// ============================================================================
// PoolInfo Combined Query
// ============================================================================

#[test]
fn test_query_pool_info() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);
    NEXT_POSITION_ID.save(&mut deps.storage, &5u64).unwrap();

    let env = mock_env();
    let msg = QueryMsg::PoolInfo {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let info: PoolInfoResponse = from_json(&res).unwrap();

    assert_eq!(info.pool_state.reserve0, Uint128::new(23_500_000_000));
    assert_eq!(info.pool_state.reserve1, Uint128::new(350_000_000_000));
    assert!(info.pool_state.nft_ownership_accepted);
    assert_eq!(info.total_positions, 5);

    // Fee state should be initialized at zero
    assert_eq!(info.fee_state.fee_growth_global_0, Decimal::zero());
    assert_eq!(info.fee_state.total_fees_collected_0, Uint128::zero());
}

// ============================================================================
// FeeState Query
// ============================================================================

#[test]
fn test_query_fee_state() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    // Inject some fee data
    let mut fee_state = POOL_FEE_STATE.load(&deps.storage).unwrap();
    fee_state.fee_growth_global_0 = Decimal::from_str("12.5").unwrap();
    fee_state.fee_growth_global_1 = Decimal::from_str("0.75").unwrap();
    fee_state.total_fees_collected_0 = Uint128::new(500_000);
    fee_state.total_fees_collected_1 = Uint128::new(750_000);
    POOL_FEE_STATE.save(&mut deps.storage, &fee_state).unwrap();

    let env = mock_env();
    let msg = QueryMsg::FeeState {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let resp: PoolFeeStateResponse = from_json(&res).unwrap();

    assert_eq!(resp.fee_growth_global_0, Decimal::from_str("12.5").unwrap());
    assert_eq!(resp.fee_growth_global_1, Decimal::from_str("0.75").unwrap());
    assert_eq!(resp.total_fees_collected_0, Uint128::new(500_000));
    assert_eq!(resp.total_fees_collected_1, Uint128::new(750_000));
}

// ============================================================================
// FeeInfo Query
// ============================================================================

#[test]
fn test_query_fee_info() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = QueryMsg::FeeInfo {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let resp: FeeInfoResponse = from_json(&res).unwrap();

    assert_eq!(resp.fee_info.commit_fee_bluechip, Decimal::percent(1));
    assert_eq!(resp.fee_info.commit_fee_creator, Decimal::percent(5));
    assert_eq!(resp.fee_info.bluechip_wallet_address, Addr::unchecked("bluechip_treasury"));
    assert_eq!(resp.fee_info.creator_wallet_address, Addr::unchecked("creator_wallet"));
}

// ============================================================================
// CommitingInfo Query
// ============================================================================

#[test]
fn test_query_commiting_info_exists() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let user = Addr::unchecked("committer1");
    COMMIT_INFO.save(&mut deps.storage, &user, &Commiting {
        pool_contract_address: Addr::unchecked("pool_contract"),
        commiter: user.clone(),
        total_paid_usd: Uint128::new(5_000_000_000),
        total_paid_bluechip: Uint128::new(5_000_000_000),
        last_commited: Timestamp::from_seconds(1_600_000_000),
        last_payment_bluechip: Uint128::new(1_000_000_000),
        last_payment_usd: Uint128::new(1_000_000_000),
    }).unwrap();

    let env = mock_env();
    let msg = QueryMsg::CommitingInfo { wallet: "committer1".to_string() };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let info: Option<Commiting> = from_json(&res).unwrap();

    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.total_paid_usd, Uint128::new(5_000_000_000));
    assert_eq!(info.total_paid_bluechip, Uint128::new(5_000_000_000));
}

#[test]
fn test_query_commiting_info_not_found() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let env = mock_env();
    let msg = QueryMsg::CommitingInfo { wallet: "nobody".to_string() };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let info: Option<Commiting> = from_json(&res).unwrap();

    assert!(info.is_none());
}

// ============================================================================
// LastCommited Query
// ============================================================================

#[test]
fn test_query_last_commited_exists() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let user = Addr::unchecked("committer1");
    COMMIT_INFO.save(&mut deps.storage, &user, &Commiting {
        pool_contract_address: Addr::unchecked("pool_contract"),
        commiter: user.clone(),
        total_paid_usd: Uint128::new(5_000_000_000),
        total_paid_bluechip: Uint128::new(5_000_000_000),
        last_commited: Timestamp::from_seconds(1_600_000_000),
        last_payment_bluechip: Uint128::new(1_000_000_000),
        last_payment_usd: Uint128::new(1_000_000_000),
    }).unwrap();

    let env = mock_env();
    let msg = QueryMsg::LastCommited { wallet: "committer1".to_string() };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let resp: LastCommitedResponse = from_json(&res).unwrap();

    assert!(resp.has_commited);
    assert_eq!(resp.last_commited, Some(Timestamp::from_seconds(1_600_000_000)));
    assert_eq!(resp.last_payment_bluechip, Some(Uint128::new(1_000_000_000)));
    assert_eq!(resp.last_payment_usd, Some(Uint128::new(1_000_000_000)));
}

#[test]
fn test_query_last_commited_not_found() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let env = mock_env();
    let msg = QueryMsg::LastCommited { wallet: "nobody".to_string() };
    let res = query(deps.as_ref(), env, msg).unwrap();
    let resp: LastCommitedResponse = from_json(&res).unwrap();

    assert!(!resp.has_commited);
    assert!(resp.last_commited.is_none());
    assert!(resp.last_payment_bluechip.is_none());
}

// ============================================================================
// IsFullyCommited (CommitStatus) Query
// ============================================================================

#[test]
fn test_query_is_fully_commited_in_progress() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Pool not yet at threshold
    USD_RAISED_FROM_COMMIT.save(&mut deps.storage, &Uint128::new(10_000_000_000)).unwrap();

    let env = mock_env();
    let msg = QueryMsg::IsFullyCommited {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let status: CommitStatus = from_json(&res).unwrap();

    match status {
        CommitStatus::InProgress { raised, target } => {
            assert_eq!(raised, Uint128::new(10_000_000_000));
            assert_eq!(target, Uint128::new(25_000_000_000));
        }
        CommitStatus::FullyCommitted => panic!("Should be InProgress"),
    }
}

#[test]
fn test_query_is_fully_commited_fully_committed() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = QueryMsg::IsFullyCommited {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let status: CommitStatus = from_json(&res).unwrap();

    assert!(matches!(status, CommitStatus::FullyCommitted));
}

// ============================================================================
// PoolState Query
// ============================================================================

#[test]
fn test_query_pool_state() {
    let mut deps = mock_dependencies();
    setup_pool_post_threshold(&mut deps);

    let env = mock_env();
    let msg = QueryMsg::PoolState {};
    let res = query(deps.as_ref(), env, msg).unwrap();
    let state: PoolStateResponse = from_json(&res).unwrap();

    assert_eq!(state.reserve0, Uint128::new(23_500_000_000));
    assert_eq!(state.reserve1, Uint128::new(350_000_000_000));
    assert!(state.nft_ownership_accepted);
    assert!(state.total_liquidity > Uint128::zero());
}
