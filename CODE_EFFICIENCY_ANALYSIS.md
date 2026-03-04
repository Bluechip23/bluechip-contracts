# Code Efficiency Analysis — Bluechip Contracts

A review of the Rust/CosmWasm codebase looking for safe ways to reduce line count
through deduplication and minor consolidation. All suggestions preserve existing
semantics and safety guarantees.

---

## 1. Duplicated Ratio-Deviation Calculation (~50 lines saved)

**Files:** `pool/src/liquidity.rs` — lines 513-560 and 777-831

The ratio-deviation-in-basis-points calculation is copy-pasted between
`remove_all_liquidity` and `remove_partial_liquidity`. Both blocks are ~45 lines
of identical logic:

```rust
// Appears twice, nearly verbatim:
if let Some(max_deviation_bps) = max_ratio_deviation_bps {
    if let (Some(min0), Some(min1)) = (min_amount0, min_amount1) {
        // ... 30+ lines of deviation math ...
    }
}
```

**Suggestion:** Extract into a shared helper in `liquidity_helpers.rs`:

```rust
pub fn check_ratio_deviation(
    actual_amount0: Uint128,
    actual_amount1: Uint128,
    min_amount0: Option<Uint128>,
    min_amount1: Option<Uint128>,
    max_ratio_deviation_bps: Option<u16>,
) -> Result<(), ContractError> { ... }
```

This would reduce each call site to ~1 line instead of ~45.

---

## 2. Repeated Fee-Collect-and-Transfer Pattern (~30 lines saved)

**File:** `pool/src/liquidity.rs`

The following sequence appears in `execute_collect_fees` (lines 210-273),
`add_to_position` (lines 326-455), and `remove_all_liquidity` (lines 562-656):

1. Calculate `fees_owed_0` and `fees_owed_1` via `calculate_fees_owed` + cap
   against fee reserves
2. Build BankMsg::Send for bluechip fees
3. Build WasmMsg::Execute for CW20 fees

The fee calculation + message-building portion could be extracted:

```rust
fn collect_position_fees(
    position: &Position,
    pool_fee_state: &PoolFeeState,
    pool_info: &PoolInfo,
    recipient: &Addr,
) -> Result<(Uint128, Uint128, Vec<CosmosMsg>), ContractError> { ... }
```

This would cut ~10-15 lines from each of the three call sites.

---

## 3. Repeated Reserve-Ordering Destructure (~10 lines saved)

**File:** `factory/src/internal_bluechip_price_oracle.rs` — `calculate_weighted_price_with_atom`

The expression:
```rust
let (bluechip_reserve, other_reserve) = if is_bluechip_second {
    (pool_state.reserve1, pool_state.reserve0)
} else {
    (pool_state.reserve0, pool_state.reserve1)
};
```
appears **three times** in the same function (lines 372-376, 382-386, 395-399).

**Suggestion:** Compute it once right after determining `is_bluechip_second` and
reuse the binding. This is straightforward — just move it up to where
`is_bluechip_second` is determined (after line 327) and use it in all three spots.

---

## 4. `get_usd_value` Is a Subset of `get_usd_value_with_staleness_check` (~12 lines saved)

**File:** `pool/src/swap_helper.rs` — lines 121-157

`get_usd_value` (line 146) is identical to `get_usd_value_with_staleness_check`
(line 121) minus the staleness check. They both load `POOL_INFO`, make the same
factory query, and return `response.amount`.

**Suggestion:** Make the staleness check optional:

```rust
pub fn get_usd_value(
    deps: Deps,
    bluechip_amount: Uint128,
    staleness_check: Option<u64>, // pass current_block_time to enable check
) -> StdResult<Uint128> { ... }
```

Eliminates the second function entirely.

---

## 5. `bluechip_to_usd` / `usd_to_bluechip` Share ~90% Logic (~15 lines saved)

**File:** `factory/src/internal_bluechip_price_oracle.rs` — lines 669-726

These two functions are structurally identical:
- Load oracle, get cached price, check for zero
- Multiply/divide (the only difference is which operand gets `PRICE_PRECISION`)
- Return `ConversionResponse`

**Suggestion:** A single internal conversion helper with a direction enum/bool:

```rust
fn convert_with_oracle(
    deps: Deps, env: Env, amount: Uint128, to_usd: bool,
) -> StdResult<ConversionResponse> { ... }
```

---

## 6. `let result = ...; result` Anti-pattern (~6 lines saved)

**File:** `pool/src/liquidity.rs` — lines 936-948, 969-978, 1000-1012

Three wrapper functions (`execute_add_to_position`, `execute_remove_all_liquidity`,
`execute_remove_partial_liquidity`) bind the inner function call to `result` and
then immediately return it:

```rust
let result = remove_all_liquidity(...);
result
```

This can just be the tail expression directly — no intermediate binding needed.

---

## 7. Dead Commented-Out Code (~10 lines saved)

**File:** `pool/src/liquidity.rs` — lines 588-601

`remove_all_liquidity` contains a commented-out NFT burn block followed by
`let messages: Vec<CosmosMsg> = vec![];`. The comment block serves as a note about
why the burn isn't done, but the dead code could be condensed to a single-line
comment. The empty `messages` vec is also unused since `response` is built with
`.add_messages(messages)` where messages is guaranteed empty.

---

## 8. Duplicate Slippage-Check Blocks (~15 lines saved)

**File:** `pool/src/liquidity.rs`

This exact 6-line pattern appears 5 times across the liquidity functions:

```rust
if let Some(min0) = min_amount0 {
    if actual_amount0 < min0 {
        return Err(ContractError::SlippageExceeded {
            expected: min0, actual: actual_amount0, token: "bluechip".to_string(),
        });
    }
}
// + same for min1/amount1
```

**Suggestion:** A one-liner helper:

```rust
fn check_slippage(actual: Uint128, min: Option<Uint128>, token: &str) -> Result<(), ContractError>
```

---

## Summary

| # | What | Where | Est. Lines Saved |
|---|------|-------|-----------------|
| 1 | Ratio deviation helper | `liquidity.rs` | ~50 |
| 2 | Fee-collect-and-transfer helper | `liquidity.rs` | ~30 |
| 3 | Reserve ordering computed once | `internal_bluechip_price_oracle.rs` | ~10 |
| 4 | Consolidate USD value functions | `swap_helper.rs` | ~12 |
| 5 | Unify conversion direction | `internal_bluechip_price_oracle.rs` | ~15 |
| 6 | Remove `let result = ...; result` | `liquidity.rs` | ~6 |
| 7 | Remove dead commented-out code | `liquidity.rs` | ~10 |
| 8 | Slippage check helper | `liquidity.rs` | ~15 |
| | **Total** | | **~148** |

### What Was NOT Flagged

- **Checked arithmetic verbosity** — the `.map_err(|_| ...)` chains on every
  `checked_add`/`checked_mul` call add lines but are essential for meaningful
  error messages in a financial smart contract. Not worth reducing.
- **Oracle SHA256 pool selection** — while a simpler `block_height % count`
  approach uses fewer lines, the SHA256 approach provides better distribution
  and manipulation resistance. The tradeoff favors security.
- **Per-function state loading** — several functions load `POOL_INFO`,
  `POOL_STATE`, and `POOL_FEE_STATE` separately. Bundling them into a single
  load would save lines but reduce clarity about which state each code path
  actually needs.
- **Error enum variants** — both contracts have thorough error enums. These
  add lines but provide excellent debugging context on-chain.
