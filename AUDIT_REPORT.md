# Bluechip Contracts — Smart Contract Audit Report (Re-Audit #3)

**Audit Date:** 2026-02-24
**Auditor:** Independent CosmWasm Security Review (Claude Opus 4.6)
**Codebase Commit:** Current working tree
**Prior Reports:** 2026-02-13 (initial), 2026-02-19 (re-audit #2)
**Verdict:** CONDITIONAL PASS — See remaining items below

---

## Executive Summary

This is the **third audit** of the bluechip-contracts codebase. The development team has addressed the vast majority of findings from the two prior reports. Most critically:

- **C-NEW-1 (Dead Code Oracle)** — the production-breaking bug from the prior report — is now **FIXED**. The pool's `QueryMsg` enum now includes `GetPoolState` and `GetAllPools` variants, and the entry point correctly dispatches them to `query_for_factory`.
- **H-NEW-1 (Unbounded LP Fee)** — now **FIXED** with a 10% max and 0.1% minimum enforced in `execute_update_config_from_factory`.
- **H-3 (Emergency Withdraw No Timelock)** — now **FIXED** with a two-phase 24-hour timelock and cancellation mechanism.
- **H-NEW-2 (ProposeConfigUpdate No Validation)** — now **FIXED** with full `addr_validate` calls on all address fields.
- **H-1 (Predictable Oracle Entropy)** — now **MITIGATED** by mixing in prior oracle state (TWAP price, update timestamp, observation count) to the hash, making pool selection unpredictable at block-production time.

**No Critical issues remain.** The remaining open items are Medium and Low severity, none of which are blocking for a carefully monitored initial deployment.

---

## Verdict Summary

| Severity | Total Historical | Resolved | Still Open | Net Open |
|----------|-----------------|----------|------------|----------|
| Critical | 4 | **4 fixed** | 0 | **0** |
| High | 7 | **7 fixed** | 0 | **0** |
| Medium | 8 | 6 fixed, 2 partial | 2 + 3 new | **5** |
| Low | 13 | 8 fixed | 3 + 4 new | **7** |

---

## CRITICAL FINDINGS — ALL RESOLVED

### C-1 — Post-Threshold Commit Fee Double Deduction ✅ FIXED
**File:** `pool/src/contract.rs:1067-1070, 1319-1322`
Correctly subtracts both `return_amt` and `commission_amt` from ask reserve.

### C-2 — Split-Commit Excess Uses Wrong Fee Denominator ✅ FIXED
**File:** `pool/src/contract.rs:972-978`
Post-fee split now uses proportional division against the original `amount`.

### C-3 — Reentrancy Guard Stuck State ✅ FIXED
**File:** `pool/src/contract.rs:477-489`
`RecoveryType::StuckReentrancyGuard` allows factory admin to reset a stuck guard.

### C-NEW-1 — `query_for_factory` Dead Code ✅ FIXED
**File:** `pool/src/query.rs:89-94`, `pool/src/msg.rs`
The pool's `QueryMsg` now includes `GetPoolState { pool_contract_address }` and `GetAllPools {}` variants. The query entry point dispatches these to `query_for_factory`, which correctly loads `POOL_STATE` and `POOL_INFO` to return live pool data to the factory's internal oracle. The oracle can now successfully query pools for TWAP calculations.

---

## HIGH FINDINGS — ALL RESOLVED

### H-1 — Oracle Pool Selection Predictable to Validators ✅ MITIGATED
**File:** `factory/src/internal_bluechip_price_oracle.rs:87-117`
The hash now incorporates prior oracle state: `last_price`, `last_update`, and `twap_observations.len()`. These values are determined by the *previous* oracle update call and are unknown to a validator constructing the current block. While not as strong as a VRF, this raises the bar significantly — an attacker must now control both block production *and* have predicted the previous oracle update's output. For the expected threat model, this is acceptable.

### H-2 — `calculate_unclaimed_fees` Returns `Uint128::MAX` ✅ FIXED
Now returns `StdResult<Uint128>` with proper error propagation.

### H-3 — Emergency Withdraw No Timelock ✅ FIXED
**File:** `pool/src/contract.rs:1565-1700`
Emergency withdraw is now two-phase:
- **Phase 1** (initiate): Pauses pool, records `PENDING_EMERGENCY_WITHDRAW` with a 24-hour effective-after timestamp.
- **Phase 2** (execute): Only proceeds if timelock has elapsed.
- **Cancel**: `CancelEmergencyWithdraw` allows the factory admin to abort and unpause.

The 24-hour window gives LPs visibility before funds move. While shorter than the 48-hour config timelock, it's an appropriate trade-off for emergency scenarios.

**Remaining design limitation (not a bug):** LP position holders still have no on-chain claims mechanism post-withdrawal. The `EmergencyWithdrawalInfo` struct records amounts for off-chain reconciliation. This is an accepted trust assumption for V1.

### H-4 — Expand Economy Withdraw No Address Validation ✅ FIXED
**File:** `expand-economy/src/contract.rs:149-150`
`deps.api.addr_validate(&target)?` validates the recipient address.

### H-5 — O(n) Full Table Scan `query_positions_by_owner` ✅ FIXED
**File:** `pool/src/query.rs:327-352`
Uses `OWNER_POSITIONS.prefix(&owner_addr).range(...)` for O(log n) lookup.

### H-NEW-1 — `UpdateConfigFromFactory` LP Fee Unbounded ✅ FIXED
**File:** `pool/src/contract.rs:1485-1502`
Now enforces:
- Maximum: `Decimal::percent(10)` (10%)
- Minimum: `Decimal::permille(1)` (0.1%)

### H-NEW-2 — `ProposeConfigUpdate` No Address Validation ✅ FIXED
**File:** `factory/src/execute.rs:138-146`
All address fields (`factory_admin_address`, `bluechip_wallet_address`, `atom_bluechip_anchor_pool_address`, `bluechip_mint_contract_address`) are validated via `deps.api.addr_validate()` before saving to `PENDING_CONFIG`.

---

## MEDIUM FINDINGS

### M-1 through M-6 — All Previously Reported ✅ FIXED
- M-1: Factory instantiation validates addresses
- M-2: Pool instantiation circular check removed
- M-4: Minimum liquidity lock implemented (Uniswap V2 pattern)
- M-5: Distribution bounty paid from fee reserves
- M-6: Migration fee bounds at 10% max

### M-3 — TWAP Accumulator Uint128 Overflow ⚠️ ACCEPTABLE RISK
**File:** `pool/src/swap_helper.rs:104-113`
Uses `saturating_add` which prevents bricking. When saturated, TWAP delta collapses to zero and oracle falls back to spot price. The proper fix (Uint256 wrapping arithmetic) would provide better long-term accuracy but is not a correctness issue at the expected scale.

### M-NEW-1 — Factory `QueryMsg::Pool` Returns Stale Cached Data ⚠️ STILL OPEN
**File:** `factory/src/query.rs:31-36`, `factory/src/pool_creation_reply.rs:229-243`
`POOLS_BY_CONTRACT_ADDRESS` is populated once at pool creation with zeroed reserves and never updated. The factory's `QueryMsg::Pool` endpoint returns misleading data.

**Impact:** Front-end or third-party integrators querying the factory will see zero reserves for all pools. The internal oracle correctly queries pools directly via `PoolQueryMsg::GetPoolState`, so this does not affect pricing or commits.

**Recommendation:** Deprecate this endpoint with documentation directing callers to query pool contracts directly, or proxy the query to the pool's live state.

### M-NEW-2 — TWAP Window Strict `>` Discards Boundary Observations ⚠️ STILL OPEN
**File:** `factory/src/internal_bluechip_price_oracle.rs:264-268`
```rust
.retain(|obs| obs.timestamp > cutoff_time);  // should be >=
```
An observation at exactly `cutoff_time` is pruned. When the oracle is updated at exactly 1-hour intervals, this reduces the TWAP to a single-point price, degrading manipulation resistance.

**Impact:** Low probability in practice since block times are not exact. The 5-minute `UPDATE_INTERVAL` means observations typically accumulate well within the 3600s window.

**Recommendation:** Change `>` to `>=`.

### M-NEW-3 — Oracle Falls Back to Spot Price After Pool Rotation 🟡 NEW
**File:** `factory/src/internal_bluechip_price_oracle.rs:401-408`
When a pool has no previous cumulative snapshot (cleared after rotation at line 247), the oracle falls back to raw spot price from reserves:
```rust
// No previous snapshot — first observation, use spot price as baseline
calculate_price_from_reserves(bluechip_reserve, other_reserve)?
```
This creates a one-update window after each pool rotation where spot-price manipulation via large swaps can influence the oracle. The TWAP weighting across multiple pools and the ATOM anchor pool partially mitigate this, but the first post-rotation observation for any newly-selected pool is unprotected.

**Impact:** An attacker who manipulates reserves in a newly-selected pool and triggers `UpdateOraclePrice` in the same block can inject a manipulated price into the TWAP window. The damage is bounded by the pool's liquidity weight and the presence of other TWAP-protected pools.

**Recommendation:** Skip pools without a prior cumulative snapshot for price calculation (use them only to establish a baseline), or require 2+ observation cycles before including a pool's price in the weighted average.

### M-NEW-4 — Pyth Confidence Interval Not Validated 🟡 NEW
**File:** `factory/src/internal_bluechip_price_oracle.rs:570-594`
The Pyth price response includes a `conf` (confidence interval) field that is available but never checked. During high volatility or low oracle participation, the reported price can have a very wide confidence band. The contract uses the point estimate unconditionally.

**Impact:** During market turbulence, the oracle may accept a Pyth price with a 20%+ confidence interval, leading to inaccurate USD valuations for commits.

**Recommendation:** Reject prices where `conf * PRICE_PRECISION / price > threshold` (e.g., 5%).

### M-NEW-5 — `SETCOMMIT` Key Collision for Multi-Pool Creators 🟡 NEW
**File:** `factory/src/pool_creation_reply.rs:202-206`
`SETCOMMIT` is keyed by creator wallet address. If the same creator creates multiple pools, each subsequent pool **overwrites** the commit info for all prior pools by that creator:
```rust
SETCOMMIT.save(deps.storage, &pool_context.temp_creator_wallet.to_string(), &commit_info)?;
```

**Impact:** The `SETCOMMIT` mapping for the creator's first pool is silently lost when they create a second pool. Any logic depending on this state for the earlier pool will read the wrong pool's commit info.

**Recommendation:** Key `SETCOMMIT` by `pool_id` or a composite key of `(creator_addr, pool_id)`.

---

## LOW FINDINGS

### L-1 — Hardcoded `"ubluechip"` Denom ⚠️ STILL OPEN
**Files:** `expand-economy/src/contract.rs:87`, `factory/src/mint_bluechips_pool_creation.rs:98`
Both locations hardcode `"ubluechip"`. Not a bug for the intended deployment chain but limits portability.

### L-NEW-1 — `query_token_balance` Silently Returns Zero on Error ⚠️ STILL OPEN (both contracts)
**Files:** `factory/src/query.rs:64-73`, `pool/src/asset.rs:341-343`
```rust
.unwrap_or_else(|_| Cw20BalanceResponse { balance: Uint128::zero() })
```
This pattern exists in both the factory and pool contracts. Any CW20 query failure is silently swallowed, masking integration bugs. Neither instance is security-critical since the callers handle zero balances gracefully, but it makes debugging harder.

### L-5 — `ContinuePoolUpgrade` Attribute Off-by-One ⚠️ STILL OPEN
**File:** `factory/src/execute.rs:450-453`
`messages.len()` includes the recursive `ContinuePoolUpgrade` message, over-counting the actual migrations in the batch by 1. Purely cosmetic — affects event metadata only.

### L-NEW-6 — `get_eligible_creator_pools` Linear Scan for `is_bluechip_second` Determination
**File:** `factory/src/internal_bluechip_price_oracle.rs:332-344`
For each pool in the oracle update, `POOLS_BY_ID` is scanned linearly to find the matching pool and determine token ordering. With many pools, this multiplies gas costs. Not a correctness issue but could hit gas limits at scale.

**Recommendation:** Store the token ordering in `POOLS_BY_CONTRACT_ADDRESS` at creation time.

### L-NEW-7 — `execute_force_rotate_pools` Does Not Clear Cumulative Snapshots
**File:** `factory/src/internal_bluechip_price_oracle.rs:800-821`
When the admin force-rotates oracle pools, `pool_cumulative_snapshots` is not cleared. If rotation introduces new pools, stale snapshots from prior pools may persist. The `update_internal_oracle_price` path correctly clears snapshots on rotation (line 247), but the admin force-rotation path does not.

**Impact:** First oracle update after force rotation may use incorrect cumulative deltas for pools that happened to share addresses with previously-snapshotted pools (unlikely in practice but theoretically possible).

**Recommendation:** Add `oracle.pool_cumulative_snapshots = vec![];` to `execute_force_rotate_pools`.

### L-NEW-8 — Factory Migration `CONTRACT_NAME` Mismatch
**File:** `factory/src/execute.rs:23` vs `factory/src/migrate.rs:7`
`instantiate` writes `"crates.io:factory"` via `cw2::set_contract_version`, but `migrate` writes `"crates.io:bluechip-factory"`. After migration, the contract name changes. Future migrations that check the stored contract name would fail unexpectedly.

**Recommendation:** Use a single shared `CONTRACT_NAME` constant.

### L-NEW-9 — Pool Selection Hash Uses Overlapping Byte Windows
**File:** `factory/src/internal_bluechip_price_oracle.rs:123-133`
The pool selection loop extracts 8-byte seeds from overlapping regions of the SHA256 hash (`hash[i..i+7]`, `hash[i+1..i+8]`, etc.). With `ORACLE_POOL_COUNT = 5` (4 random pools needed), the byte windows overlap by 7 bytes, creating correlated selections. This slightly reduces the effective randomness of pool selection.

**Impact:** Marginal. The `used_indices` deduplication (line 137) prevents duplicate selection, and the eligible pool set is typically small enough that the correlation is not exploitable.

**Recommendation:** Use non-overlapping byte ranges: `hash[i*8..(i+1)*8]`.

---

## FULL STATUS TABLE — ALL FINDINGS

| ID | Title | Severity | Status |
|----|-------|----------|--------|
| C-1 | Post-threshold commit double fee deduction | CRITICAL | ✅ FIXED |
| C-2 | Split-commit excess wrong denominator | CRITICAL | ✅ FIXED |
| C-3 | Reentrancy guard stuck state | CRITICAL | ✅ FIXED |
| C-NEW-1 | `query_for_factory` dead code — oracle broken | CRITICAL | ✅ FIXED |
| H-1 | Oracle pool selection predictable to validators | HIGH | ✅ MITIGATED |
| H-2 | `calculate_unclaimed_fees` returns `Uint128::MAX` | HIGH | ✅ FIXED |
| H-3 | Emergency withdraw no timelock or LP recovery | HIGH | ✅ FIXED (24h timelock) |
| H-4 | Expand Economy withdraw no address validation | HIGH | ✅ FIXED |
| H-5 | O(n) full table scan `query_positions_by_owner` | HIGH | ✅ FIXED |
| H-NEW-1 | `UpdateConfigFromFactory` LP fee unbounded | HIGH | ✅ FIXED (0.1%–10%) |
| H-NEW-2 | `ProposeConfigUpdate` no address validation | HIGH | ✅ FIXED |
| M-1 | Factory instantiation unvalidated addresses | MEDIUM | ✅ FIXED |
| M-2 | Pool instantiation circular factory check | MEDIUM | ✅ FIXED |
| M-3 | TWAP accumulator Uint128 overflow | MEDIUM | ⚠️ ACCEPTABLE |
| M-4 | No minimum liquidity lock | MEDIUM | ✅ FIXED |
| M-5 | Distribution bounty self-funded | MEDIUM | ✅ FIXED |
| M-6 | Migration fee bounds missing | MEDIUM | ✅ FIXED |
| M-NEW-1 | Factory `query_pool` returns stale zeroed data | MEDIUM | 🟡 OPEN |
| M-NEW-2 | TWAP window strict `>` discards boundary | MEDIUM | 🟡 OPEN |
| M-NEW-3 | Oracle spot-price fallback after rotation | MEDIUM | 🟡 NEW |
| M-NEW-4 | Pyth confidence interval not validated | MEDIUM | 🟡 NEW |
| M-NEW-5 | `SETCOMMIT` key collision for multi-pool creators | MEDIUM | 🟡 NEW |
| L-1 | Hardcoded `"ubluechip"` denom | LOW | 🟡 OPEN |
| L-NEW-1 | `query_token_balance` swallows errors silently | LOW | 🟡 OPEN |
| L-5 | `ContinuePoolUpgrade` attribute off-by-one | LOW | 🟡 OPEN |
| L-NEW-6 | Linear scan in `is_bluechip_second` | LOW | 🟡 NEW |
| L-NEW-7 | Force-rotate doesn't clear snapshots | LOW | 🟡 NEW |
| L-NEW-8 | Factory migration `CONTRACT_NAME` mismatch | LOW | 🟡 NEW |
| L-NEW-9 | Pool selection hash overlapping byte windows | LOW | 🟡 NEW |

---

## ARCHITECTURE & DESIGN ASSESSMENT

### Positive Security Properties

The following security properties are correctly implemented and verified:

1. **Constant-product AMM (x*y=k)** — Swap math uses `Uint256` intermediate precision, avoiding truncation errors.
2. **Minimum liquidity lock** — Uniswap V2-style `integer_sqrt(reserve0 * reserve1)` seed liquidity prevents first-depositor inflation attacks.
3. **Reentrancy guard** with admin-recoverable stuck state.
4. **Batched token distribution** — Threshold crossing triggers deferred distribution via cursor-based pagination, avoiding gas-limit exhaustion.
5. **48-hour timelocks** on factory config changes and pool upgrades.
6. **24-hour timelock** on emergency withdrawals with cancel mechanism.
7. **Double-mint prevention** via `POOL_THRESHOLD_MINTED` set-before-execute pattern.
8. **Hardcoded threshold payout validation** — All four allocation amounts are checked against exact constants; total is cross-validated.
9. **Rate limiting** — 13-second per-user commit interval prevents spam.
10. **Pool creation cleanup** — Failed sub-messages trigger NFT/CW20 ownership revert.
11. **Oracle TWAP with cumulative accumulators** — Follows Uniswap V2 pattern: accumulate before swap, delta-divide for manipulation resistance.
12. **Pyth staleness check** with cached fallback — Oracle queries Pyth live; caches the result for 2x staleness window.
13. **Pool pause mechanism** — Pools auto-pause on low liquidity and can be admin-paused.
14. **Expand-economy timelocked withdrawals** — Withdrawal requires 48-hour proposal/execute cycle.
15. **Position-based fee accounting** — Per-position `fee_growth_inside_*_last` with `unclaimed_fees` preservation on partial removals.
16. **Factory admin auth** — Consistent `assert_correct_factory_address` / factory sender checks across all privileged operations.

### Design Considerations (Not Bugs)

1. **Single admin key** — The factory admin (`factory_admin_address`) has broad powers: pool creation, config changes, oracle rotation, pool upgrades, emergency withdrawals. A compromised admin key, while now timelock-constrained, remains the single largest trust assumption. Consider transitioning to a multisig or governance module before mainnet TVL exceeds risk tolerance.

2. **Mock mode detection** — `atom_bluechip_anchor_pool_address == factory_admin_address` triggers mock/local-testing mode in multiple code paths (oracle bypass, mint bypass). In production, these addresses MUST differ. Consider adding an explicit `is_testnet: bool` config field to make this intent clearer.

3. **LP recovery post-emergency** — Emergency withdrawal records `EmergencyWithdrawalInfo` but provides no on-chain claims mechanism. LPs must trust the protocol team for off-chain reconciliation. This is an accepted V1 trade-off but should be documented clearly for users.

4. **`mockoracle` contract** — Present in the workspace but marked for testing only. It has no access control on `SetPrice` and must NEVER be deployed as the Pyth oracle in production.

---

## BUILD & DEPLOYMENT READINESS

| Criterion | Status |
|-----------|--------|
| `cargo build` compiles | ✅ |
| `cargo test` passes | ✅ (pool, factory, expand-economy) |
| Optimizer script present (`optimize.sh`) | ✅ |
| `cw2` contract versioning | ✅ (pool, factory, expand-economy) |
| Migration support | ✅ (`MigrateMsg` with `UpdateFees`, `UpdateVersion`) |
| No `unwrap()` in production paths | ✅ (only in test code) |
| No `panic!()` in production paths | ✅ |
| Checked math throughout | ✅ |
| Admin key management | ⚠️ Single address — multisig recommended |

---

## RECOMMENDED ACTIONS BEFORE MAINNET

| Priority | ID | Action |
|----------|----|--------|
| **P1 — Recommended** | M-NEW-3 | Skip pools without prior cumulative snapshot in oracle price calc, or require 2+ cycles. |
| **P1 — Recommended** | M-NEW-5 | Fix `SETCOMMIT` key collision — use `pool_id` or composite key. |
| **P1 — Recommended** | M-NEW-2 | Change `>` to `>=` in TWAP observation retention (1-line fix). |
| **P1 — Recommended** | L-NEW-7 | Clear `pool_cumulative_snapshots` in `execute_force_rotate_pools`. |
| **P1 — Recommended** | L-NEW-8 | Unify `CONTRACT_NAME` across `execute.rs` and `migrate.rs`. |
| **P2 — Advisory** | M-NEW-4 | Add Pyth confidence interval validation (reject conf/price > 5%). |
| **P2 — Advisory** | M-NEW-1 | Deprecate or fix factory `QueryMsg::Pool` stale data endpoint. |
| **P2 — Advisory** | L-1 | Parameterize `"ubluechip"` denom for multi-chain portability. |
| **P3 — Cosmetic** | L-5 | Fix off-by-one in upgrade batch attribute. |
| **P3 — Cosmetic** | L-NEW-9 | Use non-overlapping byte ranges in pool selection hash. |
| **Operational** | — | Ensure `atom_bluechip_anchor_pool_address != factory_admin_address` in production config. |
| **Operational** | — | Do NOT deploy `mockoracle` on mainnet. |
| **Operational** | — | Use a multisig for `factory_admin_address`. |

---

## CONCLUSION

The codebase has undergone significant improvement since the initial audit. All 4 Critical and all 7 High-severity findings have been resolved. The remaining 5 Medium and 7 Low items are non-blocking for a carefully monitored production deployment, though several P1 items should be addressed first.

The contract architecture is sound: constant-product AMM math, TWAP oracle with cumulative accumulators, timelocked admin operations, reentrancy protection, and batched distribution all follow well-established patterns. The code is well-structured, consistently uses checked arithmetic, and has reasonable test coverage.

The most notable new findings are the oracle's spot-price fallback after pool rotation (M-NEW-3), the `SETCOMMIT` key collision for multi-pool creators (M-NEW-5), and the unchecked Pyth confidence interval (M-NEW-4). None are Critical, but M-NEW-3 and M-NEW-5 should be addressed before mainnet.

**Conditional pass for production deployment**, subject to:
1. Addressing the P1 recommended fixes (M-NEW-3, M-NEW-5, M-NEW-2, L-NEW-7, L-NEW-8).
2. Ensuring operational deployment practices (multisig admin, no mock oracle, correct atom pool address).
3. Monitoring oracle health and distribution completion post-launch.

---

*This report reflects a manual source-code review of the working tree dated 2026-02-24. It does not constitute a formal security certification. All findings must be independently verified by the development team before production deployment.*
