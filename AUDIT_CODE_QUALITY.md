# Code Quality & Efficiency Audit Report

**Date:** 2026-03-31
**Scope:** Factory, Pool, Expand-Economy contracts + shared packages
**Type:** Code quality, efficiency, and maintainability (NOT security)

---

## PHASE 1: Dead Code & Unreachable Paths

### 1.1 — Unused Cargo Dependencies

| ID | Location | Finding |
|----|----------|---------|
| F1-DEAD-1 | `factory/Cargo.toml:35`, `pool/Cargo.toml:32`, `mockoracle/Cargo.toml:36` | `integer-sqrt` crate listed but never imported. `integer_sqrt` is hand-implemented in `pool/src/liquidity_helpers.rs:195-206`. |
| F1-DEAD-2 | `factory/Cargo.toml:45` | `protobuf` crate listed but no factory source file imports it. |
| F1-DEAD-4 | `pool/Cargo.toml:30` | `sha2` crate listed but no pool source file imports it. |

### 1.2 — Unused Functions & Types

| ID | Location | Finding |
|----|----------|---------|
| F1-DEAD-5 | `pool/src/asset.rs:160-170` | `pair_info_by_pool()` defined but never called from any execution or query path. |
| F1-DEAD-6 | `pool/src/asset.rs:80-137` | `TokenSending` trait and `impl for Vec<Coin>` defined but never used. |
| F1-DEAD-7 | `pool/src/oracle.rs:20-23` | `OracleData` struct defined but never referenced. |
| F1-DEAD-8 | `factory/src/query.rs:127-129` | `query_pending_config()` exists but not wired to any `QueryMsg` variant. Unreachable. |
| F1-DEAD-9 | `factory/src/query.rs:98-141` | `query_token_balance`, `query_token_ticker`, `query_balance` duplicate functions already in `pool-factory-interfaces::asset`. Not called from any non-test code. |
| F1-DEAD-10 | `factory/src/pool_struct.rs:77-83` | `ConfigResponse` defined but never constructed or returned by factory code. Pool has its own identical version. |

### 1.3 — Unused State Entries & Constants

| ID | Location | Finding |
|----|----------|---------|
| F1-DEAD-11 | `factory/src/state.rs:26,28` | `ATOM_BLUECHIP_ANCHOR_POOL_ADDRESS` and `POOL_CODE_ID` Items defined but never loaded or saved. The actual atom pool address lives in `FactoryInstantiate`. |
| F1-DEAD-12 | `factory/src/state.rs:22-24` | `PYTH_CONTRACT_ADDR` and `ATOM_USD_PRICE_FEED_ID` hardcoded constants defined but never used. Oracle reads from `FactoryInstantiate` config. |
| F1-DEAD-13 | `packages/pool-factory-interfaces/src/asset.rs:97` | `PoolPairType::Stable {}` variant exists but all pools are hardcoded to `Xyk`. |
| F1-DEAD-14 | `packages/pool-factory-interfaces/src/asset.rs:50-54` | `is_token_an_ibc_token()` method never called anywhere. |
| F1-DEAD-16 | `pool/Cargo.toml:24` | `backtraces = []` feature flag declared but no code uses `#[cfg(feature = "backtraces")]`. |

### 1.4 — No-op Code

| ID | Location | Finding |
|----|----------|---------|
| F1-DEAD-17 | `factory/src/migrate.rs:20-22` | Migration loads and saves identical config data (no schema change): `let config = FACTORYINSTANTIATEINFO.load(...)?; FACTORYINSTANTIATEINFO.save(..., &config)?;` |

---

## PHASE 2: Redundant & Duplicate Logic

### 2.1 — Duplicate Type Definitions

| ID | Location A | Location B | Type |
|----|-----------|-----------|------|
| F2-DUP-1 | `factory/src/pool_struct.rs:65-75` | `pool/src/msg.rs:228-238` | `CommitFeeInfo` (identical 4-field struct) |
| F2-DUP-2 | `factory/src/pool_struct.rs:54-64` | `pool/src/state.rs:203-212` | `ThresholdPayoutAmounts` (identical 4-field struct) |
| F2-DUP-3 | `factory/src/pool_struct.rs:37-43` | `pool/src/msg.rs:209-215` | `PoolConfigUpdate` (identical 4-field struct) |
| F2-DUP-4 | `factory/src/pool_struct.rs:77-83` | `pool/src/msg.rs:247-252` | `ConfigResponse` (identical, factory version unused) |

**Fix:** Move `CommitFeeInfo`, `ThresholdPayoutAmounts`, and `PoolConfigUpdate` into `pool-factory-interfaces` and import from there in both contracts.

### 2.2 — Duplicate COMMIT_INFO Update Logic (~100 lines)

| ID | Location | Context |
|----|----------|---------|
| F2-DUP-5 | `pool/src/contract.rs:1025-1049` | Threshold crossing, pre-threshold portion |
| F2-DUP-5 | `pool/src/contract.rs:1115-1129` | Threshold crossing, excess portion |
| F2-DUP-5 | `pool/src/contract.rs:1183-1207` | Threshold hit exact |
| F2-DUP-5 | `pool/src/contract.rs:1263-1287` | `process_pre_threshold_commit` |
| F2-DUP-5 | `pool/src/contract.rs:1357-1381` | `process_post_threshold_commit` |

**Fix:** Extract helper:
```rust
fn update_commit_info(
    storage: &mut dyn Storage,
    sender: &Addr,
    pool_contract_address: Addr,
    bluechip_amount: Uint128,
    usd_amount: Uint128,
    timestamp: Timestamp,
) -> Result<(), ContractError> {
    COMMIT_INFO.update(storage, sender, |maybe| -> Result<_, ContractError> {
        match maybe {
            Some(mut c) => {
                c.total_paid_bluechip = c.total_paid_bluechip.checked_add(bluechip_amount)?;
                c.total_paid_usd = c.total_paid_usd.checked_add(usd_amount)?;
                c.last_payment_bluechip = bluechip_amount;
                c.last_payment_usd = usd_amount;
                c.last_commited = timestamp;
                Ok(c)
            }
            None => Ok(Commiting {
                pool_contract_address,
                commiter: sender.clone(),
                total_paid_bluechip: bluechip_amount,
                total_paid_usd: usd_amount,
                last_commited: timestamp,
                last_payment_bluechip: bluechip_amount,
                last_payment_usd: usd_amount,
            }),
        }
    })?;
    Ok(())
}
```

### 2.3 — Other Duplications

| ID | Location | Finding |
|----|----------|---------|
| F2-DUP-6 | `pool/src/contract.rs:855-895` | Inline fee calculation (numerator/denominator pattern) done twice. Extract to `fn calculate_fee(amount, rate) -> Result<Uint128>`. |
| F2-DUP-7 | `pool/src/contract.rs:833-839, 1415-1421` | Bluechip denom extraction done manually when `get_bluechip_denom()` helper already exists. |
| F2-DUP-8 | `pool/src/query.rs:427-487` | `GetPoolState` and `GetAllPools` share 90% identical code. Extract `build_pool_state_for_factory()`. |

---

## PHASE 3: Gas & Compute Efficiency

### 3.1 — Unnecessary Storage Operations

| ID | Location | Impact | Finding |
|----|----------|--------|---------|
| F3-GAS-1 | `pool/src/liquidity.rs:210` | MEDIUM | `POOL_STATE` loaded twice in `execute_deposit_liquidity`. Second load at line 210 is immediately after a save at line 208. Use the local variable instead. |
| F3-GAS-2 | `pool/src/contract.rs:795→1316` | MEDIUM | `POOL_SPECS` loaded in `execute_commit_logic`, then loaded again in `process_post_threshold_commit`. Pass as parameter. |
| F3-GAS-3 | `pool/src/contract.rs:793→1315` | MEDIUM | `POOL_INFO` loaded in `execute_commit_logic`, then loaded again in `process_post_threshold_commit`. Pass as parameter. |
| F3-GAS-4 | `pool/src/contract.rs:1517,1532,1540` | LOW | `POOL_SPECS.update()` called up to 3 times in `execute_update_config_from_factory`. Load once, mutate, save once. |
| F3-GAS-5 | `factory/src/internal_bluechip_price_oracle.rs:64,286` | LOW | `FACTORYINSTANTIATEINFO` loaded twice in oracle update path. Pass as parameter to `calculate_weighted_price_with_atom`. |

### 3.2 — Expensive Iterations

| ID | Location | Impact | Finding |
|----|----------|--------|---------|
| F3-GAS-6 | `factory/src/internal_bluechip_price_oracle.rs:316-323` | **HIGH** | Linear scan of `POOLS_BY_ID` to find pool by address — **for every oracle pool**. O(N × oracle_pools). Use `POOLS_BY_CONTRACT_ADDRESS` or a reverse lookup map. |
| F3-GAS-7 | `factory/src/internal_bluechip_price_oracle.rs:171-218` | **HIGH** | `get_eligible_creator_pools` iterates ALL pools twice: once over `POOLS_BY_ID` to build a HashSet, then over `POOLS_BY_CONTRACT_ADDRESS` querying each. Combine into single pass. |

### 3.3 — Unused Computation

| ID | Location | Impact | Finding |
|----|----------|--------|---------|
| F3-GAS-12 | `pool/src/contract.rs:905-906` | LOW | `_total_fee_rate` computed but never used (underscore prefix). Delete. |

### 3.4 — Unnecessary Clones

| ID | Location | Impact | Finding |
|----|----------|--------|---------|
| F3-GAS-8 | `factory/src/internal_bluechip_price_oracle.rs:230,234` | LOW | `oracle.selected_pools.clone()` followed by potential second clone on rotation. Use reference when no rotation. |
| F3-GAS-9 | `pool/src/contract.rs:271,304,601,764` | LOW | Multiple unnecessary `info.sender.clone()` or `sender.clone()`. |
| F3-GAS-10 | `pool/src/liquidity.rs:665,699` | LOW | `info.clone()` passed to inner functions that only need `info.sender` and `info.funds`. |

---

## PHASE 4: Structural & Architectural Simplification

### 4.1 — Over-sized Functions

| ID | Location | Finding |
|----|----------|---------|
| F4-STRUCT-1 | `pool/src/contract.rs:784-1241` | `execute_commit_logic` is 400+ lines with 5+ nesting levels. Extract threshold-crossing logic into `process_threshold_crossing_commit()`. |

### 4.2 — Module Organization

| ID | Location | Finding |
|----|----------|---------|
| F4-STRUCT-2 | `pool/src/oracle.rs` | Only 23 lines. Merge `PythQueryMsg` and `PriceResponse` into `mock_querier.rs`. Delete `OracleData`. |
| F4-STRUCT-3 | `factory/src/asset.rs` | Single-line re-export file. Could be `pub use` in `lib.rs`. |

### 4.3 — Unused Error Variants

| ID | Location | Finding |
|----|----------|---------|
| F4-STRUCT-4 | `pool/src/error.rs:87-88` | `InvalidPaymentTiers {}` never returned. |
| F4-STRUCT-5 | `pool/src/error.rs:110-111` | `InsufficientFunds {}` never returned. |

### 4.4 — Redundant State Fields

| ID | Location | Finding |
|----|----------|---------|
| F4-STRUCT-7 | `pool/src/state.rs:99-100` | `estimated_gas_per_distribution` and `max_gas_per_tx` in `DistributionState` always set to constants. Use constants directly. |

### 4.5 — Test Coverage Gaps

| ID | Location | Finding |
|----|----------|---------|
| F4-STRUCT-8 | `pool/src/liquidity.rs:738-791` | `execute_remove_partial_liquidity_by_percent` has no dedicated test. |
| F4-STRUCT-9 | `pool/src/liquidity_helpers.rs:368-458` | `execute_claim_creator_excess` has no dedicated test. |
| F4-STRUCT-10 | `pool/src/contract.rs:1457-1487` | Pool migration paths (`UpdateFees`, `UpdateVersion`) have no tests. |
| F4-STRUCT-11 | `pool/src/contract.rs:1707-1727` | `CancelEmergencyWithdraw` path untested. |

---

## PHASE 5: Rust-Specific & CosmWasm-Specific Improvements

### 5.1 — Idiomatic Rust

| ID | Location | Finding | Fix |
|----|----------|---------|-----|
| F5-RUST-1 | `pool/src/contract.rs:531-541` | Manual loop for CW20 auth check | Use `.any()` with `matches!` |
| F5-RUST-2 | `pool/src/contract.rs:543-547` | Verbose `if let Some / else None` | Use `.map().transpose()?` |
| F5-RUST-3 | `pool/src/contract.rs:98` | `format!()` with no interpolation | Use `.to_string()` or literal |
| F5-RUST-4 | `factory/src/internal_bluechip_price_oracle.rs:143` | Redundant `env.clone()` before last use | Transfer ownership |

### 5.2 — CosmWasm Patterns

| ID | Location | Finding | Fix |
|----|----------|---------|-----|
| F5-CW-1 | Multiple liquidity/swap functions | Attributes added one-by-one (8+ per function) | Use `add_attributes(vec![...])` |
| F5-CW-2 | `factory/src/migrate.rs:10` | Missing `#[cfg_attr(not(feature = "library"), ...)]` on `entry_point` | Add the feature gate to match other entry points |
| F5-CW-3 | `factory/src/internal_bluechip_price_oracle.rs:389` | `Uint128::from(2u128)` | Use `Uint128::new(2)` |

---

## Summary

### Lines Removable/Consolidatable

| Category | Estimated Lines |
|----------|---------------:|
| Dead code removal | ~145 |
| Duplicate consolidation | ~180 |
| Structural simplification | ~50 |
| **Total** | **~375** |

### Priority Ranking

1. **F3-GAS-6 + F3-GAS-7** — Oracle O(N²) iteration → O(N). HIGH gas impact, MEDIUM effort.
2. **F2-DUP-5** — Extract COMMIT_INFO helper. ~100 lines removed. LOW effort.
3. **F1-DEAD-1,2,4** — Remove unused crate deps. Reduces binary size. Trivial.
4. **F1-DEAD-5 through F1-DEAD-17** — Remove all dead code. ~130 lines. Trivial.
5. **F3-GAS-1,2,3** — Eliminate redundant storage reads in hot paths. MEDIUM gas savings.
6. **F2-DUP-1,2,3** — Move shared types to interfaces crate. Prevents drift.
7. **F4-STRUCT-1** — Break up `execute_commit_logic`. HIGH maintainability.
8. **F3-GAS-4** — Consolidate POOL_SPECS updates. LOW gas, trivial.
9. **F5-CW-2** — Fix missing library feature gate on migrate. Correctness.
10. **F5-RUST-1,2,3** — Idiomatic Rust. Readability, trivial.

### Refactoring Roadmap

**Phase A — Zero-risk deletions:**
- Remove unused Cargo deps, dead code, unused constants/state items
- Fix `entry_point` feature gate
- Run `cargo test`

**Phase B — Extract helpers:**
- `update_commit_info()`, `calculate_fee()`, `build_pool_state_for_factory()`
- Replace manual denom extraction with `get_bluechip_denom()`
- Run `cargo test`

**Phase C — Move shared types:**
- Move `CommitFeeInfo`, `ThresholdPayoutAmounts`, `PoolConfigUpdate` to `pool-factory-interfaces`
- Run `cargo test`

**Phase D — Gas optimizations (storage):**
- Eliminate redundant storage loads by passing loaded state as parameters
- Run `cargo test`

**Phase E — Oracle optimization (highest risk):**
- Fix O(N²) pool iteration with reverse lookup or combined pass
- Run oracle-specific tests thoroughly

**Phase F — Structural (optional):**
- Break up `execute_commit_logic`
- Apply Rust idiom cleanups
