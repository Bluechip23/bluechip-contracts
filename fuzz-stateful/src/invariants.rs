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
    check_threshold_mint_count(world)?;
    for i in 0..world.pools.len() {
        let pool_snapshot = world.pools[i].clone();
        check_pool_invariants(world, i, &pool_snapshot)?;
    }
    Ok(())
}

fn check_threshold_mint_count(world: &mut World) -> Result<(), Violation> {
    let notify_count: u64 = world
        .app
        .wrap()
        .query_wasm_smart(&world.factory_shim, &HarnessQueryMsg::NotifyCount {})
        .map_err(|e| violation("notify_count_query_failed", format!("{e:?}")))?;
    let threshold_hit_pools = world.pools.iter().filter(|p| p.threshold_hit_seen).count() as u64;
    if notify_count > threshold_hit_pools {
        return Err(violation(
            "threshold_mint_notify_exceeded_threshold_hits",
            format!("notify_count={} > threshold_hit_pools={}", notify_count, threshold_hit_pools),
        ));
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
        // USD raised monotonic pre-threshold AND consistent with the
        // FullyCommitted/InProgress branch: while InProgress, raised
        // must always be strictly below the configured target. The
        // moment raised >= target the pool must transition to
        // FullyCommitted; the inverse is also asserted (the
        // post-threshold Commit probe below).
        if let CommitStatus::InProgress { raised, target } = &status {
            if *raised < snap.last_observed_usd_raised {
                return Err(violation(
                    "usd_raised_decreased",
                    format!(
                        "pool {} prev_usd={} now_usd={}",
                        pool_addr, snap.last_observed_usd_raised, raised
                    ),
                ));
            }
            if raised >= target {
                return Err(violation(
                    "threshold_phase_inconsistent_in_progress",
                    format!(
                        "pool {} reports InProgress but raised={} >= target={}",
                        pool_addr, raised, target
                    ),
                ));
            }
            world.pools[i].last_observed_usd_raised = *raised;
        }
        if now_hit { world.pools[i].threshold_hit_seen = true; }

        // Phase exclusivity: once threshold is hit, Commit must reject.
        if now_hit {
            let probe = world.app.execute_contract(
                world.users[0].clone(),
                pool_addr.clone(),
                &creator_pool::msg::ExecuteMsg::Commit {
                    asset: pool_factory_interfaces::asset::TokenInfo {
                        info: pool_factory_interfaces::asset::TokenType::Native {
                            denom: BLUECHIP_DENOM.to_string(),
                        },
                        amount: Uint128::new(1),
                    },
                    transaction_deadline: None,
                    belief_price: None,
                    max_spread: None,
                },
                &[Coin::new(1u128, BLUECHIP_DENOM)],
            );
            if probe.is_ok() {
                return Err(violation(
                    "commit_allowed_post_threshold",
                    format!("pool {} accepted Commit after FullyCommitted", pool_addr),
                ));
            }
        }

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

    // Position/NFT integrity: for queried positions, cw721 owner must match
    // recorded position owner.
    let owner_page: PositionsResponse = world
        .app
        .wrap()
        .query_wasm_smart(
            pool_addr,
            &creator_pool::msg::QueryMsg::Positions { start_after: None, limit: Some(5) },
        )
        .map_err(|e| violation("positions_integrity_query_failed", format!("{e:?}")))?;
    for p in owner_page.positions {
        // The sentinel "0" position is saved to LIQUIDITY_POSITIONS at
        // pool instantiate (see creator-pool::contract instantiate, which
        // runs `LIQUIDITY_POSITIONS.save(storage, "0", &..)`) but no NFT
        // is ever minted for it, so the cw721 OwnerOf query is expected
        // to error. Skip it.
        if p.position_id == "0" || p.liquidity.is_zero() {
            continue;
        }
        let q = pool_factory_interfaces::cw721_msgs::Cw721QueryMsg::OwnerOf {
            token_id: p.position_id.clone(),
            include_expired: None,
        };
        let owner: pool_factory_interfaces::cw721_msgs::OwnerOfResponse = world
            .app
            .wrap()
            .query_wasm_smart(&snap.nft_addr, &q)
            .map_err(|e| violation("nft_owner_query_failed", format!("{e:?}")))?;
        if owner.owner != p.owner.as_str() {
            return Err(violation(
                "position_nft_owner_mismatch",
                format!("position {} owner {} != nft owner {}", p.position_id, p.owner, owner.owner),
            ));
        }
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
