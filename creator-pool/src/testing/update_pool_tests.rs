use cosmwasm_std::{
    testing::{message_info, mock_dependencies, mock_env},
    Addr, Decimal, Uint128,
};

use crate::{
    contract::execute,
    error::ContractError,
    msg::{ExecuteMsg, PoolConfigUpdate},
    state::{COMMIT_LIMIT_INFO, MAX_MIN_COMMIT_USD},
    testing::liquidity_tests::setup_pool_storage,
};

#[test]
fn test_pool_update_config_from_factory() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    // Only factory can update
    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let update = PoolConfigUpdate {
        lp_fee: Some(Decimal::permille(3)),   // 0.3% LP fee
        min_commit_interval: Some(120),       // 2 minutes between commits
        ..Default::default()
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
        ..Default::default()
    };

    let hacker = message_info(&Addr::unchecked("hacker"), &[]);
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

/// Audit b8b0bcb: per-pool commit floors are tunable but must be > 0 and
/// <= `MAX_MIN_COMMIT_USD`. The apply-side guard rejects zero on
/// `min_commit_usd_pre_threshold` (defense-in-depth — factory's
/// `PoolConfigUpdate::validate()` rejects the same at propose time, but
/// the pool must not trust a stale or migrated `PendingPoolConfig`).
#[test]
fn test_update_config_rejects_zero_pre_threshold_floor() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let update = PoolConfigUpdate {
        min_commit_usd_pre_threshold: Some(Uint128::zero()),
        ..Default::default()
    };

    let err = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap_err();

    match err {
        ContractError::InvalidCommitFloor { field, got, max } => {
            assert_eq!(field, "min_commit_usd_pre_threshold");
            assert_eq!(got, Uint128::zero());
            assert_eq!(max, MAX_MIN_COMMIT_USD);
        }
        other => panic!("expected InvalidCommitFloor, got {:?}", other),
    }
}

#[test]
fn test_update_config_rejects_zero_post_threshold_floor() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let update = PoolConfigUpdate {
        min_commit_usd_post_threshold: Some(Uint128::zero()),
        ..Default::default()
    };

    let err = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap_err();

    match err {
        ContractError::InvalidCommitFloor { field, got, max } => {
            assert_eq!(field, "min_commit_usd_post_threshold");
            assert_eq!(got, Uint128::zero());
            assert_eq!(max, MAX_MIN_COMMIT_USD);
        }
        other => panic!("expected InvalidCommitFloor, got {:?}", other),
    }
}

#[test]
fn test_update_config_rejects_pre_floor_above_max() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let too_high = MAX_MIN_COMMIT_USD + Uint128::one();
    let update = PoolConfigUpdate {
        min_commit_usd_pre_threshold: Some(too_high),
        ..Default::default()
    };

    let err = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap_err();

    match err {
        ContractError::InvalidCommitFloor { field, got, max } => {
            assert_eq!(field, "min_commit_usd_pre_threshold");
            assert_eq!(got, too_high);
            assert_eq!(max, MAX_MIN_COMMIT_USD);
        }
        other => panic!("expected InvalidCommitFloor, got {:?}", other),
    }
}

#[test]
fn test_update_config_rejects_post_floor_above_max() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let too_high = MAX_MIN_COMMIT_USD + Uint128::one();
    let update = PoolConfigUpdate {
        min_commit_usd_post_threshold: Some(too_high),
        ..Default::default()
    };

    let err = execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap_err();

    match err {
        ContractError::InvalidCommitFloor { field, got, max } => {
            assert_eq!(field, "min_commit_usd_post_threshold");
            assert_eq!(got, too_high);
            assert_eq!(max, MAX_MIN_COMMIT_USD);
        }
        other => panic!("expected InvalidCommitFloor, got {:?}", other),
    }
}

/// Positive boundary: a value equal to `MAX_MIN_COMMIT_USD` must succeed
/// and persist on both fields simultaneously.
#[test]
fn test_update_config_accepts_floors_at_max_boundary() {
    let mut deps = mock_dependencies();
    setup_pool_storage(&mut deps);

    let factory_info = message_info(&Addr::unchecked("factory_contract"), &[]);
    let update = PoolConfigUpdate {
        min_commit_usd_pre_threshold: Some(MAX_MIN_COMMIT_USD),
        min_commit_usd_post_threshold: Some(MAX_MIN_COMMIT_USD),
        ..Default::default()
    };

    execute(
        deps.as_mut(),
        mock_env(),
        factory_info,
        ExecuteMsg::UpdateConfigFromFactory { update },
    )
    .unwrap();

    let stored = COMMIT_LIMIT_INFO.load(&deps.storage).unwrap();
    assert_eq!(stored.min_commit_usd_pre_threshold, MAX_MIN_COMMIT_USD);
    assert_eq!(stored.min_commit_usd_post_threshold, MAX_MIN_COMMIT_USD);
}
