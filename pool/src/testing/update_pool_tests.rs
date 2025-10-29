use cosmwasm_std::{
    Addr, Decimal, Uint128, testing::{mock_dependencies, mock_env, mock_info}
};
use cw2::set_contract_version;

use crate::{
    contract::{execute, migrate},
    error::ContractError,
    msg::{CommitFeeInfo, ExecuteMsg, MigrateMsg, PoolConfigUpdate},
    state::POOL_SPECS, testing::liquidity_tests::setup_pool_storage,
};

#[test]
fn test_pool_migration_from_factory() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Set older version
    set_contract_version(&mut deps.storage, "crates.io:bluechip-pool", "1.0.0").unwrap();

    // Factory sends migration message
    let msg = MigrateMsg::UpdateFees {
        new_fees: Decimal::percent(5),
    };

    let res = migrate(deps.as_mut(), mock_env(), msg).unwrap();

    // Verify fee updated
    let pool_specs = POOL_SPECS.load(&deps.storage).unwrap();
    assert_eq!(pool_specs.lp_fee, Decimal::percent(5));
}

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
        threshold_payout: None, // Usually don't change this after creation
        cw20_token_contract_id: None, // Can't change after pool created
        cw721_nft_contract_id: None, // Can't change after pool created
        lp_fee: Some(Decimal::permille(3)), // 0.3% LP fee
        min_commit_interval: 120, // 2 minutes between commits
        usd_payment_tolerance_bps: 100, // 1% tolerance (100 basis points)
    };

    let res = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap();

    // Response has no `is_ok()`; unwrap() above already panicked on error.
    // Check response contents instead (for example, no outgoing messages).
    assert!(res.messages.is_empty());

    // Non-factory should fail: recreate the update (the previous `update` was moved).
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
        threshold_payout: None, // Usually don't change this after creation
        cw20_token_contract_id: None, // Can't change after pool created
        cw721_nft_contract_id: None, // Can't change after pool created
        lp_fee: Some(Decimal::permille(3)), // 0.3% LP fee
        min_commit_interval: 120, // 2 minutes between commits
        usd_payment_tolerance_bps: 100, // 1% tolerance (100 basis points)
    };

    let hacker = mock_info("hacker", &[]);
    let err = execute(
        deps.as_mut(),
        mock_env(),
        hacker,
        ExecuteMsg::UpdateConfigFromFactory { update: update_for_hacker },
    )
    .unwrap_err();

    assert!(matches!(err, ContractError::Unauthorized {}));
}
