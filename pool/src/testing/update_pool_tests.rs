use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info},
    Addr, Decimal, Uint128,
};

use crate::{
    contract::{execute},
    error::ContractError,
    msg::{CommitFeeInfo, ExecuteMsg, PoolConfigUpdate},
    testing::liquidity_tests::setup_pool_storage,
};


#[test]
fn test_pool_update_config_from_factory() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Only factory can update
    let factory_info = mock_info("factory_contract", &[]);
    let update = PoolConfigUpdate {
        commit_fee_info: Some(CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("new_bluechip"),
            creator_wallet_address: Addr::unchecked("new_creator"),
            commit_fee_bluechip: Decimal::percent(2),
            commit_fee_creator: Decimal::percent(8),
        }),
        commit_limit_usd: Some(Uint128::new(30_000_000_000)), // 30k USD
        pyth_contract_addr_for_conversions: Some("new_oracle_addr".to_string()),
        pyth_atom_usd_price_feed_id: Some("new_feed_id".to_string()),
        commit_amount_for_threshold: Some(Uint128::new(30_000_000_000)), // 30k
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
        lp_fee: Some(Decimal::permille(3)), // 0.3% LP fee
        min_commit_interval: 120,           // 2 minutes between commits
        usd_payment_tolerance_bps: 100,     // 1% tolerance (100 basis points)
    };

    let res = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap();

    assert!(res.messages.is_empty());

    let update_for_hacker = PoolConfigUpdate {
        commit_fee_info: Some(CommitFeeInfo {
            bluechip_wallet_address: Addr::unchecked("new_bluechip"),
            creator_wallet_address: Addr::unchecked("new_creator"),
            commit_fee_bluechip: Decimal::percent(2),
            commit_fee_creator: Decimal::percent(8),
        }),
        commit_limit_usd: Some(Uint128::new(30_000_000_000)), // 30k USD
        pyth_contract_addr_for_conversions: Some("new_oracle_addr".to_string()),
        pyth_atom_usd_price_feed_id: Some("new_feed_id".to_string()),
        commit_amount_for_threshold: Some(Uint128::new(30_000_000_000)), // 30k
        threshold_payout: None,
        cw20_token_contract_id: None,
        cw721_nft_contract_id: None,
        lp_fee: Some(Decimal::permille(3)), // 0.3% LP fee
        min_commit_interval: 120,           // 2 minutes between commits
        usd_payment_tolerance_bps: 100,     // 1% tolerance (100 basis points)
    };

    let hacker = mock_info("hacker", &[]);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        hacker,
        ExecuteMsg::UpdateConfigFromFactory {
            update: update_for_hacker,
        },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::Unauthorized {}));
}
