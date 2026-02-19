# Bluechip Contracts — Smart Contract Audit Report

**Audit Date:** 2026-02-19
**Auditor:** Independent CosmWasm Security Review
**Codebase Commit:** Current working tree
**Prior Report:** 2026-02-13 (Claude Opus 4.6 Automated Audit)
**Verdict:** ❌ NOT READY FOR PRODUCTION

---

## Executive Summary

This is a **re-audit** of the bluechip-contracts codebase, conducted as a full independent review of all Rust source files across the pool, factory, expand-economy, and shared packages. The three previously-reported Critical issues (C-1, C-2, C-3) have been resolved. However, a new **Critical** issue was discovered that fundamentally breaks the protocol in production: a dead-code query routing bug prevents the factory oracle from ever reading pool state. Until this is fixed, the oracle cannot be updated, and all commit transactions will fail.

Additionally, one High-severity issue from the prior report (H-1) remains unaddressed, and new High-severity findings have been identified.

---

## Verdict Summary

| Severity | Prior Report Open | Newly Resolved | Still Open | New Findings | Net Open |
|----------|-----------------|----------------|------------|--------------|----------|
| Critical | 3 | 3 fixed | 0 | **1 new** | **1** |
| High | 5 | 3 fixed, 1 partial | 2 | **2 new** | **4** |
| Medium | 6 | 5 fixed, 1 partial | 1 | **2 new** | **3** |
| Low | 8 | 6 fixed | 2 | **4 new** | **6** |

---

## CRITICAL FINDINGS

---

### C-NEW-1 — `query_for_factory` is Dead Code: Oracle Queries Fail in Production

**File:** `pool/src/query.rs:427`, `factory/src/internal_bluechip_price_oracle.rs:180`
**Status:** 🔴 NEW — UNRESOLVED
**Impact:** Production system is non-functional; the oracle can never be updated; all commit operations fail with "TWAP price is zero".

#### Description

The pool contract defines `query_for_factory` at `query.rs:427`, a function that handles `PoolQueryMsg::GetPoolState` and `PoolQueryMsg::GetAllPools` — the exact messages the factory's internal oracle sends to read pool reserves for TWAP calculation:

```rust
// factory/src/internal_bluechip_price_oracle.rs:180
let pool_state: PoolStateResponseForFactory = deps.querier.query_wasm_smart(
    pool_address.to_string(),
    &PoolQueryMsg::GetPoolState {
        pool_contract_address: pool_address.to_string(),
    },
)?;
```

However, `query_for_factory` is **never called** from the pool's `query` entry point:

```rust
// pool/src/query.rs:25 — the ONLY entry point for the pool contract
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Pair {} => ...
        QueryMsg::PoolState {} => ...
        // PoolQueryMsg::GetPoolState is NOT handled here — query_for_factory is unreachable
    }
}
```

`PoolQueryMsg::GetPoolState { pool_contract_address: "..." }` serializes (via `#[cw_serde]` / snake_case) to:
```json
{"get_pool_state": {"pool_contract_address": "..."}}
```

The pool's entry point attempts to deserialize the query into `QueryMsg`, which has no `GetPoolState` variant. The deserialization **fails with a JSON unknown-field error** for every oracle query issued by the factory.

#### Production Impact Chain

1. `get_eligible_creator_pools` queries each pool via `PoolQueryMsg::GetPoolState` — **always fails** → propagates error upward; no eligible creator pools returned.
2. `calculate_weighted_price_with_atom` queries the ATOM anchor pool the same way — **always fails** → `has_atom_pool` remains `false`.
3. Returns `Err("ATOM pool price could not be calculated")`.
4. `update_internal_oracle_price` fails on every invocation → **the oracle is never successfully updated**.
5. `oracle.bluechip_price_cache.last_price` remains `Uint128::zero()` indefinitely.
6. `get_bluechip_usd_price` returns `Err("TWAP price is zero - oracle may need update")`.
7. `get_usd_value_with_staleness_check` (called on every commit) always fails.
8. **ALL COMMIT TRANSACTIONS FAIL** — the protocol is entirely non-functional in production.

The only working execution path is mock/test mode (`atom_bluechip_anchor_pool_address == factory_admin_address`), which bypasses the internal oracle entirely. This mode is documented as local-testing only and must never be used in production.

#### Root Cause

The `query_for_factory` function appears to be a refactoring artifact: it was written to handle factory-originated queries but never wired into the entry point dispatcher.

#### Recommended Fix

Add `PoolQueryMsg::GetPoolState` and `PoolQueryMsg::GetAllPools` handling to the pool's `query` entry point. The simplest approach:

```rust
// pool/src/query.rs — extend the entry point
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    // First try QueryMsg; if deserialization fails, try PoolQueryMsg
    ...
}
```

Or add a wrapper variant to `QueryMsg` that delegates to `query_for_factory`.

---

### C-1 — Post-Threshold Commit Fee Double Deduction ✅ FIXED

**File:** `pool/src/contract.rs:1064–1067, 1311–1314`
Prior Status: CRITICAL → **RESOLVED**

The fix correctly subtracts both `return_amt` and `commission_amt` from the ask reserve (`ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?`) in both the threshold-crossing split-commit path and the post-threshold commit swap path. The `C-1 FIX` comment is present at both locations.

---

### C-2 — Split-Commit Excess Uses Wrong Fee Denominator ✅ FIXED

**File:** `pool/src/contract.rs:964–975`
Prior Status: CRITICAL → **RESOLVED**

The fix correctly computes the post-fee split using proportional division against the original `amount` (not re-applying the fee rate to the excess):
```rust
let threshold_portion_after_fees = amount_after_fees.multiply_ratio(bluechip_to_threshold, amount);
let effective_bluechip_excess = amount_after_fees.checked_sub(threshold_portion_after_fees)?;
```

---

### C-3 — Reentrancy Guard Stuck State ✅ FIXED

**File:** `pool/src/contract.rs:474–486`
Prior Status: CRITICAL → **RESOLVED**

A `RecoveryType::StuckReentrancyGuard` variant and `recover_reentrancy_guard` function now allow the factory admin to reset a stuck `RATE_LIMIT_GUARD` via `RecoverStuckStates`.

---

## HIGH FINDINGS

---

### H-1 — Oracle Pool Selection Is Predictable to Validators

**File:** `factory/src/internal_bluechip_price_oracle.rs:81–113`
**Status:** 🔴 STILL OPEN (unchanged from prior report)

Pool selection in `select_random_pools_with_atom` uses `SHA256(block_time || block_height || chain_id)`. Both `block_time` and `block_height` are known to validators before they produce a block, making the pool selection deterministic and manipulable. A colluding validator can time transactions or selectively include/exclude blocks to influence which pools dominate the oracle sample during a target window.

**Recommendation:** Use a verifiable random function (VRF), commit-reveal scheme, or incorporate the previous block's hash (which validators cannot predict when producing the current block).

---

### H-2 — `calculate_unclaimed_fees` Returns `Uint128::MAX` ✅ FIXED

Prior Status: HIGH → **RESOLVED** (function now returns `StdResult<Uint128>` and propagates errors).

---

### H-3 — Emergency Withdraw Has No Timelock or LP Recovery Path ⚠️ PARTIALLY FIXED

**File:** `pool/src/contract.rs:1537–1619`
Prior Status: HIGH → **PARTIALLY MITIGATED**

**What was fixed:**
- Funds now route to `bluechip_wallet_address` (a protocol-controlled address from `COMMITFEEINFO`) instead of the raw transaction sender, reducing risk from a one-off admin key compromise.
- An `EmergencyWithdrawalInfo` struct is written to state at withdrawal time for LP audit purposes.

**What remains unfixed:**
1. **No timelock.** The factory admin can drain all pool funds in a single transaction with zero delay. Config changes require 48 hours; pool upgrades require 48 hours; fund withdrawal requires nothing.
2. **No LP claims mechanism.** After emergency withdrawal, LP position NFT holders have no contract-enforced path to recover their proportional share. The on-chain `EmergencyWithdrawalInfo` is purely informational.
3. **Irreversible and total.** The withdrawal drains all reserves, fee reserves, and creator excess position atomically. There is no partial or phased option.

**Recommendation:** Add a minimum timelock (matching the 48h config timelock). Implement an LP claims path that allows NFT holders to burn their position post-withdrawal and receive `(position_liquidity / total_liquidity_at_withdrawal) * withdrawn_amount` of each asset.

---

### H-4 — Expand Economy Withdraw No Address Validation ✅ FIXED

Prior Status: HIGH → **RESOLVED** (`deps.api.addr_validate(&target)?` added at `expand-economy/src/contract.rs:143`).

---

### H-5 — O(n) Full Table Scan in `query_positions_by_owner` ✅ FIXED

Prior Status: HIGH → **RESOLVED** (secondary `OWNER_POSITIONS: Map<(&Addr, &str), bool>` index added; `query_positions_by_owner` now uses `OWNER_POSITIONS.prefix(&owner_addr).range(...)` for O(log n) lookup).

---

### H-NEW-1 — `UpdateConfigFromFactory` Allows Unbounded LP Fee (0–99.99%)

**File:** `pool/src/contract.rs:1477–1488`
**Status:** 🔴 NEW — UNRESOLVED

The `execute_update_config_from_factory` handler only rejects fees `>= 100%`:
```rust
if let Some(fee) = update.lp_fee {
    if fee >= Decimal::one() {  // only rejects ≥ 100%
        return Err(...);
    }
    POOL_SPECS.update(...);
}
```

The migration handler correctly caps fees at 10% (`M-6 FIX` in `migrate()`). But `UpdateConfigFromFactory` — callable by the factory admin at any time with no timelock — allows setting LP fee anywhere from 0% to 99.99%. Setting the fee near 100% would route almost every swap's output to `fee_reserve`, effectively stealing from traders. Setting it to 0% removes all LP fee incentives. This bypasses the 10% safety cap introduced in the prior audit fix cycle.

**Recommendation:** Apply the same `Decimal::percent(10)` upper bound in `execute_update_config_from_factory`.

---

### H-NEW-2 — `ProposeConfigUpdate` Does Not Validate Proposed Config Addresses

**File:** `factory/src/execute.rs:131–147`
**Status:** 🟠 NEW — UNRESOLVED

`execute_propose_factory_config_update` saves a new `FactoryInstantiate` struct to `PENDING_CONFIG` without validating any of its address fields:
```rust
pub fn execute_propose_factory_config_update(..., config: FactoryInstantiate) {
    assert_correct_factory_address(deps.as_ref(), info)?;
    let pending = PendingConfig {
        new_config: config,  // no addr_validate calls
        effective_after: env.block.time.plus_seconds(86400 * 2),
    };
    PENDING_CONFIG.save(deps.storage, &pending)?;
}
```

The M-1 fix added address validation to `instantiate` only. If an admin proposes a config containing a malformed or incorrect Bech32 address (e.g., wrong chain prefix, typo), the 48-hour countdown begins. After the timelock expires, `execute_update_factory_config` writes the unvalidated config to `FACTORYINSTANTIATEINFO`. Subsequent operations that load this state — pool creation, oracle queries, fee routing — may fail with cryptic errors or route funds to an unintended address.

**Recommendation:** Duplicate the address-validation block from `instantiate` (lines 55–60) into `execute_propose_factory_config_update`.

---

## MEDIUM FINDINGS

---

### M-1 — Factory Instantiation Unvalidated Addresses ✅ FIXED
### M-2 — Pool Instantiation Circular Factory Check ✅ FIXED
### M-4 — No Minimum Liquidity Lock ✅ FIXED
### M-5 — Distribution Bounty Self-Funded ✅ FIXED
### M-6 — Migration Fee Bounds Missing ✅ FIXED (via `migrate()` only — see H-NEW-1)

---

### M-3 — TWAP Accumulator Uint128 Overflow ⚠️ PARTIALLY FIXED

**File:** `pool/src/swap_helper.rs:104–113`
Prior Status: MEDIUM → **PARTIALLY MITIGATED**

`saturating_add` (instead of `checked_add`) prevents the pool from becoming permanently bricked when accumulators overflow. When an accumulator saturates at `Uint128::MAX`, the delta between consecutive snapshots collapses toward zero, causing the oracle to fall back to spot price for that pool — the conservative path. The proper fix (using `Uint256` as Uniswap V2 does, relying on wrapping arithmetic for delta computation) would eliminate precision loss for high-volume long-running pools. At the expected scale this is acceptable, but should be tracked.

---

### M-NEW-1 — Factory `QueryMsg::Pool` Returns Permanently Stale Cached Data

**File:** `factory/src/query.rs:31–36`, `factory/src/pool_creation_reply.rs:229–243`
**Status:** 🟡 NEW — UNRESOLVED

`QueryMsg::Pool { pool_address }` in the factory reads from `POOLS_BY_CONTRACT_ADDRESS`, which is populated once at pool creation with all-zero reserves and **never updated**:

```rust
// pool_creation_reply.rs — written once at finalization
POOLS_BY_CONTRACT_ADDRESS.save(deps.storage, pool_address.clone(),
    &PoolStateResponseForFactory {
        reserve0: Uint128::zero(),
        reserve1: Uint128::zero(),
        total_liquidity: Uint128::zero(),
        block_time_last: 0,
        ...
    },
)?;
```

Any integrator, front-end, or protocol querying `factory::QueryMsg::Pool` will always receive zeroed reserve data, regardless of actual trading activity. This misleads users and third-party contracts about pool health and liquidity.

**Recommendation:** Either deprecate `QueryMsg::Pool` and direct callers to query each pool contract directly, or sync `POOLS_BY_CONTRACT_ADDRESS` from the pool's live state on updates.

---

### M-NEW-2 — TWAP Window Uses Strict `>` Comparison, Discarding Boundary Observations

**File:** `factory/src/internal_bluechip_price_oracle.rs:229–233`
**Status:** 🟡 NEW — UNRESOLVED

```rust
let cutoff_time = current_time.saturating_sub(TWAP_WINDOW);  // TWAP_WINDOW = 3600
oracle.bluechip_price_cache.twap_observations
    .retain(|obs| obs.timestamp > cutoff_time);  // strict >, not >=
```

An observation timestamped exactly at `cutoff_time` is discarded. In the edge case where the oracle is updated at exactly 1-hour intervals, the oldest anchor is always pruned, leaving the TWAP based on only the most recent observation — collapsing it to a point price. This degrades the manipulation-resistance that the TWAP window is designed to provide.

**Recommendation:** Change `obs.timestamp > cutoff_time` to `obs.timestamp >= cutoff_time`.

---

## LOW FINDINGS

---

### L-1 — Hardcoded `"ubluechip"` Denom in Two Contract Locations ⚠️ STILL OPEN

**Files:** `expand-economy/src/contract.rs:84`, `factory/src/mint_bluechips_pool_creation.rs:98`

Both files hardcode `"ubluechip"` as the native token denom. If deployed to a different environment, these will fail silently or send funds to a non-existent denomination.

**Recommendation:** Add a `bluechip_native_denom: String` field to `FactoryInstantiate` and pass it through to both locations.

---

### L-2 — `query_token_balance` Silently Returns Zero on CW20 Query Error

**File:** `factory/src/query.rs:64–76`
**Status:** 🟡 NEW

```rust
.unwrap_or_else(|_| Cw20BalanceResponse { balance: Uint128::zero() })
```

Any CW20 query failure (wrong address, paused contract, out-of-gas) silently returns balance `0`. Callers cannot distinguish "balance is zero" from "query failed." This masks integration bugs and is inconsistent with how other query functions in the file handle errors.

---

### L-3 — Emergency Withdraw Has No Timelock

**File:** `pool/src/contract.rs:1537`
**Status:** 🟡 NEW

Admin operations with financial impact have the following delays: config changes = 48h, pool upgrades = 48h, emergency withdraw = **0h**. A compromised admin key or malicious governance action can drain all funds in one block.

---

### L-4 — `lp_fee` and `min_commit_interval` Have No Lower Bounds in `UpdateConfigFromFactory`

**File:** `pool/src/contract.rs:1477–1505`
**Status:** 🟡 NEW

`lp_fee` can be set to `0` (removing LP revenue), and `min_commit_interval` can be set to `0` (disabling the spam rate-limiter entirely). Both are sensitive parameters that should have on-chain minimum values enforced.

---

### L-5 — `ContinuePoolUpgrade` "upgraded_in_batch" Attribute Is Off-by-One

**File:** `factory/src/execute.rs:432–444`
**Status:** 🟡 NEW

When more pools remain, a recursive `ContinuePoolUpgrade` message is appended to `messages`. The event attribute `upgraded_in_batch` then reports `messages.len()` which includes the recursive call, over-reporting the actual number of pools upgraded in the current batch by one.

---

### L-6 — `get_usd_value` (No Staleness Check) Used Alongside Stale-Checked Variant

**File:** `pool/src/swap_helper.rs:150–161`
**Status:** 🟡 NEW

`get_usd_value` skips the 600-second staleness check applied in `get_usd_value_with_staleness_check`. Any future caller of `get_usd_value` could silently act on stale oracle data. Prefer the stale-checked variant universally, or add a doc comment explaining intentional exclusion.

---

## FULL STATUS TABLE — ALL FINDINGS

| ID | Title | Severity | Status |
|----|-------|----------|--------|
| C-1 | Post-threshold commit double fee deduction | CRITICAL | ✅ FIXED |
| C-2 | Split-commit excess wrong denominator | CRITICAL | ✅ FIXED |
| C-3 | Reentrancy guard stuck state | CRITICAL | ✅ FIXED |
| C-NEW-1 | `query_for_factory` dead code — oracle broken in production | CRITICAL | 🔴 OPEN |
| H-1 | Oracle pool selection predictable to validators | HIGH | 🔴 OPEN |
| H-2 | `calculate_unclaimed_fees` returns `Uint128::MAX` | HIGH | ✅ FIXED |
| H-3 | Emergency withdraw no LP recovery mechanism | HIGH | ⚠️ PARTIAL |
| H-4 | Expand Economy withdraw no address validation | HIGH | ✅ FIXED |
| H-5 | O(n) full table scan `query_positions_by_owner` | HIGH | ✅ FIXED |
| H-NEW-1 | `UpdateConfigFromFactory` LP fee unbounded (0–99.99%) | HIGH | 🔴 OPEN |
| H-NEW-2 | `ProposeConfigUpdate` no address validation | HIGH | 🔴 OPEN |
| M-1 | Factory instantiation unvalidated addresses | MEDIUM | ✅ FIXED |
| M-2 | Pool instantiation circular factory check | MEDIUM | ✅ FIXED |
| M-3 | TWAP accumulator Uint128 overflow | MEDIUM | ⚠️ PARTIAL |
| M-4 | No minimum liquidity lock | MEDIUM | ✅ FIXED |
| M-5 | Distribution bounty self-funded | MEDIUM | ✅ FIXED |
| M-6 | Migration fee bounds missing | MEDIUM | ✅ FIXED |
| M-NEW-1 | Factory `query_pool` returns stale zeroed data | MEDIUM | 🔴 OPEN |
| M-NEW-2 | TWAP window strict `>` discards boundary observation | MEDIUM | 🔴 OPEN |
| L-1 | Hardcoded `"ubluechip"` denom | LOW | 🔴 OPEN |
| L-2 (prev) | Unused `POOLS` map | LOW | ✅ FIXED |
| L-3 (prev) | Debug logging in production | LOW | ✅ FIXED |
| L-4 (prev) | Position ID format inconsistency | LOW | ✅ FIXED |
| L-5 (prev) | Missing input validation commit interval | LOW | ✅ FIXED |
| L-6 (prev) | `query_pool_commiters` double-counts | LOW | ✅ FIXED |
| L-7 (prev) | Position key format inconsistency | LOW | ✅ FIXED |
| L-8 (prev) | `get_eligible_creator_pools` double-iterates storage | LOW | ✅ FIXED |
| L-NEW-1 | `query_token_balance` swallows errors silently | LOW | 🔴 OPEN |
| L-NEW-2 | Emergency withdraw no timelock | LOW | 🔴 OPEN |
| L-NEW-3 | `lp_fee`/`min_commit_interval` no lower bound | LOW | 🔴 OPEN |
| L-NEW-4 | `ContinuePoolUpgrade` attribute off-by-one | LOW | 🔴 OPEN |
| L-NEW-5 | `get_usd_value` skips staleness check | LOW | 🔴 OPEN |

---

## RECOMMENDED FIX PRIORITY

Before any production deployment, the following must be resolved:

| Priority | ID | Description |
|----------|----|-------------|
| **P0 — Blocking** | C-NEW-1 | Connect `query_for_factory` to the pool query entry point. Without this, the oracle never updates and all commits fail. |
| **P0 — Blocking** | H-NEW-1 | Apply the 10% LP fee cap in `execute_update_config_from_factory`. |
| **P0 — Blocking** | H-3 | Add timelock and LP claims mechanism to emergency withdraw. |
| **P1 — High** | H-1 | Replace predictable SHA256(block_time\|\|height) with unpredictable entropy for oracle pool selection. |
| **P1 — High** | H-NEW-2 | Add address validation to `ProposeConfigUpdate`. |
| **P2 — Medium** | M-NEW-1 | Fix or remove the stale-data `query_pool` endpoint. |
| **P2 — Medium** | M-NEW-2 | Change strict `>` to `>=` in TWAP window retention. |
| **P3 — Low** | L-1 | Parameterize native denom; remove hardcoded `"ubluechip"`. |
| **P3 — Low** | L-NEW-1–5 | Remaining low-severity items. |

---

## POSITIVE OBSERVATIONS

The following security properties were confirmed as correctly implemented:

- **Minimum liquidity lock** (Uniswap V2 pattern) prevents first-depositor inflation attacks.
- **Reentrancy guard** on `simple_swap` with admin-callable reset path via `RecoverStuckStates`.
- **Batched token distribution** correctly handles large committer sets with pagination and gas budgeting.
- **48-hour timelocks** on factory config changes and pool upgrades are correctly implemented and enforced.
- **Double-mint prevention** via `POOL_THRESHOLD_MINTED` with flag set before execution prevents re-entrancy on threshold notification.
- **Threshold payment validation** hardcodes all expected token allocation amounts and cross-checks totals; prevents factory from minting unauthorized quantities.
- **Rate limiting** (13-second minimum commit interval per user) is correctly enforced with `USER_LAST_COMMIT`.
- **Pool creation cleanup** correctly reverts partial state when sub-messages fail mid-creation.
- **Post-threshold token distribution accounting** (`C-1 FIX`, `C-2 FIX`) is now mathematically correct.

---

*This report reflects a manual source-code review of the working tree dated 2026-02-19. It does not constitute a formal security certification. All findings must be independently verified by the development team before production deployment.*
