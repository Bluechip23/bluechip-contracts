# H14 Split Refactor — Execution Plan

Splitting the unified `pool/` crate into **`creator-pool`** (the original
two-phase pool) and **`standard-pool`** (a plain xyk pool with no commit
phase), backed by a shared library crate **`pool-core`** that contains
every handler both pools use verbatim.

This plan is authored in five sequential steps. Step 1 (this section) is
architecture and context; Steps 2–5 are the mechanical changes. Each
step is scoped so it can land as a discrete commit and be reviewed in
isolation.

---

## Step 1 — Foundation & architecture

### 1.1 Why split

The security audit found that every CRITICAL and the majority of HIGH
findings live in the commit-phase code path:

| Finding | Code surface | Applies to standard pools? |
|---|---|---|
| C1 — oracle bootstrap deadlock | commit pricing | no |
| C3 — threshold-crossing MEV | `process_threshold_crossing_with_excess` | no |
| C4 — dust-commit ledger bloat | `commit` entry + COMMIT_LEDGER | no |
| C5 — pre-threshold fee trap | commit fee handling | no |
| H1/H2 — oracle manipulation | factory-oracle interaction during commit | no |
| H8 — NFT-transfer fee forfeit | LP position logic | **yes** (shared) |

Standard pools inherit H8 (fee forfeit on NFT transfer) because that's
LP-position mechanics, but everything else listed above is commit-phase
machinery a standard pool has no reason to carry. Splitting lets
standard-pool users run on a smaller, audit-cleaner wasm; any future
exploit in commit-phase code physically cannot reach them.

### 1.2 What gets deployed

Exactly two wasms on-chain, plus the factory:

- `creator-pool.wasm` — new code_id, replaces today's `pool.wasm`
- `standard-pool.wasm` — new code_id, new contract
- `factory.wasm` — updated to track both code_ids

`pool-core` is **not a contract**. It's a Rust library crate with no
`#[entry_point]`s. At build time each consumer statically links it into
their own wasm. On-chain there are two pool wasms; off-chain there is
one `pool-core` source tree both consumers use.

### 1.3 Target file tree

```
bluechip-contracts/
├── packages/
│   ├── easy-addr/                 (unchanged)
│   ├── pool-factory-interfaces/   (unchanged)
│   └── pool-core/                 (NEW — library crate; skeleton already on branch)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── error.rs           (already moved here)
│           ├── asset.rs           (Step 2)
│           ├── state.rs           (Step 2 — shared subset)
│           ├── msg.rs             (Step 2 — shared subset)
│           ├── swap.rs            (Step 2 — AMM math)
│           ├── liquidity.rs       (Step 3 — LP op bodies)
│           ├── liquidity_helpers.rs (Step 3 — shared subset)
│           ├── generic.rs         (Step 3 — shared utilities)
│           ├── admin.rs           (Step 3 — pause / emergency-withdraw)
│           └── query.rs           (Step 3 — shared queries)
│
├── creator-pool/                  (was `pool/`; renamed in Step 4)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── contract.rs            (commit-pool instantiate + execute dispatch)
│       ├── commit.rs              (stays — commit-phase handlers)
│       ├── oracle_conversion.rs   (was part of swap_helper.rs; commit-only)
│       ├── state.rs               (commit-only storage + re-exports from pool-core)
│       ├── msg.rs                 (ExecuteMsg with all variants + re-exports)
│       ├── query.rs               (commit-only queries + re-exports)
│       ├── error.rs               (one-line re-export of pool-core)
│       ├── admin_recovery.rs      (recover_stuck_states — commit-only)
│       ├── threshold_helpers.rs   (trigger_threshold_payout, process_distribution_batch)
│       ├── mock_querier.rs        (test-only)
│       └── testing/               (existing tests, import-paths updated)
│
├── standard-pool/                 (NEW contract crate — Step 4)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── contract.rs            (thin entry points → pool-core)
│       ├── msg.rs                 (ExecuteMsg sans commit variants)
│       ├── query.rs               (QueryMsg sans commit variants)
│       ├── error.rs               (re-export pool-core)
│       └── testing/               (new integration tests — Step 5)
│
├── factory/                       (updated — Step 4)
│   └── src/                       (adds standard_pool_wasm_contract_id)
│
├── expand-economy/                (unchanged)
├── mockoracle/                    (unchanged)
└── router/                        (unchanged — imports pool_factory_interfaces only)
```

### 1.4 Build-dependency direction

Strict hierarchy, zero cycles:

```
pool-factory-interfaces       (no deps on any contract crate)
         ▲
         │
      pool-core                (library; depends on pool-factory-interfaces only)
         ▲
    ┌────┴──────┐
creator-pool   standard-pool   (contract crates; depend on pool-core + pool-factory-interfaces)
         ▲
         │
      factory                  (depends on pool-factory-interfaces only — NOT on either pool crate)
```

- `pool-core` imports **nothing** from creator-pool or standard-pool.
  If you find yourself wanting to, the item you're reaching for is
  commit-phase-specific and should stay in creator-pool.
- `factory` instantiates pools via `WasmMsg::Instantiate { code_id, msg }`
  where `msg` is serialized JSON. The factory **does not depend** on
  either pool crate at the Rust level — it only knows the code_ids and
  wire-format structs (which live in `pool-factory-interfaces`).
- `router` continues to depend on `pool_factory_interfaces` only.

### 1.5 Wire-format decisions

Each pool's `instantiate` accepts a **flat struct**, not a tagged enum:

- `creator-pool::instantiate(msg: CreatePoolReplyMsg)` — the struct the
  factory already builds. Identical wire shape to what ships today
  pre-H14-Commit-3.
- `standard-pool::instantiate(msg: StandardPoolInstantiateMsg)` — the
  struct added in H14 Commit 2 (`pool_factory_interfaces::StandardPoolInstantiateMsg`).

The `PoolInstantiateMsg::Commit(...) / Standard(...)` tagged enum added
in H14 Commit 3 goes away (Step 4). The factory's mirror
`PoolInstantiateWire` enum also goes away. Rationale: each wasm only
accepts one shape, so dispatching on a variant at runtime is dead code.
Factory sends the right flat struct to the right code_id directly.

### 1.6 Current state on this branch

Already on `claude/audit-cosmwasm-defi-amm-YnAvG` (commits visible via
`git log`):

| Commit | Subject | Status vs. final plan |
|---|---|---|
| `a37c763` | `pool-core` skeleton | **keeps** — empty library ready to fill |
| `b71b891` | `error.rs` extracted to pool-core | **keeps** — stays as-is |
| `2f4af00` | H14 4b: pair-shape refactor in `pool/src/` | **keeps** — code moves verbatim into `pool-core` in Step 3 |
| `fee40b8` | `TokenType::Bluechip` → `::Native` rename | **keeps** — pure naming win |
| `8d1e49c` | H14 C3: `PoolInstantiateMsg` enum + `require_commit_pool` guards | **partially reverts** — Step 4 flattens msg back to struct and deletes the guards |
| `ff6d15c` | H14 C2: Factory `CreateStandardPool` + `SetAnchorPool` | **mostly keeps** — Step 4 trims `PoolInstantiateWire` and points at new code_id |
| `969283f` | H14 C1: `PoolKind` scaffolding | **partially reverts** — factory side stays (`pool_kind` on `PoolDetails`, oracle filter); pool side (`POOL_KIND` Item, `load_pool_kind`, `require_commit_pool`) deletes in Step 4 |

### 1.7 What *stays* from the earlier unified-wasm work

Everything below landed before the split decision and remains correct /
useful under the split architecture:

- **H3 — canonical `bluechip_denom` pinning** (factory config field + validation)
- **H5 — dead reply-handler code removal** (atomic reply_on_success chain)
- **TokenType::Native rename** with `#[serde(rename = "bluechip")]` preserving wire format
- **Pair-shape generalization** (Commit 4b): `collect_deposit_side`,
  `build_transfer_msg`, per-asset dispatch in `build_fee_transfer_msgs`,
  per-side refund tracking in `DepositPrep`. All of this moves to
  `pool-core` in Step 3 and is used by BOTH pool kinds.
- **Factory `PoolKind` enum and `pool_kind` on `PoolDetails`.** The
  *factory* still tracks which kind each pool is so `get_eligible_creator_pools`
  can filter standard pools out of oracle sampling and `SetAnchorPool`
  can require a standard-pool-kind argument. The *pool side* stops
  needing a runtime kind discriminator (the wasm it's running in IS
  the kind).
- **Factory `CreateStandardPool` + `SetAnchorPool` ExecuteMsg variants**,
  fee logic (USD-denominated with bluechip fallback), and the standard-pool
  reply chain (`MINT_STANDARD_NFT`, `FINALIZE_STANDARD_POOL`).
- **`NotifyThresholdCrossed` defensive guard** against standard pools
  — stays as a belt-and-braces check, even though the standard-pool wasm
  physically cannot send this message.

### 1.8 What *reverts* during the refactor

These were added to make one-wasm-two-behaviors work at runtime. With
split wasms, they become dead complexity:

| Item | Where it lives today | Removed in step |
|---|---|---|
| `PoolKind` re-export on pool side | `pool/src/state.rs` | Step 4 |
| `POOL_KIND` storage Item | `pool/src/state.rs` | Step 4 |
| `load_pool_kind` helper | `pool/src/state.rs` | Step 4 |
| `require_commit_pool` dispatch guard | `pool/src/contract.rs` | Step 4 |
| `PoolInstantiateMsg::Commit(...) / Standard(...)` enum | `pool/src/msg.rs` | Step 4 (flattens back to struct) |
| `is_standard_pool: Option<bool>` on `CommitPoolInstantiateMsg` | `pool/src/msg.rs` | Step 4 (dead flag from pre-H14) |
| `PoolInstantiateWire` enum | `factory/src/pool_creation_reply.rs` | Step 4 |
| Factory's admin-only gate on `is_standard_pool: Some(true)` in `Create` | `factory/src/execute.rs` | Step 4 (entire `is_standard_pool` flag on CreatePool goes) |

### 1.9 Naming conventions

- Library crate: **`pool-core`** (directory: `packages/pool-core/`)
- Commit-pool contract: **`creator-pool`** (directory: `creator-pool/`,
  renamed from `pool/` via `git mv` in Step 4 to preserve file history)
- Standard-pool contract: **`standard-pool`** (directory: `standard-pool/`, new in Step 4)
- Factory: **`factory`** (unchanged)

Function/type/storage-key names keep their existing spellings wherever
possible to minimize noise in the diff. The mechanical changes are
mostly `use crate::X` → `use pool_core::X` and `pub(crate) fn` →
`pub fn` on items that now cross a crate boundary.

### 1.10 Step-by-step execution sequence

Each numbered step is a commit (or small group of commits if it grows
too large). We pause between steps for you to `cargo check` locally,
fix any compile errors I missed, and confirm before moving on.

1. **Step 1 — Foundation & architecture** (this doc)
2. **Step 2 — pool-core part 1**: state + asset + swap math + shared msg types
3. **Step 3 — pool-core part 2**: liquidity + helpers + admin + query
4. **Step 4 — Standard-pool crate + factory dual-code_id + creator-pool reverts**:
   - `git mv pool/ creator-pool/`
   - new `standard-pool/` crate
   - factory gains `standard_pool_wasm_contract_id`, loses `PoolInstantiateWire`
   - creator-pool drops `POOL_KIND` / `require_commit_pool` / `PoolInstantiateMsg` enum / `is_standard_pool` flag
5. **Step 5 — Tests + deploy scripts**: integration tests for standard-pool
   across three pair shapes; deploy scripts upload both wasms; verification
   checklist.

### 1.11 Verification at each boundary

After each step, run locally:

```
cargo check --workspace
cargo test --workspace
```

Schema generation (if schema artifacts are committed):

```
cargo run --bin schema -p pool-core      # if schema bin exists
cargo run --bin schema -p creator-pool
cargo run --bin schema -p standard-pool
cargo run --bin schema -p factory
```

Optimizer (final validation that both contract wasms build):

```
docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source="$(basename "$(pwd)")_cache",target=/code/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0
```

Both `creator_pool.wasm` and `standard_pool.wasm` should appear in
`artifacts/` at the end, with sizes broadly consistent (standard-pool
should be smaller — no commit.rs in its wasm).

---

Steps 2–5 will be appended to this document in subsequent turns.

## Step 2a — `state.rs` split

Source: `pool/src/state.rs` (338 lines). Target split:

- `packages/pool-core/src/state.rs` (shared items + structs)
- `pool/src/state.rs` (commit-specific items + re-export of shared from
  pool-core, so every existing `use crate::state::X;` in the creator-pool
  crate keeps resolving)

### Items that MOVE to `pool-core/src/state.rs`

Structs used by shared code paths:

| Struct | Notes |
|---|---|
| `TokenMetadata` | NFT position metadata — shared |
| `PoolState` | reserves, cumulative prices, NFT-accept flag — shared |
| `PoolFeeState` | fee_growth + fee_reserve — shared |
| `PoolSpecs` | lp_fee, min_commit_interval, usd_payment_tolerance_bps — shared |
| `PoolInfo` | pool_id, token_address, position_nft_address, factory_addr — shared |
| `PoolDetails` (pool-side) | asset_infos + contract_addr + pool_type; `query_pools` impl moves with it — shared |
| `Position` | LP position record — shared |
| `PoolAnalytics` + `Default` impl | counters — shared |
| `CreatorFeePot` + `Default` impl | struct is shared because emergency_withdraw sweeps it; Item is shared too (standard pool never writes, but `may_load` returns `None`) |
| `EmergencyWithdrawalInfo` | audit-trail struct — shared |
| `ExpectedFactory` | factory-address pin — shared |
| `PoolCtx` + `impl PoolCtx::load` | bundle loader for the four hot-path items — shared |

Storage Items that move:

| Item | Key | Notes |
|---|---|---|
| `POOL_INFO` | `"pool_info"` | shared |
| `POOL_STATE` | `"pool_state"` | shared |
| `POOL_FEE_STATE` | `"pool_fee_state"` | shared |
| `POOL_SPECS` | `"pool_specs"` | shared |
| `POOL_ANALYTICS` | `"pool_analytics"` | shared |
| `LIQUIDITY_POSITIONS` | `"positions"` | shared |
| `OWNER_POSITIONS` | `"owner_positions"` | shared |
| `NEXT_POSITION_ID` | `"next_position_id"` | shared |
| `POOL_PAUSED` | `"pool_paused"` | shared |
| `EMERGENCY_WITHDRAWAL` | `"emergency_withdrawal"` | shared |
| `PENDING_EMERGENCY_WITHDRAW` | `"pending_emergency_withdraw"` | shared |
| `EMERGENCY_DRAINED` | `"emergency_drained"` | shared |
| `EXPECTED_FACTORY` | `"expected_factory"` | shared |
| `REENTRANCY_LOCK` | `"rate_limit_guard"` | shared |
| `USER_LAST_COMMIT` | `"user_last_commit"` | shared (rate limit applies to both kinds) |
| `IS_THRESHOLD_HIT` | `"threshold_hit"` | shared (shared `query_check_commit` reads it; standard pool sets it `true` at instantiate) |
| `CREATOR_FEE_POT` | `"creator_fee_pot"` | shared (emergency_withdraw sweeps it; standard pool's stays empty) |
| `COMMITFEEINFO` | `"fee_info"` | shared (emergency_withdraw reads `bluechip_wallet_address` off it for the drain recipient; standard pool saves zero-valued placeholder at instantiate) |
| `ORACLE_INFO` | `"oracle_info"` | shared (struct + Item both — though its `oracle_addr` field is effectively dead code per audit H9; leave as-is for now, separate cleanup) |

Constants that move:

| Constant | Value | Notes |
|---|---|---|
| `MINIMUM_LIQUIDITY` | `Uint128::new(1000)` | shared |
| `EMERGENCY_WITHDRAW_DELAY_SECONDS` | `86_400` | shared |

### Items that STAY in `pool/src/state.rs` (creator-pool only)

Storage Items (commit-phase only):

- `USD_RAISED_FROM_COMMIT`, `NATIVE_RAISED_FROM_COMMIT`
- `COMMIT_LEDGER`, `COMMIT_INFO`
- `THRESHOLD_PROCESSING`, `THRESHOLD_PAYOUT_AMOUNTS`, `COMMIT_LIMIT_INFO`
- `LAST_THRESHOLD_ATTEMPT`, `PENDING_FACTORY_NOTIFY`
- `DISTRIBUTION_STATE`, `CREATOR_EXCESS_POSITION`

Structs (commit-phase only):

- `Committing`, `DistributionState`, `CreatorExcessLiquidity`
- `ThresholdPayoutAmounts`, `CommitLimitInfo`, `RecoveryType`

Constants (commit-phase only):

- `REPLY_ID_FACTORY_NOTIFY_INITIAL`, `REPLY_ID_FACTORY_NOTIFY_RETRY`
- `DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION`, `DEFAULT_MAX_GAS_PER_TX`
- `MAX_DISTRIBUTIONS_PER_TX`, `DISTRIBUTION_STALL_TIMEOUT_SECONDS`

Scheduled to be **removed entirely in Step 4** (dead with split wasms):

- `pub use pool_factory_interfaces::PoolKind;` re-export
- `POOL_KIND: Item<PoolKind>` storage Item
- `pub fn load_pool_kind(...)` helper

Leave these in place during Step 2a; Step 4 is where they go. Keeping
them until then avoids breaking `require_commit_pool` and the tagged-enum
instantiate dispatch, both of which are still live code today.

### `pool/src/state.rs` after the split

Top of file becomes:

```rust
// Shared structs, Items, and constants live in pool-core. This glob
// re-export preserves every `use crate::state::X;` import in the
// creator-pool crate (including tests) without touching call sites.
// Commit-phase-specific items are defined below.
pub use pool_core::state::*;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, StdResult, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};
use crate::msg::CommitFeeInfo;  // CommitFeeInfo itself is moved in Step 2d
```

...followed by the commit-only Items, structs, and constants listed above.

### `packages/pool-core/src/state.rs` imports

The new file needs:

```rust
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, StdResult, Storage, Uint128, Timestamp, QuerierWrapper};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::asset::{PoolPairType, TokenInfo, TokenType};
use crate::msg::CommitFeeInfo;  // CommitFeeInfo lives in pool_core::msg after Step 2d
```

Note the dependency on `CommitFeeInfo` — `COMMITFEEINFO` Item's value
type. Step 2d is where `CommitFeeInfo` moves to `pool-core/src/msg.rs`.
If you execute 2a before 2d, temporarily import it from `crate::msg::CommitFeeInfo`
in creator-pool and re-export from pool-core via a forward-declaration,
or just do 2a and 2d together. Simplest: commit them in one PR.

### Update `pool-core/src/lib.rs`

Add `pub mod state;` alongside the existing `pub mod error;`.

### Cargo.toml changes

None for Step 2a. `pool-core` already depends on `pool-factory-interfaces`
and `cw-storage-plus` (added in C1 skeleton). `pool/` already depends on
`pool-core` (added in the error.rs extraction).

### Expected compile-error patterns

When you `cargo check -p pool-core -p pool` after executing 2a:

1. **Missing `CommitFeeInfo` in pool-core state.rs** — if 2d hasn't
   landed yet. Either land 2d first, or temporarily declare a stub
   `pub struct CommitFeeInfo { ... }` in pool-core/src/msg.rs (with
   identical fields) that 2d will replace.

2. **Orphan implementation** of `PoolDetails::query_pools` — `PoolDetails`
   is moving to pool-core but `query_pools` calls
   `pool_factory_interfaces::asset::query_pools`, which is fine. Should
   Just Work.

3. **Visibility errors** on items that were `pub(crate)` and now cross a
   crate boundary. Search for `pub(crate)` in `pool/src/state.rs` (there
   are none currently — every storage Item is already `pub`), so this is
   a non-issue for 2a.

4. **Duplicate definition** if you forget to delete the original in
   `pool/src/state.rs`. After `pub use pool_core::state::*;` at the top
   of `pool/src/state.rs`, a local `pub const POOL_STATE: ...` re-declaration
   would collide. Delete the originals.

5. **Tests** — `pool/src/testing/*.rs` files use `use crate::state::X;`.
   These resolve through the glob re-export, so no test file changes are
   required for 2a.

### Verification after 2a

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool   # existing tests should still pass; no behavior change
```

If step 2a is committed alone (before 2d), expect one compile error
around `CommitFeeInfo`. The fix is either (a) co-land 2d, or (b) stub
`CommitFeeInfo` in pool-core temporarily as described above.

### Suggested commit message

```
H14 split (2a/N): extract shared state items to pool-core

Moves every storage key, struct, and constant in pool/src/state.rs that
is used by both pool kinds into packages/pool-core/src/state.rs.
Commit-phase-specific items (COMMIT_LEDGER, THRESHOLD_PAYOUT_AMOUNTS,
DISTRIBUTION_STATE, etc.) stay in pool/src/state.rs behind a glob
re-export of pool_core::state::* so existing call sites keep resolving.

See docs/H14_SPLIT_PLAN.md#step-2a for the item-by-item mapping.

No behavior change. Creator-pool wasm should produce an identical
artifact after this commit.
```

## Step 2b — `asset.rs` move

Source: `pool/src/asset.rs` (104 lines). This is mostly a wholesale
move — every item is used by shared swap/liquidity/fee-message-building
code that will live in pool-core.

### Items that MOVE to `pool-core/src/asset.rs`

| Item | Kind | Notes |
|---|---|---|
| `pub use pool_factory_interfaces::asset::*;` | re-export | preserves `TokenType`, `TokenInfo`, `PoolPairType`, `get_native_denom`, `native_asset*`, `token_asset*`, `query_pools`, etc. as `pool_core::asset::*` |
| `UBLUECHIP_DENOM` | constant | shared default for the canonical bluechip denom |
| `TokenInfoPoolExt` trait | trait | 3 methods: `deduct_tax`, `into_msg`, `confirm_sent_native_balance` |
| `impl TokenInfoPoolExt for TokenInfo` | impl block | moves with the trait |
| `PoolPairInfo` struct + `impl PoolPairInfo::query_pools` | struct + impl | shared — used in query responses by both pools |
| `call_pool_info(deps, pool_info)` function | function | used by `query_cumulative_prices` (shared query); depends on `PoolInfo` which lives in `pool-core::state` after 2a |

Nothing stays in `pool/src/asset.rs` except the re-export shim.

### `pool/src/asset.rs` after the split

Replace the entire file (104 lines) with a single re-export:

```rust
//! Re-export of `pool_core::asset::*`. Preserves every existing
//! `use crate::asset::X;` import in the creator-pool crate, including
//! the `TokenInfoPoolExt` trait import needed for method-call resolution
//! on `TokenInfo` values.
pub use pool_core::asset::*;
```

### `packages/pool-core/src/asset.rs` imports

```rust
pub use pool_factory_interfaces::asset::*;

use crate::state::PoolInfo;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, MessageInfo, QuerierWrapper,
    StdError, StdResult, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_utils::must_pay;
```

Note the `use crate::state::PoolInfo;` — depends on Step 2a having moved
`PoolInfo` into `pool-core/src/state.rs` already. Correct dependency
order: **land 2a before 2b**.

### Update `pool-core/src/lib.rs`

Add `pub mod asset;` alongside existing `pub mod error;` and (after 2a)
`pub mod state;`. Order:

```rust
pub mod error;
pub mod state;   // 2a
pub mod asset;   // 2b  — depends on state
```

### Cargo.toml changes

None. `pool-core` already has `pool-factory-interfaces`, `cosmwasm-std`,
`cw20`, `cw-utils` from the C1 skeleton.

### Expected compile-error patterns after 2b

1. **Trait-method resolution in creator-pool** — Rust requires the trait
   to be in scope for method calls like `offer_asset.into_msg(&querier, to)`.
   Existing creator-pool files do `use crate::asset::TokenInfoPoolExt;`
   or rely on the glob re-export. Since `pool/src/asset.rs` now
   `pub use pool_core::asset::*;`, those imports resolve to
   `pool_core::asset::TokenInfoPoolExt`. Should Just Work.

2. **Circular import risk** — `pool-core::asset` depends on
   `pool-core::state::PoolInfo`. `pool-core::state` does NOT depend on
   `pool-core::asset` (it uses `pool_factory_interfaces::asset::TokenType`
   directly). No cycle. Confirmed clean.

3. **Test files** — creator-pool tests reference `crate::asset::TokenInfoPoolExt`,
   `crate::asset::call_pool_info`, etc. Glob re-export in `pool/src/asset.rs`
   covers them.

### Verification after 2b

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool   # behavior-preserving; all existing tests pass
```

### Suggested commit message

```
H14 split (2b/N): extract asset.rs to pool-core

Moves pool/src/asset.rs wholesale to packages/pool-core/src/asset.rs:
TokenInfoPoolExt trait + impl, PoolPairInfo struct, call_pool_info
helper, UBLUECHIP_DENOM constant, and the pool_factory_interfaces::asset
glob re-export.

pool/src/asset.rs becomes a one-line `pub use pool_core::asset::*;`
shim so every `use crate::asset::X;` import (including trait-method
imports for TokenInfo) keeps resolving unchanged.

Depends on Step 2a (state.rs split) for the PoolInfo type that
call_pool_info consumes.

No behavior change.
```
