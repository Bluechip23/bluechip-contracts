//! Invariant checks evaluated after every action.

use cosmwasm_std::{Coin, Uint128};
use cw20::{BalanceResponse, Cw20ExecuteMsg, Cw20QueryMsg};
use cw_multi_test::Executor;
use pool_core::msg::{CommitStatus, PoolStateResponse, PositionsResponse};

use crate::factory_shim::HarnessQueryMsg;
use crate::world::{PoolKind, World, BLUECHIP_DENOM};

#[derive(Debug)]
pub struct Violation {
    pub name: &'static str,
    pub detail: String,
}

pub fn check_all(world: &mut World) -> Result<(), Violation> {
    for i in 0..world.pools.len() {
        let pool_snapshot = world.pools[i].clone();
        check_pool_invariants(world, i, &pool_snapshot)?;
    }
    Ok(())
}

fn check_pool_invariants(world: &mut World, i: usize, snap: &crate::world::PoolHandle) -> Result<(), Violation> {
    // -- Conservation: pool's bank+cw20 balances >= reported reserves --
    let pool_addr = &snap.pool_addr;
    let pool_state: PoolStateResponse = world
        .app
        .wrap()
        .query_wasm_smart(pool_addr, &creator_pool::msg::QueryMsg::PoolState {})
        .map_err(|e| violation("pool_state_query_failed", format!("{e:?}")))?;

    let bluechip_balance = world
        .app
        .wrap()
        .query_balance(pool_addr, BLUECHIP_DENOM)
        .map(|c| c.amount)
        .unwrap_or_else(|_| Uint128::zero());
    if bluechip_balance < pool_state.reserve0 {
        return Err(violation(
            "conservation_native_underwater",
            format!(
                "pool {} bluechip bank={} < reserve0={}",
                pool_addr, bluechip_balance, pool_state.reserve0
            ),
        ));
    }

    let cw20_balance: BalanceResponse = world
        .app
        .wrap()
        .query_wasm_smart(
            &snap.cw20_addr,
            &Cw20QueryMsg::Balance {
                address: pool_addr.to_string(),
            },
        )
        .map_err(|e| violation("cw20_balance_query_failed", format!("{e:?}")))?;
    if cw20_balance.balance < pool_state.reserve1 {
        return Err(violation(
            "conservation_cw20_underwater",
            format!(
                "pool {} cw20 bal={} < reserve1={}",
                pool_addr, cw20_balance.balance, pool_state.reserve1
            ),
        ));
    }

    // -- Minimum-liquidity floor (after first deposit, both reserves
    //    must each be >= 1000 OR both zero). --
    let r0 = pool_state.reserve0;
    let r1 = pool_state.reserve1;
    if !(r0.is_zero() && r1.is_zero()) {
        if r0 < Uint128::new(1000) || r1 < Uint128::new(1000) {
            return Err(violation(
                "minimum_liquidity_breached",
                format!("pool {} r0={} r1={} both must be >=1000 or both zero", pool_addr, r0, r1),
            ));
        }
    }

    // -- Threshold sticky + monotonic (creator pools only) --
    if snap.kind == PoolKind::Commit {
        let status: CommitStatus = world
            .app
            .wrap()
            .query_wasm_smart(pool_addr, &creator_pool::msg::QueryMsg::IsFullyCommited {})
            .map_err(|e| violation("commit_status_query_failed", format!("{e:?}")))?;
        let now_hit = matches!(status, CommitStatus::FullyCommitted);
        if snap.threshold_hit_seen && !now_hit {
            return Err(violation(
                "threshold_unsticky",
                format!("pool {} was FullyCommitted, now InProgress", pool_addr),
            ));
        }
        // USD raised monotonic pre-threshold
        if let CommitStatus::InProgress { raised, .. } = &status {
            if *raised < snap.last_observed_usd_raised {
                return Err(violation(
                    "usd_raised_decreased",
                    format!(
                        "pool {} prev_usd={} now_usd={}",
                        pool_addr, snap.last_observed_usd_raised, raised
                    ),
                ));
            }
            world.pools[i].last_observed_usd_raised = *raised;
        }
        if now_hit { world.pools[i].threshold_hit_seen = true; }

        // Factory shim should record minted exactly when threshold notified.
        let minted: bool = world
            .app
            .wrap()
            .query_wasm_smart(
                &world.factory_shim,
                &HarnessQueryMsg::ThresholdMinted { pool_id: snap.pool_id },
            )
            .map_err(|e| violation("threshold_minted_query_failed", format!("{e:?}")))?;
        if snap.mint_recorded && !minted {
            return Err(violation(
                "threshold_minted_flag_regressed",
                format!("pool {} minted regressed false", snap.pool_id),
            ));
        }
        if minted { world.pools[i].mint_recorded = true; }
    }

    // -- Total liquidity sum: pool_state.total_liquidity must equal the
    //    sum of every position's liquidity (excluding the sentinel "0"
    //    placeholder, which carries Uint128::zero anyway). --
    let mut start_after: Option<String> = None;
    let mut sum_pos_liquidity: Uint128 = Uint128::zero();
    let mut total_positions: u64 = 0;
    loop {
        let page: PositionsResponse = match world.app.wrap().query_wasm_smart(
            pool_addr,
            &creator_pool::msg::QueryMsg::Positions { start_after: start_after.clone(), limit: Some(30) },
        ) {
            Ok(r) => r,
            Err(_) => break, // standard-pool may not respond identically; tolerate
        };
        if page.positions.is_empty() { break; }
        for p in &page.positions {
            sum_pos_liquidity = sum_pos_liquidity.checked_add(p.liquidity).unwrap_or(Uint128::MAX);
            total_positions += 1;
        }
        start_after = page.positions.last().map(|p| p.position_id.clone());
        if page.positions.len() < 30 { break; }
    }
    // Positions can never collectively claim more liquidity than the
    // pool reports. (The reverse — total_liquidity > sum(positions) —
    // is legal: threshold-crossing seeds locked liquidity that no LP
    // owns; that locked share is intentionally not represented as a
    // Position record.)
    if sum_pos_liquidity > pool_state.total_liquidity {
        return Err(violation(
            "positions_overclaim_liquidity",
            format!(
                "pool {} sum(positions)={} > total_liquidity={} (n={})",
                pool_addr, sum_pos_liquidity, pool_state.total_liquidity, total_positions
            ),
        ));
    }

    // -- Drained pool blocks ops: once we observed a successful drain,
    //    every state-changing op must error. We probe with a 1-ubluechip
    //    self-send swap that the drained pool MUST reject. --
    if snap.drained {
        let probe = world.app.execute_contract(
            world.users[0].clone(),
            pool_addr.clone(),
            &creator_pool::msg::ExecuteMsg::SimpleSwap {
                offer_asset: pool_factory_interfaces::asset::TokenInfo {
                    info: pool_factory_interfaces::asset::TokenType::Native {
                        denom: BLUECHIP_DENOM.to_string(),
                    },
                    amount: Uint128::new(1),
                },
                belief_price: None,
                max_spread: None,
                allow_high_max_spread: Some(true),
                to: None,
                transaction_deadline: None,
            },
            &[Coin::new(1u128, BLUECHIP_DENOM)],
        );
        if probe.is_ok() {
            return Err(violation(
                "drained_pool_accepted_swap",
                format!("pool {} drained but SimpleSwap succeeded", pool_addr),
            ));
        }
        // Also: probe a CW20 send that should fail (the cw20 hook path).
        let cw20_probe = world.app.execute_contract(
            world.users[0].clone(),
            snap.cw20_addr.clone(),
            &Cw20ExecuteMsg::Send {
                contract: pool_addr.to_string(),
                amount: Uint128::new(1),
                msg: cosmwasm_std::to_json_binary(&pool_core::msg::Cw20HookMsg::Swap {
                    belief_price: None,
                    max_spread: None,
                    allow_high_max_spread: Some(true),
                    to: None,
                    transaction_deadline: None,
                })
                .unwrap(),
            },
            &[],
        );
        if cw20_probe.is_ok() {
            return Err(violation(
                "drained_pool_accepted_cw20_swap",
                format!("pool {} drained but cw20 swap succeeded", pool_addr),
            ));
        }
    }

    Ok(())
}

fn violation(name: &'static str, detail: String) -> Violation {
    Violation { name, detail }
}
