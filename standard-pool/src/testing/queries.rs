//! Pool-core query coverage via standard-pool's query dispatch.

use cosmwasm_std::testing::{message_info, mock_env};
use cosmwasm_std::{from_json, Coin, Uint128};
use pool_core::asset::TokenType;
use pool_core::msg::{
    CommitStatus, ConfigResponse, FeeInfoResponse, PoolAnalyticsResponse, PoolFeeStateResponse,
    PoolStateResponse, PositionResponse, PositionsResponse,
};
use pool_core::state::PoolDetails;

use super::fixtures::{instantiate_default_pool, BLUECHIP_DENOM};
use crate::contract::execute;
use crate::msg::{ExecuteMsg, QueryMsg};
use crate::query::query;

fn seed(mut_deps: &mut cosmwasm_std::OwnedDeps<
    cosmwasm_std::testing::MockStorage,
    cosmwasm_std::testing::MockApi,
    cosmwasm_std::testing::MockQuerier,
>, user: &cosmwasm_std::Addr) {
    execute(
        mut_deps.as_mut(),
        mock_env(),
        message_info(user, &[Coin::new(1_000_000_000u128, BLUECHIP_DENOM)]),
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
fn query_pair_returns_pool_details() {
    let (deps, addrs) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::Pair {}).unwrap();
    let details: PoolDetails = from_json(&bin).unwrap();
    // contract_addr equals env.contract.address from instantiate, which
    // MockEnv.contract.address returns. Just verify it's non-empty rather
    // than pinning to a specific MockApi-internal value.
    assert!(!details.contract_addr.as_str().is_empty());
    assert!(matches!(
        details.asset_infos[0],
        TokenType::Native { .. }
    ));
    match &details.asset_infos[1] {
        TokenType::CreatorToken { contract_addr } => {
            assert_eq!(contract_addr, &addrs.creator_token)
        }
        _ => panic!("asset 1 should be CreatorToken"),
    }
}

#[test]
fn query_config_returns_block_time() {
    let (deps, _) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::Config {}).unwrap();
    let config: ConfigResponse = from_json(&bin).unwrap();
    assert!(config.block_time_last > 0);
    assert!(config.params.is_none());
}

#[test]
fn query_fee_info_returns_placeholder() {
    let (deps, addrs) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::FeeInfo {}).unwrap();
    let resp: FeeInfoResponse = from_json(&bin).unwrap();
    // Standard pools use zero fees with the factory as drain recipient.
    assert_eq!(resp.fee_info.bluechip_wallet_address, addrs.factory);
    assert_eq!(resp.fee_info.commit_fee_bluechip, cosmwasm_std::Decimal::zero());
}

#[test]
fn query_pool_state_empty_at_instantiate() {
    let (deps, _) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::PoolState {}).unwrap();
    let resp: PoolStateResponse = from_json(&bin).unwrap();
    assert_eq!(resp.reserve0, Uint128::zero());
    assert_eq!(resp.reserve1, Uint128::zero());
    assert_eq!(resp.total_liquidity, Uint128::zero());
    assert!(!resp.nft_ownership_accepted);
}

#[test]
fn query_fee_state_zero_at_instantiate() {
    let (deps, _) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::FeeState {}).unwrap();
    let resp: PoolFeeStateResponse = from_json(&bin).unwrap();
    assert_eq!(resp.fee_growth_global_0, cosmwasm_std::Decimal::zero());
    assert_eq!(resp.total_fees_collected_0, Uint128::zero());
}

#[test]
fn query_position_after_deposit() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed(&mut deps, &addrs.pool_owner);

    let bin = query(
        deps.as_ref(),
        mock_env(),
        QueryMsg::Position {
            position_id: "1".to_string(),
        },
    )
    .unwrap();
    let pos: PositionResponse = from_json(&bin).unwrap();
    assert_eq!(pos.position_id, "1");
    assert_eq!(pos.owner, addrs.pool_owner);
    assert!(!pos.liquidity.is_zero());
}

#[test]
fn query_positions_lists_deposited_positions() {
    let (mut deps, addrs) = instantiate_default_pool();
    seed(&mut deps, &addrs.pool_owner);

    let bin = query(
        deps.as_ref(),
        mock_env(),
        QueryMsg::Positions {
            start_after: None,
            limit: None,
        },
    )
    .unwrap();
    let resp: PositionsResponse = from_json(&bin).unwrap();
    // "0" is the instantiate placeholder; position "1" is the first real.
    assert!(resp.positions.iter().any(|p| p.position_id == "1"));
}

// Note: `query_simulation` exercises the same compute_swap math that's
// already unit-tested in pool-core::swap. A full simulation roundtrip
// requires mocking both bank balances and the CW20 BalanceOf query
// because pool-core::query::query_simulation reads live chain balances
// via `query_pools`, not pool_state reserves. Skipped here to keep the
// integration test infrastructure narrow; the math itself is covered.

#[test]
fn query_analytics_always_fully_committed() {
    let (deps, _) = instantiate_default_pool();
    let bin = query(deps.as_ref(), mock_env(), QueryMsg::Analytics {}).unwrap();
    let resp: PoolAnalyticsResponse = from_json(&bin).unwrap();
    // Standard pools report FullyCommitted + zero raised, regardless of
    // activity (no commit ledger).
    assert!(matches!(resp.threshold_status, CommitStatus::FullyCommitted));
    assert_eq!(resp.total_usd_raised, Uint128::zero());
    assert_eq!(resp.total_bluechip_raised, Uint128::zero());
}
