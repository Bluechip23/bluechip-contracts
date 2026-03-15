# Adversarial Attack Review — BlueChip Contracts

**Date:** March 15, 2026
**Reviewer:** Claude Opus 4.6 (Deep Adversarial Review)
**Scope:** All contracts — Pool, Factory, Expand Economy, shared interfaces
**Methodology:** "Red team" review — identifying the highest-ROI attacks an adversary would attempt

---

## Executive Summary

The BlueChip contract suite is well-engineered with strong fundamentals: checked arithmetic everywhere, reentrancy guards, TWAP-based oracle, timelocked admin operations, and zero `unwrap()`/`expect()` in production. However, several economically viable attack vectors remain that could yield profit to a sophisticated adversary. The most concerning are the **threshold-crossing first-swap privilege** and **oracle bootstrap spot price vulnerability**.

---

## Attack Vectors — Ranked by Severity

### ATTACK 1: Threshold-Crossing Sandwich — First Swap Privilege

- **Severity:** Medium-High
- **Capital Required:** ~$25,000+ (commit threshold amount)
- **Protected:** No

**Description:** When a commit crosses the funding threshold, `trigger_threshold_payout()` seeds the pool with known, predictable reserves (350B creator tokens + capped bluechip from commits). The **excess** from the threshold-crossing commit is then immediately swapped against this freshly-seeded pool (contract.rs lines 1030-1106). The attacker gets the absolute first swap — zero competition, predictable reserves, self-controlled slippage tolerance.

**Attack Steps:**
1. Monitor `USD_RAISED_FROM_COMMIT` via query — know exactly when a pool approaches threshold
2. Submit a large commit that crosses threshold by a significant margin (e.g., threshold $25k, commit $50k)
3. The transaction atomically: crosses threshold → seeds pool → swaps $25k excess at the best possible price
4. The `belief_price` and `max_spread` are user-controlled (lines 1050-1058), so the attacker sets them favorably
5. No other user can front-run because the pool doesn't exist until the threshold crossing completes

**Impact:** First-mover advantage on every new pool. The attacker captures value from the initial price discovery, getting more creator tokens than subsequent swappers at the same price.

**Recommendation:** Either (1) add a minimum time delay between threshold crossing and first swap, (2) use oracle TWAP pricing rather than AMM spot for the first swap, or (3) route the excess back as a pre-threshold commit rather than a swap.

---

### ATTACK 2: Oracle Price Manipulation During Bootstrap

- **Severity:** Medium
- **Capital Required:** Large (enough to move pool reserves significantly)
- **Protected:** Partially — post-rotation pools are skipped, but bootstrap is not

**Description:** The internal oracle (`internal_bluechip_price_oracle.rs:377-380`) falls back to spot prices (`calculate_price_from_reserves`) when no prior cumulative snapshot exists. On first deployment or after rotation, `calculate_twap()` returns a **single observation directly** (line 459-461).

**Attack Steps:**
1. Monitor for oracle initialization or pool rotation (every `ROTATION_INTERVAL = 3600s`)
2. Execute a large swap to skew reserves in a selected oracle pool
3. Immediately call `UpdateOraclePrice` (permissionless — no auth on line 221)
4. The skewed spot price becomes the TWAP (single observation = full weight)
5. Swap back to restore the pool
6. Commit to pools using the manipulated oracle valuation

**Impact:** Manipulated price inflates or deflates the USD value of bluechip commits, affecting threshold-crossing timing and committer reward distribution.

**Recommendation:** During bootstrap (no prior snapshot for ANY pool), skip pools from price weighting entirely. The code already does this for newly-rotated pools (line 384-385) — extend the same logic. Require at minimum 2 observations before any pool contributes to the price.

**Current Code (vulnerable):**
```rust
// Line 377-380: Bootstrap falls through to spot price
} else if prev_snapshots.is_empty() {
    calculate_price_from_reserves(bluechip_reserve, other_reserve)?
}
```

**Should be:**
```rust
} else if prev_snapshots.is_empty() {
    continue; // Skip like post-rotation pools
}
```

---

### ATTACK 3: Distribution Bounty Reserve Drain

- **Severity:** Medium
- **Capital Required:** Low (gas costs for dust commits + distribution calls)
- **Protected:** No

**Description:** `execute_continue_distribution()` pays `DISTRIBUTION_BOUNTY` (1,000,000 ubluechip) from `reserve0` on every batch (contract.rs:1341-1360). This is permissionless. The fee growth global is also decremented, affecting all LP positions.

**Attack Steps:**
1. Before a pool reaches threshold, create many dust commits from different wallets
2. After threshold crosses, distribution must process each committer individually
3. Call `ContinueDistribution` repeatedly — each call pays the bounty from pool reserves
4. With 1000 committers and batch size ~40: ~25 batches × 1M ubluechip = 25M ubluechip from reserves
5. LPs who haven't collected fees see their `fee_growth_global_0` reduced; the `unwrap_or(Decimal::zero())` silently absorbs underflows

**Impact:** Pool reserves depleted, LP fee fairness violated, potential accounting divergence between `fee_growth_global_0` and actual `fee_reserve_0`.

**Recommendation:**
- Fund bounty from a **separate bounty reserve**, not pool trading reserves
- Remove the `unwrap_or(Decimal::zero())` silent underflow — replace with an explicit error
- Cap total bounty payouts per distribution cycle

---

### ATTACK 4: Pre-Fee Accounting Mismatch in `NATIVE_RAISED_FROM_COMMIT`

- **Severity:** Low-Medium
- **Capital Required:** Requires admin fee change during funding phase
- **Protected:** Partially (commit fees appear immutable post-instantiation, but LP fee is changeable)

**Description:** `process_pre_threshold_commit()` records `asset.amount` (pre-fee) in `NATIVE_RAISED_FROM_COMMIT` (contract.rs:1237), but only `amount_after_fees` stays in the contract. When `trigger_threshold_payout()` runs, it re-deducts fees from `NATIVE_RAISED_FROM_COMMIT` using **current** fee rates.

**Attack Steps:**
1. If admin changes fee rates (via `UpdateConfigFromFactory`) during the funding phase
2. Earlier commits had fees deducted at rate X, but threshold payout recalculates at rate Y
3. If Y < X: pool seed inflated beyond actual available tokens → later LP withdrawals fail
4. If Y > X: excess tokens permanently locked

**Recommendation:** Track `amount_after_fees` in `NATIVE_RAISED_FROM_COMMIT`, or explicitly lock fee rates during the funding phase.

---

### ATTACK 5: NFT Position Fee Loss on Transfer

- **Severity:** Low-Medium
- **Capital Required:** None (social engineering)
- **Protected:** By design (anti-theft tradeoff)

**Description:** `sync_position_on_transfer()` (liquidity_helpers.rs:342-353) resets fee snapshots and **zeroes unclaimed fees** when the NFT changes hands. The previous owner's accrued-but-uncollected fees are permanently destroyed.

**Attack Steps:**
1. Find positions with large unclaimed fees on secondary NFT markets
2. Trick the owner into transferring the NFT (e.g., via misleading listing price)
3. Or conversely: collect all fees, then sell the "empty" position at a misleading price

**Impact:** Fee loss for unsuspecting NFT sellers; misleading position valuations on secondary markets.

**Recommendation:** Consider auto-claiming fees before transfer (via a CW721 transfer hook), or at minimum prominently document this behavior.

---

### ATTACK 6: Emergency Withdrawal Centralization Risk

- **Severity:** Low (requires admin key compromise), High Impact
- **Capital Required:** Admin key compromise
- **Protected:** 24-hour timelock

**Description:** Emergency withdrawal drains ALL pool funds (LP deposits + fee reserves + creator excess) to `bluechip_wallet_address`. The 24-hour timelock allows LP exit, but relies on active monitoring.

**Attack Steps (if factory admin compromised):**
1. Initiate emergency withdrawal on all pools simultaneously
2. Pool is paused (blocks swaps/fee collection) but LP removals still allowed
3. Any LP not monitoring within 24 hours loses all funds
4. No recovery mechanism — `EMERGENCY_DRAINED = true` is permanent

**Recommendation:**
- Extend timelock to 48-72 hours (matching factory config update)
- Enforce multisig at contract level (not just operational policy)
- Emit high-visibility events for indexer/notification integration

---

### ATTACK 7: Pool Creation Spam Affecting Mint Decay Curve

- **Severity:** Low
- **Capital Required:** Gas costs only
- **Protected:** Gas costs are the only barrier

**Description:** Pool creation is permissionless. `calculate_mint_amount()` uses `pool_id` directly as the decay variable `x`. Spam pools permanently inflate pool IDs, reducing bluechip minting for all future legitimate pools.

**Formula:** `500 - (((5*x^2 + x) / ((s/6) + 333*x))` where `x = pool_id`

**Impact:** Each spam pool permanently reduces future minting rewards. With 100 spam pools, the decay curve shifts significantly — legitimate pool #101 gets far fewer minted bluechip than it should.

**Recommendation:** Add a minimum deposit requirement for pool creation, or use a separate counter that only increments for pools that cross threshold.

---

## Attacks Verified as NOT Exploitable

| Attack | Why It Fails |
|--------|-------------|
| **Reentrancy via CW20** | `RATE_LIMIT_GUARD` acts as reentrancy guard; CosmWasm prevents cross-contract reentrancy within same tx |
| **Integer overflow in swap math** | All math uses `Uint256` intermediates, `checked_*` ops, and `overflow-checks = true` in release profile |
| **Malicious CW20 token registration** | Factory creates all CW20 contracts — no path to register external tokens |
| **Double-mint on threshold crossing** | `POOL_THRESHOLD_MINTED` map prevents duplicate `NotifyThresholdCrossed` calls |
| **Pyth oracle manipulation** | 5% confidence interval check, staleness validation, and negative price rejection |
| **Direct reserve manipulation** | All reserve updates use checked arithmetic; reserves only modified through authorized paths |
| **Admin config without timelock** | 48-hour timelock on factory config; LP fee bounded to 0.1%-10% on pool updates |

---

## Strengths Worth Noting

1. **`overflow-checks = true`** in release profile — eliminates integer overflow class entirely
2. **Zero `unwrap()`/`expect()`** in production code — no unexpected panics
3. **Reentrancy guard** on all state-modifying paths (swap, commit, liquidity ops)
4. **TWAP oracle** with 3600s window resists single-block manipulation
5. **NFT-based position ownership** verified via CW721 `OwnerOf` query on every operation
6. **48-hour timelocks** on factory config and pool upgrade proposals
7. **Oracle staleness checks** at both pool (600s) and factory (300s) levels
8. **Pyth confidence interval validation** rejects wide-band prices (>5%)
9. **Distribution timeout** (7200s) and failure tracking prevent infinite stuck states
10. **Fee capping** via `calc_capped_fees()` prevents claiming more than fee reserves hold

---

## Priority Remediation Order

1. **[Critical Path]** Fix oracle bootstrap spot price vulnerability (Attack #2) — simple code change
2. **[Critical Path]** Add delay or TWAP pricing for threshold-crossing excess swap (Attack #1)
3. **[Important]** Rework distribution bounty funding source (Attack #3)
4. **[Important]** Track post-fee amounts in `NATIVE_RAISED_FROM_COMMIT` (Attack #4)
5. **[Hardening]** Add minimum deposit for pool creation (Attack #7)
6. **[Hardening]** Extend emergency withdrawal timelock to 48h+ (Attack #6)
7. **[UX]** Document or mitigate NFT transfer fee loss (Attack #5)

---

*End of Adversarial Review*
