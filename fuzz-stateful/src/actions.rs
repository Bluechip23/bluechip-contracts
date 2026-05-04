//! Action enum and applier for the stateful fuzzer.

use cosmwasm_std::{Coin, Decimal, Uint128};
use cw20::Cw20ExecuteMsg;
use cw_multi_test::Executor;
use pool_factory_interfaces::asset::{TokenInfo, TokenType};
use proptest_derive::Arbitrary;

use crate::world::{
    advance_block, set_oracle_rate, PoolKind, World, BLUECHIP_DENOM,
};

/// Every meaningful state transition + a few illegal-but-typed variants.
#[derive(Debug, Clone, Arbitrary)]
pub enum Action {
    /// Spawn a new commit pool.
    CreateCreatorPool {
        #[proptest(strategy = "::proptest::sample::select(vec![6u8, 8u8, 18u8])")]
        decimals: u8,
    },
    /// Spawn a new standard pool with reserves.
    CreateStandardPool {
        #[proptest(strategy = "::proptest::sample::select(vec![6u8, 8u8, 18u8])")]
        decimals: u8,
        #[proptest(strategy = "1_000_000u128..1_000_000_000u128")]
        seed_native: u128,
        #[proptest(strategy = "1_000_000u128..1_000_000_000u128")]
        seed_cw20: u128,
    },
    /// Bluechip-side commit on a creator pool. `amount` is in ubluechip.
    Commit {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u128..50_000_000_000u128")]
        amount: u128,
    },
    /// Native-in swap (bluechip -> creator). Only meaningful post-threshold
    /// or on standard pools.
    SwapNativeIn {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u128..10_000_000_000u128")]
        amount: u128,
    },
    /// CW20-in swap (creator -> bluechip). Goes through Cw20::Send hook.
    SwapCw20In {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u128..10_000_000_000u128")]
        amount: u128,
    },
    /// Add liquidity (post-threshold on creator pools, always on std pools).
    /// CW20 sides must be pre-approved; we issue an `IncreaseAllowance`
    /// inline so this composes.
    DepositLiquidity {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u128..1_000_000_000u128")]
        amount0: u128,
        #[proptest(strategy = "1u128..1_000_000_000u128")]
        amount1: u128,
    },
    /// Remove liquidity by percentage from the user's first known position.
    RemoveLiquidityPercent {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u64..=100u64")]
        percent: u64,
    },
    /// Set oracle rate. Includes degenerate values to exercise zero/huge
    /// rejection.
    UpdateOraclePrice {
        #[proptest(strategy = "::proptest::sample::select(vec![0u128, 1u128, 1_000u128, 1_000_000u128, 5_000_000u128, 1_000_000_000_000u128])")]
        new_rate: u128,
        #[proptest(strategy = "::proptest::sample::select(vec![0u64, 30u64, 120u64, 3600u64])")]
        stale_secs: u64,
    },
    /// Skip forward in time + block height.
    AdvanceBlock {
        #[proptest(strategy = "1u64..3600u64")]
        secs: u64,
    },
    /// Illegal: non-factory tries to call UpdateConfigFromFactory.
    AttemptUnauthorizedConfigUpdate {
        #[proptest(strategy = "0usize..5usize")]
        attacker_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
    /// Illegal: non-pool address calls factory.NotifyThresholdCrossed.
    AttemptUnauthorizedThresholdNotify {
        #[proptest(strategy = "0usize..5usize")]
        attacker_idx: usize,
        #[proptest(strategy = "1u64..200u64")]
        forged_pool_id: u64,
    },
    /// Illegal: non-router caller invokes router internal handler.
    AttemptUnauthorizedRouterInternal {
        #[proptest(strategy = "0usize..5usize")]
        attacker_idx: usize,
    },
    /// Illegal: send mockoracle SetPrice with zero (must error).
    AttemptOraclePriceZero,

    // -----------------------------------------------------------------
    // Emergency-withdraw lifecycle (factory-initiated)
    // -----------------------------------------------------------------
    /// Phase 1 of emergency withdraw: factory_shim sends `EmergencyWithdraw`
    /// to a pool with no pending timelock. Arms the 24h timelock and hard-pauses
    /// the pool.
    EmergencyInitiate {
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
    /// Cancels a pending emergency withdraw. Errors if none pending.
    EmergencyCancel {
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
    /// Phase 2: factory_shim re-sends `EmergencyWithdraw` after the timelock.
    /// Drains all pool reserves + fees + creator pots. Sets `EMERGENCY_DRAINED`.
    /// Errors if not initiated or timelock not yet elapsed.
    EmergencyExecute {
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },

    // -----------------------------------------------------------------
    // Post-threshold distribution
    // -----------------------------------------------------------------
    /// Permissionless keeper: advances the per-pool committer-payout
    /// distribution batch. Errors before threshold-cross or when nothing
    /// to do.
    ContinueDistribution {
        #[proptest(strategy = "0usize..5usize")]
        keeper_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },

    // -----------------------------------------------------------------
    // Creator claims
    // -----------------------------------------------------------------
    /// Creator-only. Claims fees accumulated in CREATOR_FEE_POT.
    /// In our harness `commit_fee_info.creator_wallet_address == admin`.
    ClaimCreatorFees {
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
    /// Creator-only. Claims excess bluechip locked above
    /// max_bluechip_lock_per_pool. Errors before unlock_time or before
    /// threshold-cross.
    ClaimCreatorExcess {
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },

    // -----------------------------------------------------------------
    // Router multi-hop happy path
    // -----------------------------------------------------------------
    /// Native -> CW20 single-hop swap through the router. Requires at
    /// least one standard pool. Exercises the route validation +
    /// minimum-receive assertion path end-to-end.
    RouterSingleHop {
        #[proptest(strategy = "0usize..5usize")]
        user_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
        #[proptest(strategy = "1u128..100_000_000u128")]
        amount: u128,
    },

    // -----------------------------------------------------------------
    // Illegal extras
    // -----------------------------------------------------------------
    /// Illegal: non-factory tries to call EmergencyWithdraw on a pool.
    AttemptUnauthorizedEmergency {
        #[proptest(strategy = "0usize..5usize")]
        attacker_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
    /// Illegal: non-creator wallet attempts ClaimCreatorFees / ClaimCreatorExcess.
    AttemptUnauthorizedCreatorClaim {
        #[proptest(strategy = "0usize..5usize")]
        attacker_idx: usize,
        #[proptest(strategy = "0usize..8usize")]
        pool_idx: usize,
    },
}

#[derive(Debug)]
pub struct ActionOutcome {
    pub action: Action,
    pub kind: OutcomeKind,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Ok,
    /// Action was expected to fail (illegal-by-design) and did.
    ExpectedErr,
    /// Action ran and was rejected by contract validation. Acceptable
    /// for fuzz — we just record and continue.
    Rejected,
}

pub fn apply(world: &mut World, action: Action) -> ActionOutcome {
    let action_dbg = action.clone();
    match action {
        Action::CreateCreatorPool { decimals } => {
            // Create up to 4 creator pools; ignore beyond that.
            let count_commit = world
                .pools
                .iter()
                .filter(|p| p.kind == PoolKind::Commit)
                .count();
            if count_commit >= 4 {
                return ActionOutcome {
                    action: action_dbg,
                    kind: OutcomeKind::Rejected,
                    note: "creator-pool cap reached".into(),
                };
            }
            match crate::world::create_creator_pool(world, decimals) {
                Ok(_) => mk_ok(action_dbg, "created creator pool"),
                Err(e) => mk_rejected(action_dbg, &format!("create_creator_pool: {e}")),
            }
        }
        Action::CreateStandardPool { decimals, seed_native, seed_cw20 } => {
            let count_std = world
                .pools
                .iter()
                .filter(|p| p.kind == PoolKind::Standard)
                .count();
            if count_std >= 4 {
                return ActionOutcome {
                    action: action_dbg,
                    kind: OutcomeKind::Rejected,
                    note: "standard-pool cap reached".into(),
                };
            }
            match crate::world::create_standard_pool(world, decimals, seed_native, seed_cw20) {
                Ok(_) => mk_ok(action_dbg, "created standard pool"),
                Err(e) => mk_rejected(action_dbg, &format!("create_standard_pool: {e}")),
            }
        }
        Action::Commit { user_idx, pool_idx, amount } => {
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Commit)) else {
                return mk_rejected(action_dbg, "no creator pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            let bal = world.app.wrap().query_balance(&user, BLUECHIP_DENOM)
                .map(|b| b.amount.u128()).unwrap_or(0);
            if amount > bal {
                return mk_rejected(action_dbg, "insufficient bluechip balance");
            }
            let res = world.app.execute_contract(
                user.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::Commit {
                    asset: TokenInfo {
                        info: TokenType::Native { denom: BLUECHIP_DENOM.to_string() },
                        amount: Uint128::new(amount),
                    },
                    transaction_deadline: None,
                    belief_price: None,
                    max_spread: Some(Decimal::percent(10)),
                },
                &[Coin::new(amount, BLUECHIP_DENOM)],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "commit ok"),
                Err(e) => mk_rejected(action_dbg, &format!("commit err: {e:?}")),
            }
        }
        Action::SwapNativeIn { user_idx, pool_idx, amount } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            let bal = world.app.wrap().query_balance(&user, BLUECHIP_DENOM)
                .map(|b| b.amount.u128()).unwrap_or(0);
            if amount > bal { return mk_rejected(action_dbg, "insufficient bluechip"); }
            let msg = creator_pool::msg::ExecuteMsg::SimpleSwap {
                offer_asset: TokenInfo {
                    info: TokenType::Native { denom: BLUECHIP_DENOM.to_string() },
                    amount: Uint128::new(amount),
                },
                belief_price: None,
                max_spread: Some(Decimal::percent(10)),
                allow_high_max_spread: Some(true),
                to: None,
                transaction_deadline: None,
            };
            let res = world.app.execute_contract(
                user.clone(),
                pool.pool_addr.clone(),
                &msg,
                &[Coin::new(amount, BLUECHIP_DENOM)],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "swap_native ok"),
                Err(e) => mk_rejected(action_dbg, &format!("swap_native: {e}")),
            }
        }
        Action::SwapCw20In { user_idx, pool_idx, amount } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            let hook = pool_core::msg::Cw20HookMsg::Swap {
                belief_price: None,
                max_spread: Some(Decimal::percent(10)),
                allow_high_max_spread: Some(true),
                to: None,
                transaction_deadline: None,
            };
            let send = Cw20ExecuteMsg::Send {
                contract: pool.pool_addr.to_string(),
                amount: Uint128::new(amount),
                msg: cosmwasm_std::to_json_binary(&hook).unwrap(),
            };
            let res = world.app.execute_contract(user, pool.cw20_addr.clone(), &send, &[]);
            match res {
                Ok(_) => mk_ok(action_dbg, "swap_cw20 ok"),
                Err(e) => mk_rejected(action_dbg, &format!("swap_cw20: {e}")),
            }
        }
        Action::DepositLiquidity { user_idx, pool_idx, amount0, amount1 } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            // Approve cw20 side
            let _ = world.app.execute_contract(
                user.clone(),
                pool.cw20_addr.clone(),
                &Cw20ExecuteMsg::IncreaseAllowance {
                    spender: pool.pool_addr.to_string(),
                    amount: Uint128::new(amount1),
                    expires: None,
                },
                &[],
            );
            // creator-pool's DepositLiquidity expects (amount0, amount1).
            let msg = creator_pool::msg::ExecuteMsg::DepositLiquidity {
                amount0: Uint128::new(amount0),
                amount1: Uint128::new(amount1),
                min_amount0: None,
                min_amount1: None,
                transaction_deadline: None,
            };
            let res = world.app.execute_contract(
                user,
                pool.pool_addr.clone(),
                &msg,
                &[Coin::new(amount0, BLUECHIP_DENOM)],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "deposit ok"),
                Err(e) => mk_rejected(action_dbg, &format!("deposit: {e}")),
            }
        }
        Action::RemoveLiquidityPercent { user_idx, pool_idx, percent } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            // Look up the user's first position via PositionsByOwner.
            let positions: pool_core::msg::PositionsResponse =
                match world.app.wrap().query_wasm_smart(
                    &pool.pool_addr,
                    &creator_pool::msg::QueryMsg::PositionsByOwner {
                        owner: user.to_string(),
                        start_after: None,
                        limit: Some(1),
                    },
                ) {
                    Ok(p) => p,
                    Err(_) => return mk_rejected(action_dbg, "positions query failed"),
                };
            let Some(first) = positions.positions.first() else {
                return mk_rejected(action_dbg, "no position");
            };
            let msg = creator_pool::msg::ExecuteMsg::RemovePartialLiquidityByPercent {
                position_id: first.position_id.clone(),
                percentage: percent,
                transaction_deadline: None,
                min_amount0: None,
                min_amount1: None,
                max_ratio_deviation_bps: Some(10_000),
            };
            let res = world.app.execute_contract(user, pool.pool_addr.clone(), &msg, &[]);
            match res {
                Ok(_) => mk_ok(action_dbg, "remove ok"),
                Err(e) => mk_rejected(action_dbg, &format!("remove: {e}")),
            }
        }
        Action::UpdateOraclePrice { new_rate, stale_secs } => {
            let now = world.app.block_info().time.seconds();
            let ts = now.saturating_sub(stale_secs);
            let res = set_oracle_rate(world, Uint128::new(new_rate), ts);
            match (new_rate, res) {
                (0, Err(_)) => mk_expected(action_dbg, "rate=0 rejected"),
                (0, Ok(_)) => panic!("INVARIANT BROKEN: shim accepted rate=0"),
                (_, Ok(_)) => mk_ok(action_dbg, "rate updated"),
                (_, Err(e)) => mk_rejected(action_dbg, &format!("set_rate: {e}")),
            }
        }
        Action::AdvanceBlock { secs } => {
            advance_block(world, secs);
            mk_ok(action_dbg, "block advanced")
        }
        Action::AttemptUnauthorizedConfigUpdate { attacker_idx, pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let attacker = world.users[attacker_idx % world.users.len()].clone();
            let res = world.app.execute_contract(
                attacker,
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::UpdateConfigFromFactory {
                    update: pool_core::msg::PoolConfigUpdate { lp_fee: Some(Decimal::percent(50)), min_commit_interval: Some(0) },
                },
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "non-factory rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: pool accepted UpdateConfigFromFactory from non-factory"),
            }
        }
        Action::AttemptUnauthorizedThresholdNotify { attacker_idx, forged_pool_id } => {
            let attacker = world.users[attacker_idx % world.users.len()].clone();
            let res = world.app.execute_contract(
                attacker,
                world.factory_shim.clone(),
                &crate::factory_shim::HarnessExecuteMsg::Factory(
                    pool_factory_interfaces::FactoryExecuteMsg::NotifyThresholdCrossed {
                        pool_id: forged_pool_id,
                    }
                ),
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "non-pool rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: factory_shim accepted NotifyThresholdCrossed from non-pool"),
            }
        }
        Action::AttemptUnauthorizedRouterInternal { attacker_idx } => {
            let Some(router_addr) = world.router.clone() else {
                return mk_rejected(action_dbg, "no router");
            };
            let attacker = world.users[attacker_idx % world.users.len()].clone();
            let res = world.app.execute_contract(
                attacker,
                router_addr,
                &router::msg::ExecuteMsg::ExecuteSwapOperation {
                    operation: pool_factory_interfaces::routing::SwapOperation {
                        pool_addr: world.factory_shim.to_string(),
                        offer_asset_info: TokenType::Native { denom: BLUECHIP_DENOM.into() },
                        ask_asset_info: TokenType::Native { denom: BLUECHIP_DENOM.into() },
                    },
                    hop_index: 0,
                    to: world.admin.to_string(),
                },
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "router internal rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: router accepted internal from external sender"),
            }
        }
        Action::AttemptOraclePriceZero => {
            let res = world.app.execute_contract(
                world.admin.clone(),
                world.mockoracle.clone(),
                &oracle::msg::ExecuteMsg::SetPrice {
                    price_id: "BLUECHIP_USD".to_string(),
                    price: Uint128::zero(),
                },
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "mockoracle zero rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: mockoracle accepted price=0"),
            }
        }

        // ---- Emergency withdraw lifecycle ----
        Action::EmergencyInitiate { pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            // Sender = factory_shim (the pool's expected factory).
            let res = world.app.execute_contract(
                world.factory_shim.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::EmergencyWithdraw {},
                &[],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "emergency initiated"),
                Err(e) => mk_rejected(action_dbg, &format!("initiate: {e:?}")),
            }
        }
        Action::EmergencyCancel { pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let res = world.app.execute_contract(
                world.factory_shim.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::CancelEmergencyWithdraw {},
                &[],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "emergency cancelled"),
                Err(e) => mk_rejected(action_dbg, &format!("cancel: {e:?}")),
            }
        }
        Action::EmergencyExecute { pool_idx } => {
            let pool = match pick_pool(world, pool_idx, None) {
                Some(p) => p,
                None => return mk_rejected(action_dbg, "no pool"),
            };
            // Phase 2: same EmergencyWithdraw entry-point — the pool
            // itself decides between initiate/drain based on whether
            // the timelock has elapsed.
            let res = world.app.execute_contract(
                world.factory_shim.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::EmergencyWithdraw {},
                &[],
            );
            match res {
                Ok(r) => {
                    // creator-pool's wrapper emits action="emergency_withdraw"
                    // on phase 2 drain (vs "emergency_withdraw_initiated"
                    // on phase 1).
                    let drained = r.events.iter().any(|e| {
                        e.attributes.iter().any(|a| {
                            a.key == "action" && a.value == "emergency_withdraw"
                        })
                    });
                    if drained {
                        // Track for the drain-blocks-ops invariant.
                        if let Some(p) = world.pools.iter_mut().find(|p| p.pool_addr == pool.pool_addr) {
                            p.drained = true;
                        }
                    }
                    mk_ok(action_dbg, if drained { "drain completed" } else { "phase 2 ok (re-initiate?)" })
                }
                Err(e) => mk_rejected(action_dbg, &format!("execute: {e:?}")),
            }
        }

        // ---- Post-threshold distribution ----
        Action::ContinueDistribution { keeper_idx, pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Commit)) else {
                return mk_rejected(action_dbg, "no creator pool");
            };
            let keeper = world.users[keeper_idx % world.users.len()].clone();
            let res = world.app.execute_contract(
                keeper,
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::ContinueDistribution {},
                &[],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "distribution batch ok"),
                Err(e) => mk_rejected(action_dbg, &format!("dist: {e:?}")),
            }
        }

        // ---- Creator claims ----
        Action::ClaimCreatorFees { pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Commit)) else {
                return mk_rejected(action_dbg, "no creator pool");
            };
            // commit_fee_info.creator_wallet_address == admin in our harness
            // (see world::create_creator_pool).
            let res = world.app.execute_contract(
                world.admin.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::ClaimCreatorFees { transaction_deadline: None },
                &[],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "creator fees claimed"),
                Err(e) => mk_rejected(action_dbg, &format!("claim_fees: {e:?}")),
            }
        }
        Action::ClaimCreatorExcess { pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Commit)) else {
                return mk_rejected(action_dbg, "no creator pool");
            };
            let res = world.app.execute_contract(
                world.admin.clone(),
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline: None },
                &[],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "creator excess claimed"),
                Err(e) => mk_rejected(action_dbg, &format!("claim_excess: {e:?}")),
            }
        }

        // ---- Router single-hop ----
        Action::RouterSingleHop { user_idx, pool_idx, amount } => {
            let Some(router_addr) = world.router.clone() else {
                return mk_rejected(action_dbg, "no router");
            };
            // Need a standard pool (or post-threshold creator pool) to
            // route through. We pick a standard pool — they're always
            // tradeable.
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Standard)) else {
                return mk_rejected(action_dbg, "no standard pool");
            };
            let user = world.users[user_idx % world.users.len()].clone();
            let bal = world.app.wrap().query_balance(&user, BLUECHIP_DENOM)
                .map(|c| c.amount.u128()).unwrap_or(0);
            if amount > bal { return mk_rejected(action_dbg, "insufficient bluechip"); }

            let op = pool_factory_interfaces::routing::SwapOperation {
                pool_addr: pool.pool_addr.to_string(),
                offer_asset_info: TokenType::Native { denom: BLUECHIP_DENOM.into() },
                ask_asset_info: TokenType::CreatorToken { contract_addr: pool.cw20_addr.clone() },
            };
            let res = world.app.execute_contract(
                user,
                router_addr,
                &router::msg::ExecuteMsg::ExecuteMultiHop {
                    operations: vec![op],
                    minimum_receive: Uint128::new(1),
                    deadline: None,
                    recipient: None,
                },
                &[Coin::new(amount, BLUECHIP_DENOM)],
            );
            match res {
                Ok(_) => mk_ok(action_dbg, "router single-hop ok"),
                Err(e) => mk_rejected(action_dbg, &format!("router: {e:?}")),
            }
        }

        // ---- Illegal extras ----
        Action::AttemptUnauthorizedEmergency { attacker_idx, pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, None) else {
                return mk_rejected(action_dbg, "no pool");
            };
            let attacker = world.users[attacker_idx % world.users.len()].clone();
            let res = world.app.execute_contract(
                attacker,
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::EmergencyWithdraw {},
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "non-factory emergency rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: pool accepted EmergencyWithdraw from non-factory"),
            }
        }
        Action::AttemptUnauthorizedCreatorClaim { attacker_idx, pool_idx } => {
            let Some(pool) = pick_pool(world, pool_idx, Some(PoolKind::Commit)) else {
                return mk_rejected(action_dbg, "no creator pool");
            };
            let attacker = world.users[attacker_idx % world.users.len()].clone();
            // Anyone other than admin (which is the harness's creator wallet).
            if attacker == world.admin {
                return mk_rejected(action_dbg, "attacker == creator wallet");
            }
            let res = world.app.execute_contract(
                attacker,
                pool.pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::ClaimCreatorFees { transaction_deadline: None },
                &[],
            );
            match res {
                Err(_) => mk_expected(action_dbg, "non-creator claim rejected"),
                Ok(_) => panic!("INVARIANT BROKEN: pool accepted ClaimCreatorFees from non-creator"),
            }
        }
    }
}

fn pick_pool(world: &World, idx: usize, kind: Option<PoolKind>) -> Option<crate::world::PoolHandle> {
    let candidates: Vec<_> = match kind {
        Some(k) => world.pools.iter().filter(|p| p.kind == k).cloned().collect(),
        None => world.pools.iter().cloned().collect(),
    };
    if candidates.is_empty() { return None; }
    Some(candidates[idx % candidates.len()].clone())
}

fn mk_ok(a: Action, n: &str) -> ActionOutcome {
    ActionOutcome { action: a, kind: OutcomeKind::Ok, note: n.into() }
}
fn mk_rejected(a: Action, n: &str) -> ActionOutcome {
    ActionOutcome { action: a, kind: OutcomeKind::Rejected, note: n.into() }
}
fn mk_expected(a: Action, n: &str) -> ActionOutcome {
    ActionOutcome { action: a, kind: OutcomeKind::ExpectedErr, note: n.into() }
}
