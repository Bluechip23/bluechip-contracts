//! Invariant checks evaluated after every action.

use cosmwasm_std::Uint128;
use cw20::{BalanceResponse, Cw20QueryMsg};
use pool_core::msg::{CommitStatus, PoolStateResponse};

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

    Ok(())
}

fn violation(name: &'static str, detail: String) -> Violation {
    Violation { name, detail }
}
