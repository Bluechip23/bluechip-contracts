//! Coverage-gap tests for factory paths that had no Rust regression cover
//! prior to this file. Groups:
//!
//! - `must_pay` surplus refund on commit-pool `Create` (audit f944e07).
//! - `SetPythConfThresholdBps` bounds + auth (range
//!   `[PYTH_CONF_THRESHOLD_BPS_MIN, PYTH_CONF_THRESHOLD_BPS_MAX]` =
//!   `[50, 500]`, admin-only).
//! - Oracle-allowlist error variants that the timelock/cancel/remove
//!   tests in `oracle_eligibility_tests` don't currently fire:
//!   `OracleEligiblePoolAlreadyAdded`,
//!   `OracleEligiblePoolMissingBluechipSide`,
//!   `OracleEligiblePoolAddAlreadyPending`,
//!   `NoPendingOracleEligiblePoolAdd`,
//!   `OracleEligiblePoolNotAllowlisted`,
//!   `CommitPoolsAutoEligibleAlreadyPending`,
//!   `NoPendingCommitPoolsAutoEligible`.

use cosmwasm_std::testing::{message_info, mock_env, MockApi, MockStorage};
use cosmwasm_std::{Addr, BankMsg, Coin, CosmosMsg, Decimal, OwnedDeps, Uint128};

use crate::asset::TokenType;
use crate::error::ContractError;
use crate::execute::{execute, instantiate};
use crate::mock_querier::{mock_dependencies, WasmMockQuerier};
use crate::msg::{CreatorTokenInfo, ExecuteMsg};
use crate::pool_struct::{CreatePool, PoolDetails};
use crate::state::{
    load_pyth_conf_threshold_bps, FactoryInstantiate, ADMIN_TIMELOCK_SECONDS,
    COMMIT_POOLS_AUTO_ELIGIBLE, ORACLE_ELIGIBLE_POOLS, PENDING_COMMIT_POOLS_AUTO_ELIGIBLE,
    POOLS_BY_CONTRACT_ADDRESS, POOLS_BY_ID, POOL_COUNTER, PYTH_CONF_THRESHOLD_BPS,
    PYTH_CONF_THRESHOLD_BPS_MAX, PYTH_CONF_THRESHOLD_BPS_MIN,
};
use crate::testing::tests::setup_atom_pool;
use pool_factory_interfaces::PoolStateResponseForFactory;

// --- shared helpers --------------------------------------------------------

fn make_addr(label: &str) -> Addr {
    MockApi::default().addr_make(label)
}

fn admin() -> Addr {
    make_addr("admin")
}

fn default_factory_config() -> FactoryInstantiate {
    FactoryInstantiate {
        cw721_nft_contract_id: 58,
        factory_admin_address: admin(),
        commit_threshold_limit_usd: Uint128::new(25_000_000_000),
        pyth_contract_addr_for_conversions: make_addr("oracle0000").to_string(),
        pyth_atom_usd_price_feed_id: "ORCL".to_string(),
        cw20_token_contract_id: 10,
        create_pool_wasm_contract_id: 11,
        standard_pool_wasm_contract_id: 0,
        bluechip_wallet_address: make_addr("ubluechip"),
        commit_fee_bluechip: Decimal::percent(1),
        commit_fee_creator: Decimal::percent(5),
        max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
        creator_excess_liquidity_lock_days: 14,
        atom_bluechip_anchor_pool_address: make_addr("atom_bluechip_pool"),
        bluechip_mint_contract_address: None,
        bluechip_denom: "ubluechip".to_string(),
        atom_denom: "uatom".to_string(),
        standard_pool_creation_fee_usd: Uint128::new(1_000_000),
        threshold_payout_amounts: Default::default(),
        emergency_withdraw_delay_seconds: 86_400,
    }
}

fn fresh_factory() -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
    let mut deps = mock_dependencies(&[]);
    setup_atom_pool(&mut deps);
    instantiate(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        default_factory_config(),
    )
    .unwrap();
    deps
}

// ---------------------------------------------------------------------------
// must_pay surplus refund
// ---------------------------------------------------------------------------

/// Audit f944e07: commit-pool `Create` enforces `must_pay` on the bluechip
/// denom and the configured USD fee converted via the live oracle.
/// Bootstrap state (no oracle warm-up yet) falls back to
/// `STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP = 100_000_000 ubluechip`.
/// Overpaying that amount must produce a Bank `Send` refunding the surplus
/// to `info.sender` inside the same response.
#[test]
fn create_pool_refunds_surplus_to_sender() {
    let mut deps = fresh_factory();

    let required: u128 = 100_000_000;
    let surplus: u128 = 50_000_000;
    let paid = Uint128::new(required + surplus);

    let funds = vec![Coin {
        denom: "ubluechip".to_string(),
        amount: paid,
    }];
    let info = message_info(&admin(), &funds);

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
        },
        token_info: CreatorTokenInfo {
            name: "RefundToken".to_string(),
            symbol: "REFUND".to_string(),
            decimal: 6,
        },
    };

    let res = execute(deps.as_mut(), mock_env(), info, create_msg).unwrap();

    // Exactly one BankMsg::Send must address the sender with the surplus
    // amount of ubluechip. (The other potential BankMsg from this
    // response — the fee transfer — addresses the bluechip wallet, not
    // the sender.)
    let admin_addr_str = admin().to_string();
    let refund_match = res.messages.iter().find_map(|sub| match &sub.msg {
        CosmosMsg::Bank(BankMsg::Send { to_address, amount }) if to_address == &admin_addr_str => {
            amount
                .iter()
                .find(|c| c.denom == "ubluechip" && c.amount == Uint128::new(surplus))
                .map(|_| ())
        }
        _ => None,
    });
    assert!(
        refund_match.is_some(),
        "expected BankMsg::Send refunding {} ubluechip to {}, got {:?}",
        surplus,
        admin_addr_str,
        res.messages
    );
}

/// Negative complement: paying *exactly* `required_bluechip` must NOT
/// emit any BankMsg targeting `info.sender` — the surplus branch is
/// guarded on `!surplus.is_zero()`.
#[test]
fn create_pool_exact_pay_emits_no_refund() {
    let mut deps = fresh_factory();

    let funds = vec![Coin {
        denom: "ubluechip".to_string(),
        amount: Uint128::new(100_000_000),
    }];
    let info = message_info(&admin(), &funds);

    let create_msg = ExecuteMsg::Create {
        pool_msg: CreatePool {
            pool_token_info: [
                TokenType::Native {
                    denom: "ubluechip".to_string(),
                },
                TokenType::CreatorToken {
                    contract_addr: Addr::unchecked("WILL_BE_CREATED_BY_FACTORY"),
                },
            ],
        },
        token_info: CreatorTokenInfo {
            name: "ExactToken".to_string(),
            symbol: "EXACT".to_string(),
            decimal: 6,
        },
    };

    let res = execute(deps.as_mut(), mock_env(), info, create_msg).unwrap();

    let admin_addr_str = admin().to_string();
    let any_refund = res.messages.iter().any(|sub| {
        matches!(&sub.msg, CosmosMsg::Bank(BankMsg::Send { to_address, .. }) if to_address == &admin_addr_str)
    });
    assert!(
        !any_refund,
        "exact-pay create must not emit a refund BankMsg to sender; got {:?}",
        res.messages
    );
}

// ---------------------------------------------------------------------------
// SetPythConfThresholdBps bounds + auth
// ---------------------------------------------------------------------------

#[test]
fn set_pyth_conf_threshold_bps_rejects_below_min() {
    let mut deps = fresh_factory();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::SetPythConfThresholdBps {
            bps: PYTH_CONF_THRESHOLD_BPS_MIN - 1,
        },
    )
    .unwrap_err();
    match err {
        ContractError::Std(e) => {
            let s = e.to_string();
            assert!(
                s.contains("out of allowed range"),
                "expected range error, got {}",
                s
            );
        }
        other => panic!("expected Std range error, got {:?}", other),
    }
}

#[test]
fn set_pyth_conf_threshold_bps_rejects_above_max() {
    let mut deps = fresh_factory();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::SetPythConfThresholdBps {
            bps: PYTH_CONF_THRESHOLD_BPS_MAX + 1,
        },
    )
    .unwrap_err();
    match err {
        ContractError::Std(e) => {
            assert!(e.to_string().contains("out of allowed range"));
        }
        other => panic!("expected Std range error, got {:?}", other),
    }
}

#[test]
fn set_pyth_conf_threshold_bps_accepts_min_and_max_boundaries() {
    let mut deps = fresh_factory();

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::SetPythConfThresholdBps {
            bps: PYTH_CONF_THRESHOLD_BPS_MIN,
        },
    )
    .unwrap();
    assert_eq!(
        load_pyth_conf_threshold_bps(&deps.storage),
        PYTH_CONF_THRESHOLD_BPS_MIN
    );

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::SetPythConfThresholdBps {
            bps: PYTH_CONF_THRESHOLD_BPS_MAX,
        },
    )
    .unwrap();
    assert_eq!(
        PYTH_CONF_THRESHOLD_BPS.load(&deps.storage).unwrap(),
        PYTH_CONF_THRESHOLD_BPS_MAX
    );
}

#[test]
fn set_pyth_conf_threshold_bps_rejects_non_admin() {
    let mut deps = fresh_factory();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&make_addr("hacker"), &[]),
        ExecuteMsg::SetPythConfThresholdBps { bps: 200 },
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::Unauthorized {}));
}

// ---------------------------------------------------------------------------
// Oracle-allowlist error variants
// ---------------------------------------------------------------------------

/// Pool registration helper for the allowlist tests. Mirrors the
/// `register_standard_pool_with_reserves` helper in
/// `oracle_eligibility_tests` but keeps this file self-contained.
fn register_standard_pool_with_bluechip_side(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    pool_id: u64,
    addr: &Addr,
) {
    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: [
            TokenType::Native {
                denom: "ubluechip".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked(format!("creator_token_{}", pool_id)),
            },
        ],
        creator_pool_addr: addr.clone(),
        pool_kind: pool_factory_interfaces::PoolKind::Standard,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID.save(deps.as_mut().storage, pool_id, &pool_details).unwrap();
    crate::state::POOL_ID_BY_ADDRESS
        .save(deps.as_mut().storage, addr.clone(), &pool_id)
        .unwrap();
    let counter = POOL_COUNTER.may_load(deps.as_ref().storage).unwrap().unwrap_or(0);
    if pool_id > counter {
        POOL_COUNTER.save(deps.as_mut().storage, &pool_id).unwrap();
    }
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            addr.clone(),
            &PoolStateResponseForFactory {
                pool_contract_address: addr.clone(),
                nft_ownership_accepted: true,
                reserve0: Uint128::new(50_000_000_000),
                reserve1: Uint128::new(50_000_000_000),
                total_liquidity: Uint128::new(100_000_000_000),
                block_time_last: 100,
                price0_cumulative_last: Uint128::zero(),
                price1_cumulative_last: Uint128::zero(),
                assets: vec![],
            },
        )
        .unwrap();
}

/// Standard pool whose token-info has NO bluechip side. Used to fire
/// `OracleEligiblePoolMissingBluechipSide`.
fn register_standard_pool_no_bluechip(
    deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>,
    pool_id: u64,
    addr: &Addr,
) {
    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: [
            TokenType::Native {
                denom: "uatom".to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: Addr::unchecked(format!("creator_token_{}", pool_id)),
            },
        ],
        creator_pool_addr: addr.clone(),
        pool_kind: pool_factory_interfaces::PoolKind::Standard,
        commit_pool_ordinal: 0,
    };
    POOLS_BY_ID.save(deps.as_mut().storage, pool_id, &pool_details).unwrap();
    crate::state::POOL_ID_BY_ADDRESS
        .save(deps.as_mut().storage, addr.clone(), &pool_id)
        .unwrap();
    let counter = POOL_COUNTER.may_load(deps.as_ref().storage).unwrap().unwrap_or(0);
    if pool_id > counter {
        POOL_COUNTER.save(deps.as_mut().storage, &pool_id).unwrap();
    }
    POOLS_BY_CONTRACT_ADDRESS
        .save(
            deps.as_mut().storage,
            addr.clone(),
            &PoolStateResponseForFactory {
                pool_contract_address: addr.clone(),
                nft_ownership_accepted: true,
                reserve0: Uint128::new(50_000_000_000),
                reserve1: Uint128::new(50_000_000_000),
                total_liquidity: Uint128::new(100_000_000_000),
                block_time_last: 100,
                price0_cumulative_last: Uint128::zero(),
                price1_cumulative_last: Uint128::zero(),
                assets: vec![],
            },
        )
        .unwrap();
}

/// Run Propose + (after timelock) Apply for `pool_addr`. Leaves the pool
/// allowlisted and the pending slot cleared.
fn allowlist_pool(deps: &mut OwnedDeps<MockStorage, MockApi, WasmMockQuerier>, pool_addr: &Addr) {
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeAddOracleEligiblePool {
            pool_addr: pool_addr.to_string(),
        },
    )
    .unwrap();
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
    execute(
        deps.as_mut(),
        env,
        message_info(&admin(), &[]),
        ExecuteMsg::ApplyAddOracleEligiblePool {
            pool_addr: pool_addr.to_string(),
        },
    )
    .unwrap();
}

#[test]
fn propose_add_rejects_when_pool_already_allowlisted() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_usdc");
    register_standard_pool_with_bluechip_side(&mut deps, 2, &std_pool);
    allowlist_pool(&mut deps, &std_pool);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::OracleEligiblePoolAlreadyAdded { .. }
    ));
}

#[test]
fn propose_add_rejects_when_pool_has_no_bluechip_side() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_atom_only");
    register_standard_pool_no_bluechip(&mut deps, 2, &std_pool);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::OracleEligiblePoolMissingBluechipSide { .. }
    ));
}

#[test]
fn propose_add_rejects_when_already_pending() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_usdc");
    register_standard_pool_with_bluechip_side(&mut deps, 2, &std_pool);

    // First propose lands pending.
    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap();

    // Second propose against the same pool must fail before clobbering
    // the existing pending entry.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::OracleEligiblePoolAddAlreadyPending { .. }
    ));
}

#[test]
fn apply_add_rejects_when_no_pending() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_usdc");
    register_standard_pool_with_bluechip_side(&mut deps, 2, &std_pool);

    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&admin(), &[]),
        ExecuteMsg::ApplyAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::NoPendingOracleEligiblePoolAdd { .. }
    ));
}

#[test]
fn cancel_add_rejects_when_no_pending() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_usdc");
    register_standard_pool_with_bluechip_side(&mut deps, 2, &std_pool);

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::CancelAddOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::NoPendingOracleEligiblePoolAdd { .. }
    ));
}

#[test]
fn remove_rejects_when_pool_not_allowlisted() {
    let mut deps = fresh_factory();
    let std_pool = make_addr("std_pool_usdc");
    register_standard_pool_with_bluechip_side(&mut deps, 2, &std_pool);

    // Never allowlisted — remove must reject.
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::RemoveOracleEligiblePool {
            pool_addr: std_pool.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::OracleEligiblePoolNotAllowlisted { .. }
    ));
    // Sanity: the storage row really is absent.
    assert!(!ORACLE_ELIGIBLE_POOLS.has(&deps.storage, std_pool));
}

#[test]
fn propose_auto_eligible_flip_rejects_when_already_pending() {
    let mut deps = fresh_factory();
    // setup_atom_pool flips COMMIT_POOLS_AUTO_ELIGIBLE to true, so to
    // propose a *flip* the value the propose targets must differ from
    // the live one.
    let target = !COMMIT_POOLS_AUTO_ELIGIBLE.load(&deps.storage).unwrap();

    execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: target },
    )
    .unwrap();

    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::ProposeSetCommitPoolsAutoEligible { enabled: target },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ContractError::CommitPoolsAutoEligibleAlreadyPending
    ));
    assert!(PENDING_COMMIT_POOLS_AUTO_ELIGIBLE
        .may_load(&deps.storage)
        .unwrap()
        .is_some());
}

#[test]
fn apply_auto_eligible_flip_rejects_when_no_pending() {
    let mut deps = fresh_factory();
    let mut env = mock_env();
    env.block.time = env.block.time.plus_seconds(ADMIN_TIMELOCK_SECONDS + 1);
    let err = execute(
        deps.as_mut(),
        env,
        message_info(&admin(), &[]),
        ExecuteMsg::ApplySetCommitPoolsAutoEligible {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::NoPendingCommitPoolsAutoEligible));
}

#[test]
fn cancel_auto_eligible_flip_rejects_when_no_pending() {
    let mut deps = fresh_factory();
    let err = execute(
        deps.as_mut(),
        mock_env(),
        message_info(&admin(), &[]),
        ExecuteMsg::CancelSetCommitPoolsAutoEligible {},
    )
    .unwrap_err();
    assert!(matches!(err, ContractError::NoPendingCommitPoolsAutoEligible));
}
