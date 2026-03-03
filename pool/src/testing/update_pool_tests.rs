use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info},
    Decimal,
};

use crate::{
    contract::execute,
    error::ContractError,
    msg::{ExecuteMsg, PoolConfigUpdate},
    testing::liquidity_tests::setup_pool_storage,
};

#[test]
fn test_pool_update_config_from_factory() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Only factory can update
    let factory_info = mock_info("factory_contract", &[]);
    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::permille(3)),   // 0.3% LP fee
        min_commit_interval: Some(120),       // 2 minutes between commits
        usd_payment_tolerance_bps: Some(100), // 1% tolerance (100 basis points)
        oracle_address: None,
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
        lp_fee: Some(Decimal::permille(3)),   // 0.3% LP fee
        min_commit_interval: Some(120),       // 2 minutes between commits
        usd_payment_tolerance_bps: Some(100), // 1% tolerance (100 basis points)
        oracle_address: None,
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
