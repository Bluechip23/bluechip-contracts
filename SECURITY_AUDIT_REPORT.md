# BlueChip Contracts Security Audit Report

**Date:** March 5, 2026
**Auditor:** Claude Opus 4.6 (Automated Tier-1 Audit)
**Scope:** All contracts in the `bluechip-contracts` repository
**Contracts Audited:** Pool, Factory, Expand Economy, Pool-Factory-Interfaces
**Commit:** HEAD of main branch
**Severity Framework:** Critical / High / Medium / Low / Informational

---

## Executive Summary

The BlueChip contracts implement a three-contract system (Factory, Pool, Expand Economy) for decentralized creator subscriptions with an XYK AMM, commitment-based fundraising, and NFT-tracked liquidity positions. The codebase demonstrates significant security awareness: checked arithmetic is used consistently, reentrancy guards are present, timelocks protect privileged operations, and oracle staleness checks are implemented.

**Overall risk posture: Medium.** The critical and high findings from prior audits appear to have been addressed. No direct fund-theft vulnerabilities were identified. However, several medium-severity issues remain that could lead to fund lockups, griefing, or economic manipulation under specific conditions. The most significant concerns are: (1) the `Addr::unchecked` usage in CW20 swap handling which bypasses address validation, (2) the distribution bounty mechanism paying from fee reserves which creates an accounting asymmetry, (3) potential for oracle TWAP manipulation during the bootstrap phase when only spot prices are available, and (4) the `pre_threshold_commit` function recording `asset.amount` (pre-fee) in `NATIVE_RAISED_FROM_COMMIT` rather than the post-fee amount.

**Recommendation:** Address the Medium findings before mainnet deployment. The Low and Informational items should be resolved as part of code hardening but are not deployment blockers.

---

## Findings

### **[M-1] `Addr::unchecked` in CW20 Swap Path Bypasses Address Validation**

- **Severity:** Medium
- **Category:** Input Validation
- **File & Line(s):** `pool/src/contract.rs:542`
- **Description:** In `execute_swap_cw20`, the `cw20_msg.sender` field is wrapped with `Addr::unchecked()` before being passed to `simple_swap`. In CosmWasm, `cw20_msg.sender` is a `String` set by the CW20 contract in its `Send` handler. While the CW20 standard guarantees this is the actual sender, using `Addr::unchecked` means no canonical address validation occurs. If a non-standard or malicious CW20 token contract were somehow registered (which the factory's creation process should prevent), it could forge the sender field.
- **Attack Scenario:** If a malicious CW20 contract is set as a pool's token (requires factory compromise or a bug in pool creation), it could set `cw20_msg.sender` to an arbitrary address, directing swap outputs to the attacker while debiting another user's tokens.
- **Impact:** Swap outputs directed to wrong recipient. Requires a precondition (malicious CW20) that is unlikely under normal operation since the factory creates the CW20 contract.
- **Recommendation:** Replace `Addr::unchecked(cw20_msg.sender)` with `deps.api.addr_validate(&cw20_msg.sender)?`. This is a defense-in-depth measure:
  ```rust
  let validated_sender = deps.api.addr_validate(&cw20_msg.sender)?;
  simple_swap(deps, env, info, validated_sender, ...)
  ```
- **References:** CosmWasm best practices; CW20 spec for `Cw20ReceiveMsg`.

---

### **[M-2] Pre-Threshold Commit Records Pre-Fee Amount in `NATIVE_RAISED_FROM_COMMIT`**

- **Severity:** Medium
- **Category:** Fund Safety / Accounting
- **File & Line(s):** `pool/src/contract.rs:1236` (in `process_pre_threshold_commit`)
- **Description:** In `process_pre_threshold_commit`, the `NATIVE_RAISED_FROM_COMMIT` tracker is updated with `asset.amount` — the full amount before fees are deducted. However, the pool contract only retains `amount_after_fees` (the portion remaining after protocol and creator fees are sent out). When threshold crossing occurs, `trigger_threshold_payout` at `generic_helpers.rs:226-230` calculates `pools_bluechip_seed` by loading `NATIVE_RAISED_FROM_COMMIT` and deducting fees again via `checked_mul_floor(one_minus_fee)`. This double-fee-deduction means the actual bluechip available for the pool seed is slightly less than what's recorded, but the second deduction compensates. The real issue is that if fee rates were ever changed between commits (via `UpdateConfigFromFactory`), the retroactive deduction would be incorrect — using the current fee rate to re-deduct from amounts that had a different fee rate applied at commit time.
- **Attack Scenario:** Admin proposes a fee change (via timelock) partway through a pool's funding phase. Earlier commits had fees deducted at rate X, but the threshold payout recalculates using rate Y. If Y < X, the pool seed would be inflated beyond actual available tokens. If Y > X, some committed tokens would be unaccounted for.
- **Impact:** Accounting mismatch between tracked and actual bluechip in the pool. Could lead to the pool seed containing fewer tokens than reserves claim, causing later withdrawals to fail, or excess tokens being locked.
- **Recommendation:** Track `amount_after_fees` in `NATIVE_RAISED_FROM_COMMIT` instead of `asset.amount`, and remove the re-deduction in `trigger_threshold_payout`. Alternatively, lock fee rates per pool at instantiation time (which they currently are via `COMMITFEEINFO`, but the config update path could change them).

---

### **[M-3] Distribution Bounty Creates Fee Accounting Asymmetry**

- **Severity:** Medium
- **Category:** Fund Safety / State Consistency
- **File & Line(s):** `pool/src/contract.rs:1392-1407`
- **Description:** The `execute_continue_distribution` function pays a `DISTRIBUTION_BOUNTY` (1,000,000 ubluechip) from `fee_reserve_0` to incentivize callers to drive distribution batches. It also decrements `fee_growth_global_0` by `DISTRIBUTION_BOUNTY / total_liquidity`. However, this fee growth reduction affects ALL existing liquidity positions globally — positions that have already checkpointed their `fee_growth_inside_0_last` will see reduced uncollectable fees, while positions that collect fees before the reduction lose nothing. The `unwrap_or(Decimal::zero())` on line 1404-1405 means if the subtraction underflows (bounty paid exceeds accumulated growth), fee_growth_global_0 silently goes to zero rather than reverting.
- **Attack Scenario:** An attacker with a large liquidity position collects all fees first, then triggers many `ContinueDistribution` calls (each paying a bounty from fee reserves). Other LPs who haven't collected yet find their claimable fees reduced. If distribution requires many batches, the total bounty drain could be significant.
- **Impact:** LP fee fairness violation. Small LPs who don't collect frequently get diluted. The `unwrap_or(Decimal::zero())` silently absorbing underflows could also cause fee_growth_global_0 to diverge from the actual fee_reserve_0, eventually causing `checked_sub` failures when LPs try to collect.
- **Recommendation:** Either (1) fund the bounty from pool reserves rather than fee reserves, tracking it as a separate line item, or (2) don't adjust `fee_growth_global_0` and instead track bounty payouts separately so they don't affect LP fee calculations. At minimum, replace the `unwrap_or(Decimal::zero())` with an explicit check that errors if underflow occurs.

---

### **[M-4] Oracle Bootstrap Phase Uses Spot Prices Vulnerable to Manipulation**

- **Severity:** Medium
- **Category:** Oracle & External Data Trust
- **File & Line(s):** `factory/src/internal_bluechip_price_oracle.rs:377-380`
- **Description:** During the oracle's first update cycle (or after pool rotation introduces a new pool), the `calculate_weighted_price_with_atom` function falls back to `calculate_price_from_reserves` (spot price) instead of TWAP because no prior cumulative snapshot exists. An attacker could manipulate pool reserves (via a large swap) just before the oracle's first update to skew the price. Since oracle updates are permissionless (`UpdateOraclePrice` has no access control), the attacker can time this precisely.
- **Attack Scenario:**
  1. Attacker monitors for oracle rotation or initialization
  2. Executes a large swap to skew reserves in a selected pool
  3. Immediately calls `UpdateOraclePrice` to capture the skewed spot price
  4. The manipulated price enters the TWAP, affecting all subsequent commit USD valuations
- **Impact:** Manipulated oracle price affects how much USD value is attributed to bluechip commits. An inflated price means the threshold is crossed with fewer actual tokens, diluting committers' rewards. A deflated price means more tokens are needed, potentially locking excess user funds.
- **Recommendation:** During bootstrap (no prior snapshot), skip the pool from price weighting entirely (as is done for post-rotation pools at line 384-385). This change would make the oracle require at least 2 update cycles before any pool contributes to the price, preventing same-block manipulation. The code already does this for newly rotated pools — extend the same logic to the bootstrap case.

---

### **[M-5] `usd_payment_tolerance_bps` Update Has No Bounds Check**

- **Severity:** Medium
- **Category:** Access Control / Input Validation
- **File & Line(s):** `pool/src/contract.rs:1508-1514`
- **Description:** When the factory admin updates `usd_payment_tolerance_bps` via `UpdateConfigFromFactory`, no bounds check is applied. The LP fee has min/max bounds (0.1% to 10%), and `min_commit_interval` has a max of 86,400 seconds. But `usd_payment_tolerance_bps` can be set to any `u16` value (0 to 65,535 = 0% to 655.35%). While this field doesn't appear to be actively used in the commit flow (the commit logic uses oracle prices directly), if it were used for tolerance checking, an extremely high value would bypass price validation entirely.
- **Attack Scenario:** A compromised factory admin (or a timelock that goes unnoticed) sets `usd_payment_tolerance_bps` to `u16::MAX`, effectively disabling any price tolerance check.
- **Impact:** If the field is used for price validation (currently it appears unused in the commit code path), it could allow commits at manipulated prices. Even if unused, setting arbitrary values is a code quality concern.
- **Recommendation:** Add bounds checking, e.g., max 1000 bps (10%):
  ```rust
  if tolerance > 1000 {
      return Err(ContractError::Std(StdError::generic_err(
          "usd_payment_tolerance_bps must not exceed 1000 (10%)"
      )));
  }
  ```

---

### **[L-1] Race Condition Window in Threshold Crossing When `THRESHOLD_PROCESSING` Lock Is Held**

- **Severity:** Low
- **Category:** State Consistency
- **File & Line(s):** `pool/src/contract.rs:922-953`
- **Description:** When a commit transaction would cross the threshold, the code attempts to acquire the `THRESHOLD_PROCESSING` lock. If the lock is already held (`can_process == false`), the code falls through to check if the threshold was already hit. If it wasn't (another tx is mid-processing), it processes as a normal pre-threshold commit at line 950-952. This means the commit is recorded with `usd_value` added to `USD_RAISED_FROM_COMMIT`, potentially causing `USD_RAISED_FROM_COMMIT` to exceed `commit_amount_for_threshold_usd`. The excess is bounded by transaction ordering (only one tx can be in this state), but the ledger amount could be slightly over the threshold.
- **Attack Scenario:** Two transactions arrive in the same block, both crossing the threshold. Transaction A acquires the lock, transaction B falls through and adds its full `usd_value` to the pre-threshold ledger. This results in `USD_RAISED_FROM_COMMIT` exceeding the threshold limit.
- **Impact:** Minor accounting overshoot. The excess committed USD is still tracked in the commit ledger and distributed proportionally, so no funds are lost. However, the `USD_RAISED_FROM_COMMIT` value no longer exactly equals the threshold.
- **Recommendation:** In the `can_process == false` branch (line 932), cap the `usd_value` added to `USD_RAISED_FROM_COMMIT` to not exceed the threshold, or simply return an error telling the user to retry.

---

### **[L-2] `POOL_PAUSED` Not Checked on `RemovePartialLiquidity` and `RemoveAllLiquidity`**

- **Severity:** Low
- **Category:** Access Control
- **File & Line(s):** `pool/src/contract.rs:328-382` (execute dispatch)
- **Description:** When the pool is paused (low liquidity), `CollectFees` and `SimpleSwap` correctly check `POOL_PAUSED` and return errors. However, `RemovePartialLiquidity`, `RemoveAllLiquidity`, and `RemovePartialLiquidityByPercent` do not check the paused state. This is arguably correct behavior — LPs should be able to exit even when the pool is paused — but it means liquidity removals can further drain an already low-liquidity pool, potentially leaving it with zero reserves.
- **Impact:** Minimal. This is actually a design choice that benefits LPs (they can always exit). However, it means the pool can be drained to zero while paused, and the auto-unpause logic (checking `MINIMUM_LIQUIDITY` on deposits) would not trigger until new deposits arrive.
- **Recommendation:** This appears to be intentional ("deposits are the mechanism to re-activate a paused pool"). Document this design decision explicitly. Consider whether the `MINIMUM_LIQUIDITY` (1000 units) lockup in position 0 is sufficient to prevent the pool from being fully drained.

---

### **[L-3] Emergency Withdrawal Sends All Funds to `bluechip_wallet_address` Without LP Compensation**

- **Severity:** Low
- **Category:** Fund Safety
- **File & Line(s):** `pool/src/contract.rs:1559-1658`
- **Description:** The emergency withdrawal mechanism drains ALL pool reserves (including LP shares and fee reserves) to `bluechip_wallet_address` (the protocol's fee wallet). LPs lose their deposited funds entirely. The 24-hour timelock gives LPs time to observe the pending withdrawal and remove their liquidity, but this relies on LPs actively monitoring the chain. The pool is paused during the timelock, which blocks swaps and fee collection, but `RemoveAllLiquidity` and `RemovePartialLiquidity` are still allowed (see L-2).
- **Impact:** If LPs don't notice the pending emergency withdrawal within 24 hours, their funds are permanently lost to them (sent to the protocol wallet). This is a centralization risk inherent in the design.
- **Recommendation:** This is a known trade-off in the design (factory admin has emergency power). Consider extending the timelock to 48-72 hours to match the factory config update timelock. Also consider emitting a high-visibility event/attribute that indexers and notification systems can flag.

---

### **[L-4] Factory `migrate` Allows Same-Version "Upgrade"**

- **Severity:** Low
- **Category:** Initialization & Migration
- **File & Line(s):** `factory/src/migrate.rs:1-32`
- **Description:** The factory's `migrate` function compares the stored version with the new version using `>=` and returns early (without error) if the stored version is greater than or equal to the new version. This means calling `migrate` with the same or lower version silently succeeds without making any changes. While not exploitable, this bypasses the expected contract upgrade process.
- **Impact:** No direct security impact. However, it means a migration could appear successful in logs/events without actually doing anything. This could mask issues during upgrade coordination.
- **Recommendation:** Return an error if `stored_semver >= new_version` rather than silently succeeding.

---

### **[L-5] `RecoverStuckStates::Both` Silently Ignores Recovery Failures**

- **Severity:** Low
- **Category:** Error Handling
- **File & Line(s):** `pool/src/contract.rs:411-414`
- **Description:** When `RecoveryType::Both` is used, the code calls `recover_threshold`, `recover_distribution`, and `recover_reentrancy_guard` and ignores errors from each (`let _ = ...`). This means if one recovery succeeds but another fails due to a genuine error (not "not stuck"), the error is silently swallowed. The function could report partial recovery success while hiding an underlying issue.
- **Impact:** Low. Recovery is an admin operation used only in stuck states. However, silent error suppression could mask storage corruption.
- **Recommendation:** Collect errors and include them in the response attributes, or at minimum log which recoveries failed and why.

---

### **[L-6] Permissionless Oracle Update Could Be Front-Run**

- **Severity:** Low
- **Category:** Oracle & External Data Trust / MEV
- **File & Line(s):** `factory/src/internal_bluechip_price_oracle.rs:221`
- **Description:** `update_internal_oracle_price` is permissionless (anyone can call it). While this is good for liveness, it means an attacker can time oracle updates to coincide with favorable pool states. The 300-second update interval provides some protection, but an attacker could still: (1) skew a pool's reserves, (2) call `UpdateOraclePrice`, (3) immediately swap back. The TWAP dampens this, but with only a few observations, the impact is amplified.
- **Impact:** Slight oracle manipulation, dampened by TWAP averaging. More impactful during early oracle lifecycle with few observations.
- **Recommendation:** Consider adding a small gas bounty or restricting oracle updates to a whitelist during the early bootstrap phase.

---

### **[I-1] `overflow-checks = true` in Release Profile — Good Practice Confirmed**

- **Severity:** Informational
- **Category:** Integer Arithmetic
- **File & Line(s):** `Cargo.toml:28` (workspace root)
- **Description:** The release profile has `overflow-checks = true`, meaning arithmetic overflow will panic at runtime rather than silently wrapping. This is excellent practice for CosmWasm contracts and eliminates an entire class of vulnerabilities. Combined with the extensive use of `checked_*` methods throughout the codebase, integer overflow/underflow risk is minimal.
- **Recommendation:** None. This is correct and should be maintained.

---

### **[I-2] CW20 Token Minter Cap Set to 1.5T But Only 1.2T Is Distributed**

- **Severity:** Informational
- **Category:** Business Logic
- **File & Line(s):** `factory/src/execute.rs:198` (cap: 1,500,000,000,000) vs `factory/src/pool_creation_reply.rs:97-102` (total: 1,200,000,000,000)
- **Description:** The CW20 token's minter cap is set to 1.5 trillion, but the threshold payout only distributes 1.2 trillion. This leaves 300 billion tokens unmintable but allocated in the cap. The extra 300B appears to provide headroom for edge cases or future use, but there's no documented path to mint these additional tokens.
- **Impact:** No security impact. The 1.2T exact payout is enforced by `validate_pool_threshold_payments`. The extra cap headroom cannot be exploited because the pool contract (which is the minter) has no code path to mint beyond the threshold payout.
- **Recommendation:** Document why the cap is 1.5T vs 1.2T distribution, or reduce the cap to exactly 1.2T if the extra headroom is unintentional.

---

### **[I-3] No `unwrap()` or `expect()` in Production Code**

- **Severity:** Informational
- **Category:** Code Quality
- **File & Line(s):** All production source files
- **Description:** A thorough search confirms zero `unwrap()` or `expect()` calls in any production (non-test, non-mock) source file across all three contracts. All error handling uses `?` operator, `checked_*` methods, or explicit `map_err`. This is excellent practice and prevents unexpected panics in production.
- **Recommendation:** None. Maintain this discipline.

---

### **[I-4] `MIN_MULTIPLIER` Uses `unwrap_or` Fallback**

- **Severity:** Informational
- **Category:** Code Quality
- **File & Line(s):** `pool/src/liquidity_helpers.rs:189`
- **Description:** `Decimal::from_str(MIN_MULTIPLIER).unwrap_or(Decimal::percent(10))` uses `unwrap_or` instead of propagating the error. Since `MIN_MULTIPLIER` is a compile-time constant `"0.1"`, the `unwrap_or` will never trigger — `Decimal::from_str("0.1")` always succeeds. However, using `unwrap_or` here is semantically misleading as it silently provides a fallback for what should be an infallible operation.
- **Impact:** None. The constant parses correctly.
- **Recommendation:** Replace with `.expect("constant MIN_MULTIPLIER must parse")` or compute the value at compile time using `Decimal::percent(10)` directly.

---

### **[I-5] Pool Creation Is Permissionless**

- **Severity:** Informational
- **Category:** Access Control
- **File & Line(s):** `factory/src/execute.rs:159`
- **Description:** `execute_create_creator_pool` does not check the caller against an allowlist. Any address can create a pool via the factory. This is by design for a permissionless platform, but it means anyone can deploy CW20 tokens and NFT contracts through the factory, consuming chain storage.
- **Impact:** No direct security impact. Storage spam is rate-limited by gas costs. Each pool creation instantiates 3 contracts (CW20, CW721, Pool), which consumes significant gas.
- **Recommendation:** Consider whether a minimum deposit or cooldown should be required for pool creation to prevent spam.

---

### **[I-6] Price Accumulator Uses `saturating_add` Which Could Mask Overflow**

- **Severity:** Informational
- **Category:** Integer Arithmetic
- **File & Line(s):** `pool/src/swap_helper.rs:104-109`
- **Description:** The cumulative price accumulators use `saturating_add` for `price0_cumulative_last` and `price1_cumulative_last`. In Uniswap V2, these accumulators are designed to overflow (using wrapping arithmetic) and TWAP is calculated from the difference. Using `saturating_add` means the accumulator caps at `Uint128::MAX` and stops accumulating, which would break TWAP calculations for very long-lived pools. However, given the magnitude of typical values (reserve ratios times seconds), overflow of `Uint128` is practically impossible within any realistic timeframe.
- **Impact:** Theoretically incorrect TWAP semantics, but practically impossible to trigger with `Uint128` range.
- **Recommendation:** Document that `saturating_add` is used intentionally because `Uint128` overflow is impossible in practice, or switch to wrapping arithmetic if strict Uniswap V2 compatibility is desired.

---

## Invariant Verification

| Invariant | Status | Notes |
|-----------|--------|-------|
| Total LP fees collected never exceed fee reserves | **Holds** | `calc_capped_fees` caps at `fee_reserve_0/1`. Checked_sub used for reserve decrements. |
| Threshold payout amounts always sum to 1.2T | **Holds** | `validate_pool_threshold_payments` enforces exact values. Double-checked in `trigger_threshold_payout`. |
| Only factory can call privileged pool operations | **Holds** | `Pause`, `Unpause`, `EmergencyWithdraw`, `UpdateConfigFromFactory`, `RecoverStuckStates` all check `factory_addr`. |
| Commit fees never exceed 100% | **Holds** | Checked in `instantiate` (line 72-76). Also validated with `total_fees >= amount` check (line 868). |
| Position ownership verified via NFT | **Holds** | `verify_position_ownership` queries CW721 `OwnerOf` on every fee collection, liquidity removal, and position addition. |
| Oracle price staleness enforced | **Holds** | Pool-side: `MAX_ORACLE_STALENESS_SECONDS` = 600s. Factory-side: `MAX_PRICE_AGE_SECONDS_BEFORE_STALE` = 300s. Pyth confidence check at 5%. |
| Emergency withdrawal requires 24h timelock | **Holds** | Two-phase process. `EMERGENCY_WITHDRAW_DELAY_SECONDS` = 86,400. Phase 1 sets timelock, Phase 2 checks it. |
| Double-mint prevention for threshold crossing | **Holds** | `POOL_THRESHOLD_MINTED` map prevents `NotifyThresholdCrossed` from being called twice per pool. |
| Reserves track actual token balances | **Needs Review** | See M-2, M-3. Distribution bounty and fee accounting could cause minor divergence. |
| Reentrancy guard prevents concurrent operations | **Holds** | `RATE_LIMIT_GUARD` set before swap/commit, cleared after. Guard checked at entry. Recovery available via admin. |

---

## Attack Surface Summary

| Entry Point | Contract | Auth Required | Risk Rating | Notes |
|------------|----------|---------------|-------------|-------|
| `Commit` | Pool | None | Medium | Oracle-dependent pricing, threshold logic |
| `SimpleSwap` | Pool | None (post-threshold) | Low | Standard XYK with slippage protection |
| `Receive` (CW20 Swap) | Pool | CW20 contract only | Medium | `Addr::unchecked` on sender (M-1) |
| `DepositLiquidity` | Pool | None (post-threshold) | Low | Proper ratio enforcement |
| `RemoveAllLiquidity` | Pool | NFT owner | Low | Proper ownership verification |
| `RemovePartialLiquidity` | Pool | NFT owner | Low | Fee preservation logic added |
| `CollectFees` | Pool | NFT owner | Low | Capped to fee reserves |
| `ContinueDistribution` | Pool | None (permissionless) | Medium | Bounty drain concern (M-3) |
| `EmergencyWithdraw` | Pool | Factory admin | Medium | Centralization risk (L-3) |
| `RecoverStuckStates` | Pool | Factory admin | Low | Silent error suppression (L-5) |
| `Pause` / `Unpause` | Pool | Factory admin | Low | Proper auth |
| `UpdateConfigFromFactory` | Pool | Factory admin | Low | Bounds checked (except tolerance bps) |
| `ClaimCreatorExcessLiquidity` | Pool | Creator only | Low | Timelock enforced |
| `Create` | Factory | None (permissionless) | Low | Spam concern only (I-5) |
| `ProposeConfigUpdate` | Factory | Factory admin | Low | 48h timelock |
| `UpdateOraclePrice` | Factory | None (permissionless) | Medium | Front-running concern (M-4, L-6) |
| `ForceRotateOraclePools` | Factory | Factory admin | Low | Proper auth |
| `UpgradePools` | Factory | Factory admin | Low | 48h timelock, batched |
| `NotifyThresholdCrossed` | Factory | Pool contract only | Low | Double-mint prevention |
| `ExpandEconomy` | Expand Economy | Factory only | Low | Simple send, zero-amount check |
| `ProposeWithdrawal` | Expand Economy | Owner only | Low | 48h timelock |
| `UpdateConfig` | Expand Economy | Owner only | Low | Address validation |
| `migrate` | Pool | Chain admin | Low | Fee bounds checked |
| `migrate` | Factory | Chain admin | Low | Version check (L-4) |

---

## Recommendations Summary

**Priority order (highest severity first):**

1. **[M-1]** Replace `Addr::unchecked(cw20_msg.sender)` with `deps.api.addr_validate(&cw20_msg.sender)?` in `pool/src/contract.rs:542`.

2. **[M-2]** Track post-fee amounts in `NATIVE_RAISED_FROM_COMMIT` or document the double-deduction pattern explicitly as intentional.

3. **[M-3]** Rework the distribution bounty to not affect `fee_growth_global_0`, or fund it from a separate bounty reserve rather than LP fee reserves.

4. **[M-4]** During oracle bootstrap (no prior cumulative snapshot for any pool), skip pools from price weighting entirely, consistent with the post-rotation behavior already implemented.

5. **[M-5]** Add bounds checking to `usd_payment_tolerance_bps` updates (max 1000 bps suggested).

6. **[L-1]** Cap `usd_value` in the threshold race condition branch to not exceed remaining threshold.

7. **[L-3]** Consider extending emergency withdrawal timelock to 48-72 hours.

8. **[L-4]** Return an error on same-version migration instead of silently succeeding.

9. **[L-5]** Log individual recovery failures in `RecoveryType::Both` rather than suppressing them.

10. **[I-2]** Document or reduce the CW20 minter cap from 1.5T to 1.2T.

11. **[I-4]** Use `Decimal::percent(10)` directly instead of parsing from string constant.

---

*End of Report*
