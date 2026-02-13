# Bluechip Contracts - Security Audit Report (v2)

**Date:** 2026-02-13
**Auditor:** Claude Opus 4.6 (Automated CosmWasm Smart Contract Audit)
**Scope:** All smart contracts in the bluechip-contracts workspace
**Contracts Audited:** Pool, Factory, Expand Economy, Mock Oracle, pool-factory-interfaces, easy-addr
**Test Results:** 103/103 tests passing
**Rust Toolchain:** 1.75.0, Target: wasm32-unknown-unknown
**CosmWasm:** 1.5.11

---

## Executive Summary

The Bluechip Contracts protocol is a decentralized creator economy platform built on CosmWasm. It implements a two-phase pool lifecycle (commit-then-trade), NFT-based liquidity positions, an internal TWAP oracle, and a token expansion mechanism. A prior audit identified Critical, High, and Medium findings; the majority have been fixed. This follow-up audit confirms those fixes and identifies **1 Critical**, **2 High**, **4 Medium**, and **8 Low/Informational** issues that remain or are newly discovered.

### Verdict: **NOT READY for production.** The critical reserve accounting bug (C-1) must be fixed before deployment. Two high-severity items also require attention.

---

## Severity Classification

| Severity | Description |
|----------|-------------|
| **Critical** | Direct loss of funds, contract bricking, or exploitable vulnerability |
| **High** | Significant security risk or logic error that could cause fund loss under specific conditions |
| **Medium** | Logic issues, missing validations, or design flaws that could cause unexpected behavior |
| **Low** | Code quality, gas optimization, or minor issues with limited impact |
| **Informational** | Best practices, suggestions, and observations |

---

## Status of Previously Reported Findings

| ID | Finding | Severity | Status |
|----|---------|----------|--------|
| C-2 | Double fee deduction on threshold-crossing excess | Critical | **FIXED** (proportional split at `contract.rs:975-981`) |
| C-3 | `RATE_LIMIT_GUARD` stuck on failure | Critical | **FIXED** (`RecoveryType::StuckReentrancyGuard` at `contract.rs:477-489`) |
| H-2 | `calculate_unclaimed_fees` returns `Uint128::MAX` on overflow | High | **FIXED** (now returns `StdError`, `liquidity_helpers.rs:27-29`) |
| H-3 | Emergency withdraw sends all funds to factory admin | High | **PARTIALLY FIXED** (sends to bluechip wallet, records `EmergencyWithdrawalInfo`; still no timelock or LP claims) |
| H-5 | `query_positions_by_owner` full table scan | High | **FIXED** (`OWNER_POSITIONS` secondary index at `state.rs:53`) |
| M-1 | Factory instantiation accepts unvalidated addresses | Medium | **FIXED** (`addr_validate` calls at `execute.rs:55-60`) |
| M-3 | TWAP accumulator overflow | Medium | **FIXED** (`saturating_add` at `swap_helper.rs:108-113`) |
| M-4 | No minimum liquidity lock for first depositor | Medium | **FIXED** (`MINIMUM_LIQUIDITY` subtracted at `liquidity_helpers.rs:131-140`) |
| M-5 | Distribution bounty is self-funded (no-op) | Medium | **FIXED** (bounty paid from `fee_reserve_0` at `contract.rs:1416-1431`) |
| M-6 | Migration has no fee bounds | Medium | **FIXED** (max 10% cap at `contract.rs:1446-1451`) |

---

## New & Remaining Findings

### C-1: [NEW] Commission Double-Counting in Post-Threshold Swap Reserve Accounting

**Location:** `pool/src/contract.rs:1070-1071` (threshold-crossing excess) and `pool/src/contract.rs:1315-1316` (`process_post_threshold_commit`)
**Severity:** Critical

**Description:**

In both post-threshold commit paths, the ask reserve is updated as:

```rust
// contract.rs:1070-1071 (threshold excess)
pool_state.reserve0 = offer_pool.checked_add(effective_bluechip_excess)?;
pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;

// contract.rs:1315-1316 (post-threshold commit)
pool_state.reserve0 = offer_pool.checked_add(swap_amount)?;
pool_state.reserve1 = ask_pool.checked_sub(return_amt)?;
```

Then `update_pool_fee_growth` is called, which adds `commission_amt` to `fee_reserve_1`:

```rust
// generic_helpers.rs:37-40
pool_fee_state.fee_reserve_1 = pool_fee_state.fee_reserve_1.checked_add(commission_amt)?;
```

However, `return_amt` from `compute_swap` is already **post-commission** (i.e., `original_return - commission`). Therefore `reserve1 = ask_pool - return_amt` still implicitly includes the commission tokens. Meanwhile `fee_reserve_1` also tracks those same commission tokens. This means **commission is double-counted** -- once in `reserve1` and once in `fee_reserve_1`.

Compare with the correct implementation in `execute_simple_swap` at `contract.rs:664`:

```rust
let ask_pool_post = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;
```

Here, both `return_amt` AND `commission_amt` are subtracted from the tradeable reserve, leaving commission exclusively in `fee_reserve`.

**Impact:** The pool's tracked balances (`reserve1 + fee_reserve_1`) exceed the actual CW20 token balance held by the contract by `commission_amt` per affected swap. Over time:
1. Fee reserves become phantom -- LP fee collection will eventually fail with insufficient CW20 balance
2. Tradeable reserves are inflated, giving subsequent traders slightly better prices at LPs' expense
3. Full liquidity removal may fail if the reserves exceed actual contract holdings

**Proof of Concept:**

Given a pool with `reserve0 = 1000, reserve1 = 1000`, LP fee 0.3%:
- Post-threshold commit of 100 bluechip
- `compute_swap(1000, 1000, 100, 0.003)` returns approximately `(90.6, 0.2, 0.3)` -- return=90.6, spread=0.2, commission=0.3
- **Current (buggy):** `reserve1 = 1000 - 90.6 = 909.4`, `fee_reserve_1 += 0.3` => tracked total = 909.7, actual CW20 balance = 909.4
- **Correct:** `reserve1 = 1000 - 90.6 - 0.3 = 909.1`, `fee_reserve_1 += 0.3` => tracked total = 909.4 = actual

**Recommendation:** Fix both locations to match the `execute_simple_swap` pattern:

```rust
// Fix contract.rs:1071
pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;

// Fix contract.rs:1316
pool_state.reserve1 = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;
```

---

### H-1: [REMAINING] Oracle Pool Selection Randomness is Predictable by Validators

**Location:** `factory/src/internal_bluechip_price_oracle.rs:81-112`
**Severity:** High

**Description:** The pool selection uses `SHA256(block_time || block_height || chain_id)` as a seed. All values are known to validators before block finalization. A colluding validator could manipulate block timestamps to influence which pools feed the oracle price.

**Impact:** A validator controlling low-liquidity pools could skew the oracle price, affecting all commit USD valuations.

**Recommendation:** Use all eligible pools weighted by liquidity rather than random selection, or use a VRF-based randomness source.

---

### H-2: [REMAINING] Expand Economy `execute_withdraw` Does Not Validate Recipient

**Location:** `expand-economy/src/contract.rs:141`
**Severity:** High

**Description:**

```rust
let target = recipient.unwrap_or_else(|| info.sender.to_string());
let send_msg = BankMsg::Send {
    to_address: target.clone(), // not validated
    ...
};
```

The `recipient` parameter is passed directly to `BankMsg::Send` without `deps.api.addr_validate()`. While CosmWasm will reject invalid bech32 addresses at the SDK level, malformed but valid-looking addresses could cause funds to be sent to the wrong destination. More importantly, this bypasses the standard validation pattern used everywhere else in the codebase.

**Recommendation:** Add `deps.api.addr_validate(&target)?` before using the address.

---

### M-1: [REMAINING] Pool Instantiation Factory Validation is Circular (Dead Code)

**Location:** `pool/src/contract.rs:62-73`
**Severity:** Medium

**Description:**

```rust
let cfg = ExpectedFactory {
    expected_factory_address: msg.used_factory_addr.clone(),
};
EXPECTED_FACTORY.save(deps.storage, &cfg)?;
let real_factory = EXPECTED_FACTORY.load(deps.storage)?;
validate_factory_address(
    &real_factory.expected_factory_address,
    &msg.used_factory_addr,
)?;
```

This saves the message's factory address, immediately loads it back, and validates it against itself. This is a tautological check. The real security is the `info.sender != real_factory.expected_factory_address` check on line 72, which works because pools are instantiated via SubMsg from the factory.

**Recommendation:** Remove the circular `validate_factory_address` call and the unnecessary load-after-save pattern.

---

### M-2: [NEW] `reply` Handler Has No-Op Pool Info Update

**Location:** `pool/src/contract.rs:565-567`
**Severity:** Medium (dead code / maintenance hazard)

**Description:**

```rust
POOL_INFO.update(deps.storage, |pool_info| -> Result<_, ContractError> {
    Ok(pool_info)
})?;
```

This loads `POOL_INFO` from storage and saves it back unchanged. It wastes gas and suggests an incomplete implementation. The reply handler for ID 42 appears to be vestigial code from an earlier LP token creation flow.

**Recommendation:** Remove the no-op update or implement the intended logic.

---

### M-3: [REMAINING] Emergency Withdrawal Lacks Timelock or LP Recourse

**Location:** `pool/src/contract.rs:1539-1628`
**Severity:** Medium

**Description:** While the fix sends funds to the protocol-controlled bluechip wallet and records `EmergencyWithdrawalInfo`, there is still:
1. No timelock delay before execution
2. No mechanism for LPs to claim proportional shares after withdrawal
3. Single-signer authorization (factory admin only)

**Impact:** The factory admin can instantly drain any pool. LPs have transparency but no recourse.

**Recommendation:** Add a timelock and/or multisig requirement. Consider a two-step emergency: first pause + announce, then withdraw after a delay.

---

### M-4: [NEW] `ContinueDistribution` is Permissionless but Could Be Griefed

**Location:** `pool/src/contract.rs:1390-1438`
**Severity:** Medium

**Description:** `execute_continue_distribution` is intentionally permissionless to allow anyone to drive distributions forward. However, the `consecutive_failures` counter increments on any batch failure, and after 5 failures the distribution is paused requiring admin recovery. An attacker could repeatedly trigger failures (e.g., by front-running with state changes) to force the distribution into a stuck state.

Additionally, the bounty is paid from `fee_reserve_0` which may be zero for newly-formed pools, providing no incentive for the first critical distribution batches.

**Recommendation:** Consider separating the failure counter from external calls, or only counting "real" failures (not front-run scenarios).

---

## Low & Informational Findings

### L-1: Unused State Items

**Location:** `pool/src/state.rs:60`

`POOLS: Map<&str, PoolState>` is declared but never populated. Dead code.

### L-2: Debug Logging in Production Code

**Location:** `pool/src/liquidity.rs:49-52, 114-117`

```rust
deps.api.debug(&format!("DEBUG_DEPOSIT: paid={}, actual0={}", paid_bluechip, actual_amount0));
```

These are no-ops in production but indicate incomplete cleanup and leak internal variable names.

### L-3: Position ID Format Inconsistency

**Location:** `pool/src/liquidity.rs:139` vs `pool/src/liquidity_helpers.rs:230`

`execute_deposit_liquidity` generates IDs like `"1"`, `"2"` while `execute_claim_creator_excess` generates IDs like `"position_1"`. Mixed formats could cause lookup failures if any code assumes a single format.

### L-4: `MAX_ORACLE_AGE` Constant Defined But Unused

**Location:** `pool/src/state.rs:20`

`pub const MAX_ORACLE_AGE: u64 = 3000000;` is defined but the actual staleness check uses `MAX_ORACLE_STALENESS_SECONDS = 600` from `swap_helper.rs:123`.

### L-5: Duplicate `PoolDetails` Struct Name

**Location:** `pool/src/asset.rs:226` and `pool/src/state.rs:173`

Two different structs named `PoolDetails` exist with different fields. While Rust's module system prevents ambiguity, this is confusing for maintainers.

### L-6: Typos Throughout Codebase

Multiple typos: "commiter" -> "committer", "incriment" -> "increment", "priot" -> "prior", "exisitnng" -> "existing", "liquidty" -> "liquidity", "trakcing" -> "tracking", "taht" -> "that", etc.

### L-7: `count` Variable Unused in `query_pool_commiters`

**Location:** `pool/src/query.rs:383, 415`

`count` is incremented but its value (set as `total_count` in the response) always equals `commiters.len()`, making it redundant.

### L-8: `easy-addr` Uses `cosmwasm-std = "2.2.0"` vs Workspace `1.5.11`

**Location:** `packages/easy-addr/Cargo.toml`

Major version mismatch with the rest of the workspace. Works since it's a proc-macro, but is fragile across upgrades.

---

## Architecture Assessment

### Strengths
1. **Two-phase lifecycle** -- The commit-then-trade model protects early supporters
2. **TWAP oracle with accumulator pattern** -- Resistant to single-block manipulation
3. **Batched distribution** -- Handles large committer sets without gas limits
4. **NFT LP positions with fee tracking** -- Clean proportional fee accounting
5. **Timelocked admin operations** -- 48-hour delay on config and upgrade changes
6. **Reentrancy protection with admin recovery** -- Guards on swap/commit with stuck-state recovery
7. **Rate limiting** -- 13-second minimum commit interval prevents spam
8. **Comprehensive test suite** -- 103 tests across 6 test modules
9. **Checked arithmetic throughout** -- Consistent use of `checked_*` operations
10. **Pool creation cleanup** -- Failed pool creations properly clean up token/NFT contracts
11. **Minimum liquidity lock** -- Prevents first-depositor inflation attacks
12. **Fee reserve separation** -- Fee tokens tracked separately from tradeable reserves (when correctly implemented)

### Remaining Weaknesses
1. **Commission accounting inconsistency** -- Post-threshold swaps don't subtract commission from reserves (C-1)
2. **Single-signer admin** -- No multisig requirement for emergency operations
3. **On-chain randomness** -- Predictable oracle pool selection
4. **Commit flow complexity** -- ~500 lines with 4 branches (pre-threshold, exact, overshoot, post-threshold)

---

## Summary of Required Changes Before Production

### Must Fix (Critical)
1. **C-1**: Fix commission double-counting in `process_post_threshold_commit` and threshold-crossing excess swap by subtracting `commission_amt` from `reserve1` (matching the pattern in `execute_simple_swap`)

### Should Fix (High)
2. **H-1**: Improve oracle pool selection to resist validator manipulation
3. **H-2**: Validate recipient address in expand-economy `execute_withdraw`

### Recommended (Medium)
4. **M-1**: Remove circular factory validation dead code
5. **M-2**: Remove no-op `POOL_INFO.update` in reply handler
6. **M-3**: Add timelock to emergency withdrawal
7. **M-4**: Harden distribution batch failure handling

### Nice to Have (Low)
8. Remove dead code (`POOLS` map, `MAX_ORACLE_AGE`, unused `count` variable)
9. Fix position ID format inconsistency
10. Remove debug logging
11. Fix typos

---

**End of Audit Report**
