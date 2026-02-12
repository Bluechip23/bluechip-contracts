# Bluechip Contracts — Security Audit Report

**Date:** 2026-02-12
**Auditor:** Claude (Senior Blockchain Security Auditor)
**Scope:** All CosmWasm smart contracts in the `bluechip-contracts` repository
**Contracts Audited:** Factory, Pool, Expand-Economy, Shared Interfaces
**Methodology:** Manual line-by-line review with adversarial threat modeling

---

## Executive Summary

The Bluechip protocol implements a creator subscription AMM with a two-phase pool lifecycle (funding → active trading), an internal TWAP oracle, NFT-based liquidity positions, and a batched distribution system. The codebase demonstrates awareness of common DeFi pitfalls — checked arithmetic, reentrancy guards, timelocks on admin operations, and rate limiting are present.

However, the audit identified **4 Critical**, **6 High**, **8 Medium**, and **8 Low/Informational** severity findings. The most dangerous issues involve fund loss paths during threshold crossing, oracle manipulation windows, and privileged admin capabilities that could be weaponized.

---

## Findings Summary

| ID | Severity | Title | Location |
|----|----------|-------|----------|
| C-01 | Critical | `execute_update_factory_config` lacks access control on execution | `factory/src/execute.rs:93` |
| C-02 | Critical | Pre-threshold commit stores gross amount but pool receives net-of-fees | `pool/src/contract.rs:1186-1239` |
| C-03 | Critical | Threshold crossing fee deduction uses raw `asset.amount` instead of net | `pool/src/contract.rs:936` |
| C-04 | Critical | `EmergencyWithdraw` sends all funds to factory admin, not to LPs | `pool/src/contract.rs:1492-1563` |
| I-01 | Informational | Swap commission accounting is non-obvious but correct | `pool/src/contract.rs:636` |
| H-01 | High | Oracle pool selection is predictable (block hash manipulation by validators) | `factory/src/internal_bluechip_price_oracle.rs:81-112` |
| H-02 | High | Spot price fallback on first observation defeats TWAP purpose | `factory/src/internal_bluechip_price_oracle.rs:349-357` |
| H-03 | High | `ContinueDistribution` bounty drains pool reserves without LP consent | `pool/src/contract.rs:1377-1389` |
| H-04 | High | `execute_update_config_from_factory` can change oracle address immediately (no timelock) | `pool/src/contract.rs:1461-1467` |
| H-05 | High | Pool migration path allows arbitrary code execution by admin | `factory/src/execute.rs:213-256` |
| H-06 | High | `Addr::unchecked` used on event-extracted address in `extract_contract_address` | `factory/src/pool_create_cleanup.rs:91` |
| M-01 | Medium | `RATE_LIMIT_GUARD` reentrancy guard is not a true reentrancy guard | `pool/src/contract.rs:565-588` |
| M-02 | Medium | `NATIVE_RAISED_FROM_COMMIT` tracks gross amounts including fees | `pool/src/contract.rs:1204` |
| M-03 | Medium | `Expand-Economy` owner can drain all contract funds via `Withdraw` | `expand-economy/src/contract.rs:127-157` |
| M-04 | Medium | No staleness check on internal oracle price when used in commits | `pool/src/swap_helper.rs:118-129` |
| M-05 | Medium | `is_bluechip_second` detection is unreliable | `factory/src/internal_bluechip_price_oracle.rs:288-292` |
| M-06 | Medium | `calculate_mint_amount` uses `pool_count` (next ID) not actual created count | `factory/src/execute.rs:142-145` |
| M-07 | Medium | Partial liquidity removal calculates fees on removed portion only | `pool/src/liquidity.rs:698-714` |
| M-08 | Medium | `MAX_PRICE_AGE_SECONDS_BEFORE_STALE` set to 3000s (50 minutes) — excessive | `factory/src/state.rs:30` |
| L-01 | Low | `CreatorTokenInfo.decimal` field is ignored; hardcoded to 6 | `factory/src/execute.rs:162` |
| L-02 | Low | NFTs not burned on full liquidity removal — ghost positions | `pool/src/liquidity.rs:589-592` |
| L-03 | Low | `query_cumulative_prices` mutates a local copy, doesn't persist | `pool/src/query.rs:195-218` |
| L-04 | Low | Pool creation is not permissioned — anyone can create pools | `factory/src/execute.rs:133` |
| L-05 | Low | `TEMP_POOL_CREATION` is a singleton — concurrent pool creations will corrupt | `factory/src/state.rs:10` |
| L-06 | Low | `UBLUECHIP_DENOM` hardcoded to `"stake"` in pool asset.rs but `"ubluechip"` used elsewhere | `pool/src/asset.rs:20` vs `pool/src/liquidity.rs:36` |
| L-07 | Low | Factory migration is a no-op for versions < 2.0.0 (loads and re-saves same config) | `factory/src/migrate.rs:20-23` |

---

## Detailed Findings

### C-01: `execute_update_factory_config` Lacks Access Control on Execution

**Severity:** Critical
**Location:** `factory/src/execute.rs:93-104`

```rust
pub fn execute_update_factory_config(deps: DepsMut, env: Env) -> Result<Response, ContractError> {
    let pending = PENDING_CONFIG.load(deps.storage)?;
    if env.block.time < pending.effective_after {
        return Err(ContractError::TimelockNotExpired { ... });
    }
    FACTORYINSTANTIATEINFO.save(deps.storage, &pending.new_config)?;
    // ...
}
```

**Issue:** The `execute_update_factory_config` function (mapped to `ExecuteMsg::UpdateConfig {}`) does **not** check `assert_correct_factory_address`. Any account can call `UpdateConfig` after the timelock expires. While the admin must propose the config, an attacker or front-runner could execute it at a strategically advantageous moment, or prevent the admin from canceling by executing it immediately once the timelock expires.

**Impact:** An attacker can time the execution of a pending config update to exploit a transient state, or front-run a cancellation. If the admin proposes a config they later wish to cancel, a bot could execute it instantly at the timelock boundary.

**Recommendation:** Add `assert_correct_factory_address(deps.as_ref(), info)?` to `execute_update_factory_config`, or allow only the admin to execute. Alternatively, make execution permissionless by design but acknowledge the front-running risk.

---

### C-02: Pre-Threshold Commit Tracks Gross Amount but Pool Receives Net-of-Fees

**Severity:** Critical
**Location:** `pool/src/contract.rs:1186-1239`

**Issue:** In `process_pre_threshold_commit`, the function records `asset.amount` (the gross amount before fees) into `NATIVE_RAISED_FROM_COMMIT` and `COMMIT_INFO`. However, fees are already sent out via `BankMsg::Send` in the `messages` vector that was built in the calling function. The pool contract holds `amount - fees`, but `NATIVE_RAISED_FROM_COMMIT` records the full `amount`.

```rust
// contract.rs:1204 — records gross amount
NATIVE_RAISED_FROM_COMMIT.update::<_, ContractError>(deps.storage, |r| Ok(r.checked_add(asset.amount)?))?;
```

Later at threshold crossing in `trigger_threshold_payout` (`generic_helpers.rs:268-272`):
```rust
let total_bluechip_raised = crate::state::NATIVE_RAISED_FROM_COMMIT.load(storage)?;
let pools_bluechip_seed = total_bluechip_raised.checked_mul_floor(one_minus_fee)?;
```

The code applies the fee deduction again on the already-inflated total, double-counting the fee deduction. But more critically, `pools_bluechip_seed` will be computed from a number larger than the actual tokens held by the contract, meaning the pool tries to seed with more bluechip tokens than it actually has.

**Impact:** When the threshold is crossed, the pool's `reserve0` will be set to a value higher than the actual bluechip balance held by the contract. This creates phantom reserves. All subsequent swaps will produce incorrect outputs, and liquidity withdrawals will eventually fail with insufficient funds. This is a fund loss bug.

**Recommendation:** `NATIVE_RAISED_FROM_COMMIT` should track `amount_after_fees` (net amount), not `asset.amount`. Alternatively, compute `pools_bluechip_seed` directly from the contract's bank balance at threshold crossing time.

---

### C-03: Threshold Crossing Fee Deduction on Excess Uses Incorrect Base

**Severity:** Critical
**Location:** `pool/src/contract.rs:932-941`

```rust
let bluechip_to_threshold = get_bluechip_value(deps.as_ref(), usd_to_threshold)?;
let bluechip_excess = asset.amount.checked_sub(bluechip_to_threshold)?;
let one_minus_fee = Decimal::one().checked_sub(total_fee_rate)?;
let effective_bluechip_excess = bluechip_excess.checked_mul_floor(one_minus_fee)?;
```

**Issue:** `bluechip_excess` is computed from `asset.amount` (the gross user-sent amount). But fees for the entire `amount` were already deducted and sent out (lines 802-832). The excess calculation should be on the net amount, not the gross. The user sends `amount`, fees on the full `amount` are extracted, but then `bluechip_excess = amount - bluechip_to_threshold`. This double-counts the fee for the excess portion — the fee was already taken from the full amount, and then `one_minus_fee` is applied again to the excess.

**Impact:** Users who cross the threshold with excess will receive fewer swap tokens than they should. The "missing" tokens become trapped in the contract forever.

**Recommendation:** Calculate `bluechip_excess` from `amount_after_fees - bluechip_to_threshold_net` to avoid double fee deduction.

---

### C-04: `EmergencyWithdraw` Sends All Funds to Factory Admin

**Severity:** Critical
**Location:** `pool/src/contract.rs:1492-1563`

```rust
pub fn execute_emergency_withdraw(deps: DepsMut, _env: Env, info: MessageInfo) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr { return Err(ContractError::Unauthorized {}); }
    // ... collects ALL reserves + fees + creator excess ...
    // Sends everything to info.sender (the factory)
    messages.push(TokenInfo { ... }.into_msg(&deps.querier, info.sender.clone())?);
}
```

**Issue:** The emergency withdraw function sends **all pool funds** (reserves, uncollected fees, and creator excess positions) to the factory contract caller. It does not distribute funds proportionally to LP position holders. This is a rug-pull vector: the admin can call `EmergencyWithdraw` and seize all user funds.

**Impact:** Total loss of user funds. The factory admin (a single key or multisig) can unilaterally drain any pool at any time.

**Recommendation:** Emergency withdrawal should either (a) distribute proportionally to LP holders, (b) send to a timelock-governed escrow, or (c) be governed by a DAO vote. At minimum, add a significant timelock.

---

### C-05: Commission Not Subtracted From Ask Pool Correctly in `execute_simple_swap`

**Severity:** Critical
**Location:** `pool/src/contract.rs:636`

```rust
let ask_pool_post = ask_pool.checked_sub(return_amt.checked_add(commission_amt)?)?;
```

**Issue:** The code subtracts `return_amt + commission_amt` from the ask pool. But in the `compute_swap` function (`swap_helper.rs:63-74`), `commission_amt` is calculated from `return_amount` (before commission deduction) and `final_return_amount = return_amount - commission_amount`. So the actual `return_amt` returned to the user is already commission-deducted.

Looking at `compute_swap`:
- `return_amount` = raw amount from XY=K formula
- `commission_amount` = `return_amount * commission_rate`
- `final_return_amount` = `return_amount - commission_amount` (this is what the caller gets as `return_amt`)

So in `execute_simple_swap`:
- `return_amt` = final (post-commission) amount going to user
- `commission_amt` = the commission

The ask pool should decrease by `return_amt + commission_amt` = `return_amount` (the original pre-commission return). This is actually the total leaving the ask pool: `return_amt` goes to user, `commission_amt` stays in the pool as fees.

Wait — the commission stays in the pool but is tracked in `fee_reserve`. Let me re-examine...

Actually, looking more carefully: `ask_pool_post = ask_pool - (return_amt + commission_amt)`. Then `update_pool_fee_growth` adds `commission_amt` to `fee_reserve`. So the effective reserve decrease is `ask_pool - return_amt - commission_amt`, but commission_amt goes into fee_reserve (which is separate accounting). The `return_amt` goes to the user. The fee stays in the contract but is not in `reserves` — it's in `fee_reserve`.

**Revised Assessment:** This is actually correct accounting IF fee reserves are separate from pool reserves. The commission tokens remain in the contract but are tracked in `fee_reserve` rather than `reserves`. When LPs withdraw fees, they come from `fee_reserve`. This is consistent. **Downgrading to Informational — not a vulnerability, but the dual-tracking model is fragile and should be clearly documented.**

**Revised Severity:** Informational (accounting is correct but non-obvious)

---

### H-01: Oracle Pool Selection is Predictable

**Severity:** High
**Location:** `factory/src/internal_bluechip_price_oracle.rs:81-112`

```rust
let mut hasher = Sha256::new();
hasher.update(env.block.time.seconds().to_be_bytes());
hasher.update(env.block.height.to_be_bytes());
hasher.update(env.block.chain_id.as_bytes());
let hash = hasher.finalize();
```

**Issue:** The "randomness" for selecting oracle pools is derived entirely from deterministic on-chain data: block time, block height, and chain ID. A validator or sophisticated attacker who can predict (or influence) the block time can pre-compute which pools will be selected and manipulate those specific pools' reserves before the oracle update.

**Impact:** An adversarial validator can:
1. Pre-compute which pools will be selected for the next rotation
2. Manipulate those pools' reserves via large swaps
3. Call `UpdateOraclePrice` to lock in a manipulated TWAP
4. Exploit the skewed price in commit operations (under/over-paying)

**Recommendation:** Include unpredictable entropy sources (e.g., previous block's app hash if available via `env`), or use a commit-reveal scheme for pool rotation. Consider using on-chain VRF if available on the deployment chain.

---

### H-02: Spot Price Fallback Defeats TWAP Anti-Manipulation

**Severity:** High
**Location:** `factory/src/internal_bluechip_price_oracle.rs:349-357`

```rust
// No previous snapshot — first observation, use spot price as baseline
let (bluechip_reserve, other_reserve) = if is_bluechip_second { ... } else { ... };
calculate_price_from_reserves(bluechip_reserve, other_reserve)?
```

**Issue:** After every pool rotation (every `ROTATION_INTERVAL = 3600` seconds), cumulative snapshots are cleared. The first observation after rotation **falls back to spot reserves**, which are trivially manipulable within a single transaction. An attacker can:

1. Wait for rotation
2. Flash-manipulate a selected pool's reserves
3. Trigger `UpdateOraclePrice` in the same block
4. The spot price is used directly as the TWAP for that observation

**Impact:** Oracle price can be manipulated every hour at rotation boundaries. Since commit operations depend on this price for USD conversion, attackers can get more creator tokens for less or drain value from the pool.

**Recommendation:** After rotation, require at least 2 observations before using the new pools' data. The first observation should only store the cumulative snapshot without contributing to the weighted price.

---

### H-03: `ContinueDistribution` Bounty Drains Pool Reserves

**Severity:** High
**Location:** `pool/src/contract.rs:1377-1389`

```rust
let bounty_paid = if pool_state.reserve0 > MINIMUM_LIQUIDITY + DISTRIBUTION_BOUNTY {
    let mut pool_state = pool_state;
    pool_state.reserve0 = pool_state.reserve0.checked_sub(DISTRIBUTION_BOUNTY)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    // ... pay bounty from reserves
```

**Issue:** The bounty (`1_000_000` = 1 BLUECHIP token) is paid from `pool_state.reserve0` — the active trading reserves belonging to liquidity providers. Each call to `ContinueDistribution` drains pool reserves. With 40+ committers requiring multiple batches, and anyone able to call this permissionlessly, this silently reduces LP value.

**Impact:** LP token holders lose value proportional to the number of distribution batches. For a pool with many committers (hundreds), this could represent significant value extraction. A griefer could also call `ContinueDistribution` repeatedly with tiny batch sizes to maximize reserve drainage.

**Recommendation:** Fund bounties from a separate allocation (e.g., deducted from the commit return amount during threshold crossing setup) rather than from active LP reserves.

---

### H-04: Pool Config Update Changes Oracle Without Timelock

**Severity:** High
**Location:** `pool/src/contract.rs:1461-1467`

```rust
if let Some(oracle_addr) = update.oracle_address {
    ORACLE_INFO.update(deps.storage, |mut info| -> StdResult<_> {
        info.oracle_addr = deps.api.addr_validate(&oracle_addr)?;
        Ok(info)
    })?;
}
```

**Issue:** The factory admin can change a pool's oracle address instantly via `UpdatePoolConfig`, bypassing the 48-hour timelock that applies to factory config changes. A compromised admin key could redirect the oracle to a malicious contract that returns fabricated prices.

**Impact:** Arbitrary price manipulation. With a fake oracle, the admin can make commit operations convert at any rate, effectively stealing bluechip tokens by making them appear worthless (or making USD appear worth more).

**Recommendation:** Apply the same 48-hour timelock pattern used for factory config updates to pool oracle address changes.

---

### H-05: Pool Migration Allows Arbitrary Code by Admin

**Severity:** High
**Location:** `factory/src/execute.rs:213-256`

**Issue:** The pool upgrade mechanism (`UpgradePools`) allows the admin to migrate any pool to **any** `new_code_id` with **any** `migrate_msg`. While a 48-hour timelock exists, the admin can deploy a malicious contract and migrate all pools to it, draining all funds.

Combined with `EmergencyWithdraw` (C-04), the admin has multiple paths to unilateral fund seizure.

**Impact:** This is a trust assumption, not necessarily a bug, but users must trust the admin key completely. For a protocol advertising decentralization, this is a significant centralization risk.

**Recommendation:** Consider:
1. Multi-sig or DAO governance for migrations
2. Whitelisting approved code IDs on-chain
3. Extended timelock (7+ days) for migrations specifically
4. On-chain migration previews that show which code ID is being migrated to

---

### H-06: `Addr::unchecked` Used on Event-Extracted Address

**Severity:** High
**Location:** `factory/src/pool_create_cleanup.rs:91`

```rust
.and_then(|addr_str| Ok(Addr::unchecked(addr_str)))
```

**Issue:** The `extract_contract_address` function parses the `_contract_address` attribute from instantiation events and wraps it with `Addr::unchecked`. While event data from CosmWasm instantiation is generally reliable, using `unchecked` bypasses address validation. If the event attribute were corrupted or if a chain upgrade changed address format, this would silently create an invalid `Addr`.

**Impact:** A corrupted or non-canonical address could be stored in state, causing permanent misconfiguration of the pool's token or NFT addresses.

**Recommendation:** Use `deps.api.addr_validate(&addr_str)?` instead.

---

### M-01: `RATE_LIMIT_GUARD` Is Not a True Reentrancy Guard

**Severity:** Medium
**Location:** `pool/src/contract.rs:565-588`

**Issue:** The `RATE_LIMIT_GUARD` is stored in contract storage (`Item<bool>`). In CosmWasm, all storage changes within a message execution are atomic — if the execution fails, all changes revert. A true reentrancy attack within a single message isn't possible in CosmWasm's execution model because cross-contract calls happen in sub-messages, and the calling contract's state is committed only after the full call stack succeeds.

However, the guard is never reset if the function panics (though Rust's `?` operator handles errors gracefully here). The main risk: if any path returns early without clearing the guard, the pool becomes permanently locked.

The variable name `RATE_LIMIT_GUARD` is misleading — it's named like a rate limiter but used as a reentrancy guard. The actual rate limiting is done separately in `check_rate_limit`.

**Impact:** Low direct risk due to CosmWasm's execution model, but a logic error that leaves the guard set would permanently brick the pool.

**Recommendation:** Consider removing this guard (it's unnecessary in CosmWasm) or rename it clearly. If kept, use a try/finally pattern or ensure all exit paths clear it.

---

### M-02: `NATIVE_RAISED_FROM_COMMIT` Tracks Gross Amounts

**Severity:** Medium
**Location:** `pool/src/contract.rs:1204`

Related to C-02 but worth separate tracking. The gross-vs-net accounting inconsistency propagates into:
1. Threshold crossing seed calculation
2. Creator excess liquidity calculation
3. All subsequent pool operations

---

### M-03: `Expand-Economy` Owner Can Drain All Funds

**Severity:** Medium
**Location:** `expand-economy/src/contract.rs:127-157`

```rust
pub fn execute_withdraw(deps: DepsMut, info: MessageInfo, ...) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner { return Err(ContractError::Unauthorized {}); }
    let send_msg = BankMsg::Send { to_address: target.clone(), amount: vec![Coin { denom, amount }] };
}
```

**Issue:** The expand-economy owner can withdraw any amount of any denomination from the contract at any time with no timelock or governance.

**Impact:** If the expand-economy contract holds bluechip tokens for future minting rewards, the owner can drain them all.

**Recommendation:** Add a timelock or remove the withdraw function. If withdrawal is needed for emergency recovery, add governance controls.

---

### M-04: No Staleness Check on Oracle Price During Commits

**Severity:** Medium
**Location:** `pool/src/swap_helper.rs:118-129`

```rust
pub fn get_usd_value(deps: Deps, bluechip_amount: Uint128) -> StdResult<Uint128> {
    let response: ConversionResponse = deps.querier.query_wasm_smart(
        factory_address.factory_addr,
        &FactoryQueryWrapper::InternalBlueChipOracleQuery(FactoryQueryMsg::ConvertBluechipToUsd { amount: bluechip_amount }),
    )?;
    Ok(response.amount)
}
```

**Issue:** When converting bluechip amounts to USD for commit tracking, the pool queries the factory oracle but does not check how old the cached price is. The factory's `get_bluechip_usd_price` also doesn't enforce staleness. Only `get_price_with_staleness_check` does, but it's not used in the conversion path.

The oracle update interval is 300 seconds but the staleness tolerance for Pyth is 3000 seconds. Combined with the internal TWAP window of 3600 seconds, prices could be significantly stale.

**Impact:** Users committing during periods of high volatility could receive credit for significantly more or less USD value than their actual contribution.

**Recommendation:** Add a staleness check in `bluechip_to_usd` and `usd_to_bluechip` functions.

---

### M-05: `is_bluechip_second` Detection is Unreliable

**Severity:** Medium
**Location:** `factory/src/internal_bluechip_price_oracle.rs:288-292`

```rust
let is_bluechip_second = if pool_state.assets.len() >= 2 {
    deps.api.addr_validate(&pool_state.assets[0]).is_ok()
} else {
    false
};
```

**Issue:** The code determines which reserve is bluechip by checking if `assets[0]` is a valid address. The assumption is: if assets[0] is an address (CW20 contract), then bluechip must be assets[1] (a native denom which would fail addr_validate). However, native denoms like `ubluechip` could theoretically be valid bech32 addresses on some chains, and CW20 contract addresses could fail validation in edge cases.

**Impact:** If the detection is wrong, the oracle computes inverted prices (1/actual_price), causing all commit USD calculations to be wildly incorrect.

**Recommendation:** Use explicit token type information rather than trying to infer it from address validation. Store the asset types with each pool registration.

---

### M-06: `calculate_mint_amount` Uses Pool Counter Instead of Actual Count

**Severity:** Medium
**Location:** `factory/src/execute.rs:142-145`

```rust
let pool_counter = POOL_COUNTER.load(deps.storage).unwrap_or(0);
let pool_id = pool_counter + 1;
POOL_COUNTER.save(deps.storage, &pool_id)?;
let mint_messages = calculate_and_mint_bluechip(&mut deps, env.clone(), pool_id)?;
```

**Issue:** `pool_id` is passed to `calculate_and_mint_bluechip` as `pools_created`. But `pool_id` is the *next* sequential ID, not the count of successfully created pools. If pool creation fails mid-way, the counter is already incremented but no pool was created. Over time, this inflates the `pools_created` input, causing the mint curve to decrease faster than intended.

**Impact:** Fewer bluechip tokens are minted per pool creation than the intended economic model prescribes.

**Recommendation:** Use a separate counter for successfully completed pool creations, incremented only in `finalize_pool`.

---

### M-07: Partial Liquidity Removal Fee Calculation

**Severity:** Medium
**Location:** `pool/src/liquidity.rs:698-714`

**Issue:** When removing partial liquidity, fees are calculated based only on the liquidity being removed. After removal, `fee_growth_inside_X_last` is updated to the current global. This means the remaining liquidity's uncollected fees (for the same time period) are effectively forfeited on that snapshot reset.

Example: Position has 100 liquidity. User removes 10. Fees are calculated on 10 liquidity units, but `fee_growth_inside_X_last` is reset to current global for the entire remaining 90. The 90 units lose their claim to fees from the period between last collection and now.

**Impact:** LPs who do partial removals lose uncollected fees on their remaining liquidity. Sophisticated users can exploit this by collecting fees before partial removal, while naive users lose value.

**Recommendation:** Calculate and distribute fees for the full position before updating the snapshot, similar to how `add_to_position` handles this.

---

### M-08: Stale Price Window is 50 Minutes

**Severity:** Medium
**Location:** `factory/src/state.rs:30`

```rust
pub const MAX_PRICE_AGE_SECONDS_BEFORE_STALE: u64 = 3000;
```

**Issue:** 3000 seconds = 50 minutes. In volatile crypto markets, a 50-minute-old price can deviate substantially from current market price. This is the threshold for the external Pyth ATOM/USD feed.

**Impact:** During high volatility, the oracle may accept severely outdated prices, leading to mispriced commits.

**Recommendation:** Reduce to 60-120 seconds for production, or at least under 300 seconds.

---

### L-01: `CreatorTokenInfo.decimal` Field Ignored

**Severity:** Low
**Location:** `factory/src/execute.rs:162`

```rust
decimals: 6, // hardcoded, ignoring msg token_info.decimal
```

The `CreatorTokenInfo` struct has a `decimal` field, but the factory always creates tokens with 6 decimals. This is confusing for creators who specify a different decimal.

---

### L-02: NFTs Not Burned on Full Removal

**Severity:** Low
**Location:** `pool/src/liquidity.rs:589-592`

When all liquidity is removed, the position is deleted from storage but the NFT is not burned. The comment says "we don't burn the NFT because the pool contract is not the owner." This leaves phantom NFTs that could confuse users or frontends.

---

### L-03: `query_cumulative_prices` Mutates Local Copy

**Severity:** Low
**Location:** `pool/src/query.rs:195-218`

The query function creates a mutable local copy of `pool_state`, updates it, but never persists the changes. The reserves are also overridden with live balances from `call_pool_info`, which bypasses internal accounting. This could return inconsistent data.

---

### L-04: Permissionless Pool Creation

**Severity:** Low
**Location:** `factory/src/execute.rs:133`

Anyone can call `Create` to create pools. Combined with the pool counter inflation (M-06), an attacker could spam pool creation to accelerate the mint curve decline.

---

### L-05: `TEMP_POOL_CREATION` Singleton

**Severity:** Low
**Location:** `factory/src/state.rs:10`

Only one pool can be created at a time since `TEMP_POOL_CREATION` is a single `Item`. While CosmWasm executes transactions serially per contract, this means pool creation is effectively serialized. Not a vulnerability, but a scalability constraint.

---

### L-06: Inconsistent Native Denom

**Severity:** Low
**Location:** `pool/src/asset.rs:20` vs `pool/src/liquidity.rs:36`

`UBLUECHIP_DENOM` is set to `"stake"` in `asset.rs` but `"ubluechip"` is hardcoded directly in `liquidity.rs` and other files. If the chain's denom is `"ubluechip"`, the `UBLUECHIP_DENOM` constant is never used correctly. This suggests the constant is dead code from testing and the hardcoded values are correct, but it's a maintenance hazard.

---

### L-07: Factory Migration No-Op

**Severity:** Low
**Location:** `factory/src/migrate.rs:20-23`

```rust
if stored_semver < Version::parse("2.0.0")? {
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    FACTORYINSTANTIATEINFO.save(deps.storage, &config)?;
}
```

This loads and re-saves the same config — it's a no-op. This was likely placeholder code for a schema migration that was never implemented.

---

## Centralization Risk Assessment

The protocol has significant centralization around the `factory_admin_address`:

| Capability | Timelock | Risk |
|-----------|----------|------|
| Change factory config (including admin address, fee wallets, thresholds) | 48 hours | Medium — could change fee recipients |
| Migrate all pools to arbitrary code | 48 hours | **Critical** — arbitrary code execution |
| Emergency withdraw all funds from any pool | None | **Critical** — instant rug pull |
| Pause/unpause pools | None | Medium — can freeze trading |
| Force rotate oracle pools | None | Medium — combined with H-01 |
| Update pool config (fees, oracle, thresholds) | None | High — instant oracle redirect |
| Update expand-economy config + withdraw funds | None | Medium — drain minting reserves |

**Assessment:** A compromised admin key leads to total loss of protocol funds through multiple independent paths. The 48-hour timelocks provide partial protection for config and migration changes, but `EmergencyWithdraw` and `UpdatePoolConfig` bypass these entirely.

---

## Malicious Actor Analysis

### Scenario 1: Compromised Admin Key
1. Call `EmergencyWithdraw` on every pool → drain all reserves instantly
2. OR: Change oracle to fake contract via `UpdatePoolConfig` → manipulate prices → drain via commits
3. OR: Propose migration to malicious code → wait 48h → drain via migrated contracts

### Scenario 2: Adversarial Validator
1. Predict oracle pool selection (H-01) using block hash
2. Manipulate selected pools' reserves before `UpdateOraclePrice`
3. Exploit spot price fallback after rotation (H-02)
4. Profit from mispriced commits

### Scenario 3: External Attacker (No Privileged Access)
1. Front-run threshold crossing by detecting a commit that will cross, and submitting a larger commit in the same block to capture a disproportionate share of commit rewards
2. Spam pool creation (L-04) to inflate pool counter and reduce future mint amounts (M-06)
3. Exploit gross/net accounting bug (C-02) to create pools with phantom reserves

### Scenario 4: MEV Extraction
1. Sandwich swaps: front-run large swaps with a swap in the same direction, back-run with opposite
2. Commit timing: monitor mempool for threshold-crossing commits, front-run to capture the crossing position
3. Oracle update timing: sandwich `UpdateOraclePrice` calls with reserve manipulation

---

## Recommendations Summary

### Immediate (Pre-Launch)
1. **Fix C-02/C-03:** Correct gross-vs-net accounting in commit tracking
2. **Fix C-01:** Add access control to `UpdateConfig` execution
3. **Fix C-04:** Remove or redesign `EmergencyWithdraw` to protect LP funds
4. **Fix H-04:** Add timelock to oracle address changes

### Short-Term
5. **H-01/H-02:** Improve oracle randomness and eliminate spot price fallback
6. **H-03:** Fund distribution bounties from a separate allocation
7. **M-04:** Add staleness checks to commit price conversions
8. **M-05:** Use explicit token type info instead of addr_validate heuristic
9. **M-08:** Reduce stale price window to < 5 minutes

### Long-Term
10. Transition admin key to multi-sig or DAO governance
11. Implement circuit breakers for large price deviations
12. Add on-chain upgrade preview mechanism
13. Consider formal verification of core accounting invariants
14. Add comprehensive integration tests for threshold crossing edge cases

---

*This audit report is based on manual code review as of 2026-02-12. It does not constitute a guarantee of security. Smart contract auditing is an inherently incomplete process and this report should be treated as one input among many in the protocol's security posture.*
