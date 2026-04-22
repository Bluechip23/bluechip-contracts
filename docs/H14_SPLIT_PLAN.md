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

## Step 2c — `swap_helper.rs` split

Source: `pool/src/swap_helper.rs` (300 lines). This is the first file in
the plan that truly **splits** — half goes to pool-core, half stays in
creator-pool. The split line is "AMM math" (pair-shape-agnostic,
stateless-or-state-only) vs. "oracle integration" (calls the factory's
internal oracle, only ever used by commit flow).

### Items that MOVE to `pool-core/src/swap.rs`

Pure AMM math — shared by simple_swap, post-threshold commit, deposit
liquidity, remove liquidity, and every simulation/reverse-simulation
query on both pool kinds.

| Item | Kind | Notes |
|---|---|---|
| `DEFAULT_SLIPPAGE` | `pub const &str = "0.005"` | currently lives in `pool/src/contract.rs`; **also moves** to pool-core with the math that consumes it |
| `compute_swap` | fn | constant-product x·y=k return + spread + commission |
| `compute_offer_amount` | fn | inverse — required offer for a desired ask |
| `assert_max_spread` | fn | slippage guard used by every swap path |
| `update_price_accumulator` | fn | TWAP cumulative updater; mutates a `PoolState` ref |

### Items that STAY in `pool/src/swap_helper.rs` (creator-pool only)

Oracle integration — these make cross-contract queries to the factory's
internal oracle. Standard pools never call any of these (commit-phase
USD valuation is meaningless for them).

| Item | Kind | Notes |
|---|---|---|
| `MAX_ORACLE_STALENESS_SECONDS` | `pub const u64 = 90` | aligns with factory's own staleness threshold |
| `ORACLE_PRICE_PRECISION` | `pub const u128 = 1_000_000` | mirrors `factory::internal_bluechip_price_oracle::PRICE_PRECISION` |
| `get_usd_value_with_staleness_check` | fn | thin wrapper around `get_oracle_conversion_with_staleness` |
| `get_oracle_conversion_with_staleness` | fn | bluechip → USD conversion via factory query |
| `usd_to_bluechip_at_rate` | fn | inverse conversion at a pre-captured rate |
| `get_bluechip_value` | fn | USD → bluechip convenience |
| `FactoryQueryWrapper` | private enum | serialization wrapper for `FactoryQueryMsg::InternalBlueChipOracleQuery` |

### `pool/src/swap_helper.rs` after the split

```rust
//! Oracle-integration helpers (commit-phase only). The pure AMM math
//! that used to live in this file (`compute_swap`, `compute_offer_amount`,
//! `assert_max_spread`, `update_price_accumulator`, `DEFAULT_SLIPPAGE`)
//! now lives in `pool_core::swap` and is re-exported below so existing
//! imports like `use crate::swap_helper::compute_swap;` keep resolving.
pub use pool_core::swap::*;

use crate::state::POOL_INFO;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Deps, StdError, StdResult, Uint128};
use pool_factory_interfaces::{ConversionResponse, FactoryQueryMsg};

#[cw_serde]
enum FactoryQueryWrapper {
    InternalBlueChipOracleQuery(FactoryQueryMsg),
}

pub const MAX_ORACLE_STALENESS_SECONDS: u64 = 90;
pub const ORACLE_PRICE_PRECISION: u128 = 1_000_000;

// ... existing oracle-helper fn bodies unchanged ...
```

### `packages/pool-core/src/swap.rs` imports

```rust
use crate::error::ContractError;
use crate::state::PoolState;
use cosmwasm_std::{Decimal, Decimal256, Fraction, StdError, StdResult, Uint128, Uint256};
use std::str::FromStr;

pub const DEFAULT_SLIPPAGE: &str = "0.005";
```

### Dependency on `decimal2decimal256` (cross-step gotcha)

`compute_swap`, `compute_offer_amount`, and `assert_max_spread` call
`decimal2decimal256`, which currently lives in `pool/src/generic_helpers.rs`.
That helper moves to `pool-core/src/generic.rs` in **Step 3b**, not 2c.

Resolution options, in order of preference:

1. **Inline it into `pool-core/src/swap.rs`** as a private helper.
   It's ~8 lines of pure math, no state access, no external deps beyond
   `cosmwasm_std`. Inlining avoids a temporary cross-crate dep and lets
   2c land as a clean self-contained commit. Step 3b can then move the
   creator-pool copy and keep (or drop, if unused elsewhere) this
   pool-core local one.

2. **Co-land 3b with 2c.** Larger diff but removes the temporary
   duplication. Acceptable if you prefer fewer commits.

3. **Duplicate the helper** in pool-core/src/swap.rs with a TODO comment
   pointing at Step 3b. Fine but leaves a debt to clean up.

**Default recommendation: option 1.** Inline it.

### Update `pool-core/src/lib.rs`

Module ordering after 2c:

```rust
pub mod error;
pub mod state;   // 2a
pub mod asset;   // 2b
pub mod swap;    // 2c
```

### `pool/src/contract.rs` impact

`contract.rs` declares `pub const DEFAULT_SLIPPAGE: &str = "0.005";` at
module top. Remove that declaration. Existing `use
crate::contract::DEFAULT_SLIPPAGE;` imports inside the creator-pool
crate break — but there's only one (`pool/src/swap_helper.rs` line 1),
which is also being rewritten in this step. If any tests reference
`crate::contract::DEFAULT_SLIPPAGE`, update them to
`crate::swap_helper::DEFAULT_SLIPPAGE` (resolves via the re-export) or
directly to `pool_core::swap::DEFAULT_SLIPPAGE`.

### Cargo.toml changes

None for 2c. All required deps (`cosmwasm-std`) already present in
pool-core from C1 skeleton.

### Expected compile-error patterns after 2c

1. **`decimal2decimal256` not found in scope** — if you chose option 3
   (duplicate) or the Step 3b timing slipped. Fix: inline per option 1.

2. **`DEFAULT_SLIPPAGE` unresolved** — removed from `pool::contract`
   but still referenced somewhere in pool code that hasn't been updated.
   Search: `grep -rn "DEFAULT_SLIPPAGE\|crate::contract::DEFAULT_SLIPPAGE" pool/src`
   and either let it resolve through `crate::swap_helper::DEFAULT_SLIPPAGE`
   (via the re-export) or import directly from `pool_core::swap`.

3. **Private `FactoryQueryWrapper` visibility** — no change needed;
   it stays `enum FactoryQueryWrapper` (module-private) in
   `pool/src/swap_helper.rs` and is never referenced outside this file.

4. **`PoolState` mutability** — `update_price_accumulator` takes
   `&mut PoolState`. Callers pass through a mutable ref loaded from
   `POOL_STATE.load`. No signature change, no caller change.

### Verification after 2c

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool
```

Compare `pool.wasm` byte size before/after (optional) — should be
~nearly identical; we've moved code between static libraries being
linked into the same final wasm.

### Suggested commit message

```
H14 split (2c/N): extract AMM math to pool-core::swap

Splits pool/src/swap_helper.rs between pool-core (pair-shape-agnostic
constant-product math: compute_swap, compute_offer_amount,
assert_max_spread, update_price_accumulator, DEFAULT_SLIPPAGE) and
creator-pool (oracle-integration helpers that query the factory's
internal bluechip/USD oracle: get_oracle_conversion_with_staleness,
usd_to_bluechip_at_rate, get_bluechip_value, etc.).

Standard pools have no commit phase and never consume the oracle
helpers, so those stay in creator-pool. Every swap/deposit/remove
code path that both pools share uses the pool-core math.

Also moves DEFAULT_SLIPPAGE out of pool/src/contract.rs — it logically
belongs with assert_max_spread, which now lives in pool-core.

pool/src/swap_helper.rs is now 2 files' worth of content: a
`pub use pool_core::swap::*;` shim + the oracle helpers. Existing
imports via `use crate::swap_helper::X;` continue to resolve.

decimal2decimal256 (currently in pool/src/generic_helpers.rs, moving
in Step 3b) is inlined privately into pool-core/src/swap.rs for this
step to keep 2c self-contained.

No behavior change.
```

## Step 2d — `msg.rs` split

Source: `pool/src/msg.rs` (366 lines). Split logic: anything returned by
a shared query, consumed by a shared handler's input, or used as a wire
type on the swap/liquidity path is shared (→ pool-core). Commit-phase
inputs, responses, and the pool's `ExecuteMsg`/`QueryMsg`/`MigrateMsg`
enums stay in creator-pool (standard-pool will define its own slimmer
enums in Step 4).

### Items that MOVE to `pool-core/src/msg.rs`

#### Input types used by shared handlers

| Item | Kind | Used by |
|---|---|---|
| `CommitFeeInfo` | struct | value type of shared `COMMITFEEINFO` storage Item; consumed by shared `emergency_withdraw` (reads `bluechip_wallet_address` for drain recipient) |
| `PoolConfigUpdate` | struct | input to shared `execute_update_config_from_factory`; factory's pool-config update path sends it |
| `Cw20HookMsg` | enum | deserialized by shared `execute_swap_cw20` when CW20 `Receive` hooks fire; today contains a single `Swap {...}` variant |

#### Shared query response types

Every shared query in pool-core needs its response struct on the same side.

| Item | Kind | Returned by |
|---|---|---|
| `SimulationResponse` | struct | `query_simulation` |
| `ReverseSimulationResponse` | struct | `query_reverse_simulation` |
| `CumulativePricesResponse` | struct | `query_cumulative_prices` |
| `FeeInfoResponse` | struct | `query_fee_info` (contains `CommitFeeInfo`, hence above) |
| `ConfigResponse` | struct | `query_config` |
| `PoolStateResponse` | struct | `query_pool_state` |
| `PoolFeeStateResponse` | struct | `query_fee_state` |
| `PositionResponse` | struct | `query_position` |
| `PositionsResponse` | struct | `query_positions`, `query_positions_by_owner` (vec of `PositionResponse`) |
| `PoolInfoResponse` | struct | `query_pool_info` |
| `PoolAnalyticsResponse` | struct | `query_analytics` (contains `threshold_status: CommitStatus`) |
| `CommitStatus` | enum | returned inside `PoolAnalyticsResponse`; also returned by commit-only `query_check_threshold_limit`. Shared type; both sides use it. |

#### Misc shared

| Item | Kind | Notes |
|---|---|---|
| `PoolResponse` | struct | `{ assets: [TokenInfo; 2] }`; if any shared query returns it (grep to verify at execute time), shared. If unused, delete as dead code. |

### Items that STAY in `pool/src/msg.rs` (creator-pool only)

| Item | Kind | Reason |
|---|---|---|
| `ExecuteMsg` | enum | contains Commit / ContinueDistribution / ClaimCreatorExcessLiquidity / ClaimCreatorFees / RetryFactoryNotify / RecoverStuckStates variants. Standard-pool defines its own slimmer `ExecuteMsg` in Step 4. |
| `QueryMsg` | enum | contains `IsFullyCommited`, `CommittingInfo`, `PoolCommits`, `LastCommited`, `FactoryNotifyStatus` — all commit-phase queries. Standard-pool defines its own slimmer `QueryMsg` in Step 4. |
| `MigrateMsg` | enum | creator-pool's own `UpdateFees` / `UpdateVersion` migration variants. Standard-pool will have its own `MigrateMsg` (likely identical shape — fine to duplicate since migration logic is per-contract anyway). |
| `Cw20ReceiveMsg` (via `use cw20::Cw20ReceiveMsg`) | external | pass-through import; not owned by pool/src/msg.rs |
| `PoolInstantiateMsg` | enum | currently `Commit(CommitPoolInstantiateMsg) / Standard(...)`. **Step 4 flattens** back to the struct shape. Leave untouched in 2d. |
| `CommitPoolInstantiateMsg` | struct | the wrapped-in-Commit-variant payload. Step 4 renames back to `PoolInstantiateMsg`. Leave untouched in 2d. |
| `FactoryNotifyStatusResponse` | struct | returned by `query_factory_notify_status` (commit-only). |
| `PoolCommitResponse` | struct | returned by `query_pool_committers` (commit-only, queries COMMIT_INFO ledger). |
| `CommitterInfo` | struct | entry in `PoolCommitResponse`. Commit-only. |
| `LastCommittedResponse` | struct | returned by `query_last_committed` (commit-only, queries COMMIT_INFO). |

### `pool/src/msg.rs` after the split

Top of file:

```rust
// Shared wire-format types live in pool-core::msg. Keep the glob
// re-export so existing `use crate::msg::X;` imports resolve unchanged.
pub use pool_core::msg::*;

// ... below: creator-pool-specific ExecuteMsg, QueryMsg, MigrateMsg,
//          PoolInstantiateMsg enum + CommitPoolInstantiateMsg struct,
//          FactoryNotifyStatusResponse, PoolCommitResponse,
//          CommitterInfo, LastCommittedResponse
```

Note the `#[allow(unused_imports)]` lines at the top of the current
file can stay verbatim — they silence false positives when a subset of
the `use crate::asset` / `use crate::state` imports ends up unused
after the split.

### `packages/pool-core/src/msg.rs` imports

```rust
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use cw20::Cw20ReceiveMsg;
use crate::asset::{TokenInfo, TokenType};  // shared asset types from 2b
```

`QueryResponses` derive is kept on response structs for schema
generation; see note below if you generate schemas per contract.

### Update `pool-core/src/lib.rs`

```rust
pub mod error;
pub mod state;  // 2a
pub mod asset;  // 2b
pub mod swap;   // 2c
pub mod msg;    // 2d
```

### Wire-format invariant

Every struct moving to pool-core keeps its `#[cw_serde]` attribute
unchanged, which means the JSON shape (field names, nested struct
layout) is identical to today. Existing clients, deploy scripts, and
frontend integrations continue to deserialize responses without any
change. The Rust type lives in a different crate; the bytes on the
wire do not change.

### Cargo.toml changes

None. pool-core already has `cosmwasm-schema`, `cosmwasm-std`, `cw20`.

### Schema generation note

If the creator-pool crate has a `src/bin/schema.rs` that calls
`cosmwasm_schema::write_api!` with the types here, some of those types
now live in pool-core. Update the schema binary to import from the new
module paths. This is a creator-pool-only concern and doesn't block
compilation of pool-core itself.

### Expected compile-error patterns after 2d

1. **`CommitFeeInfo` duplicate definition** — if 2a has already landed
   and left a `use crate::msg::CommitFeeInfo;` in `pool/src/state.rs`,
   the glob re-export from `pool_core::msg::*` plus the move in 2d
   means `crate::msg::CommitFeeInfo` now resolves to
   `pool_core::msg::CommitFeeInfo`. Clean resolution, no duplication.

2. **`CommitStatus` enum** — referenced by `query_check_threshold_limit`
   (commit-only, stays in creator-pool). After 2d that query imports
   `CommitStatus` from `pool_core::msg` via the re-export. No change
   needed at the call site.

3. **Creator-pool's `QueryMsg::Analytics` → `PoolAnalyticsResponse`** —
   `PoolAnalyticsResponse` now lives in pool-core. The `#[returns(...)]`
   attribute on the variant must reference the right path. `#[returns(PoolAnalyticsResponse)]`
   resolves via the glob re-export at top of `pool/src/msg.rs`. Works.

4. **Response-struct public fields** — structs moving to pool-core must
   keep ALL their fields `pub` (they're `#[cw_serde]` already; cw_serde
   does not change visibility). Sanity: visibility stays as-written.

5. **`pub enum CommitStatus::InProgress { raised, target }`** — variant
   fields are `pub` by virtue of being inside a `#[cw_serde]` enum. No
   change.

### Verification after 2d

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool
```

All three must pass. Test behavior unchanged — tests that construct
`SimulationResponse { ... }` now construct a pool-core type but Rust
resolves the path through the re-export, so no test file edits are
required.

### Step 2 is complete after 2d

pool-core now contains: error, state (shared subset), asset, swap (math),
msg (shared wire types). Step 3 takes care of the operational layer
(liquidity, helpers, admin, query).

### Suggested commit message

```
H14 split (2d/N): extract shared wire types to pool-core::msg

Moves the subset of pool/src/msg.rs that is consumed by shared code
paths into packages/pool-core/src/msg.rs:

  Shared inputs:          CommitFeeInfo, PoolConfigUpdate, Cw20HookMsg
  Shared query responses: Simulation*, ReverseSimulation*, CumulativePrices*,
                          FeeInfo*, Config*, PoolState*, PoolFeeState*,
                          Position*, Positions*, PoolInfo*, PoolAnalytics*,
                          PoolResponse, CommitStatus

Commit-phase-only types stay in pool/src/msg.rs behind a
`pub use pool_core::msg::*;` glob re-export: ExecuteMsg, QueryMsg,
MigrateMsg, PoolInstantiateMsg (enum), CommitPoolInstantiateMsg,
FactoryNotifyStatusResponse, PoolCommitResponse, CommitterInfo,
LastCommittedResponse.

Wire format unchanged — every moved struct keeps #[cw_serde] with
identical field names, so deployed clients / frontends / deploy
scripts continue to round-trip responses unchanged.

Step 2 (pool-core foundational modules) is complete after this commit.
Step 3 (operational modules: liquidity, helpers, admin, query) comes
next.

No behavior change.
```

## Step 3a — `liquidity.rs` + `liquidity_helpers.rs`

Source files:
- `pool/src/liquidity.rs` (975 lines) — LP-operation handler bodies
- `pool/src/liquidity_helpers.rs` (554 lines) — math + validators +
  two commit-only claim handlers

After Commit 4b (pair-shape generalization), every LP op body and
every math helper in these two files is pair-shape agnostic and is
called by both pool kinds. The only commit-only code here is the two
`execute_claim_creator_*` handlers at the bottom of `liquidity_helpers.rs`.

### `liquidity.rs` — wholesale move to `pool-core/src/liquidity.rs`

All 9 public functions plus their internal helpers move verbatim:

| Item | Kind | Notes |
|---|---|---|
| `DepositPrep` struct | private | per-deposit state bundle incl. `refund_amount0/1` per-side refunds (from 4b) |
| `collect_deposit_side` | private fn | per-asset `TokenType` dispatch — Native verifies funds + emits refund, CW20 emits TransferFrom |
| `prepare_deposit` | private fn | wraps `calc_liquidity_for_deposit` + calls `collect_deposit_side` for each side |
| `execute_deposit_liquidity` | pub fn | first-deposit and general-deposit path; mints position NFT |
| `execute_collect_fees` | pub fn | LP fee collection; sweeps `CREATOR_FEE_POT` clip |
| `add_to_position` | pub fn | add to existing position + auto-collect pending |
| `execute_add_to_position` | pub fn | rate-limited entry wrapper |
| `remove_all_liquidity` | pub fn | full withdraw + fees |
| `execute_remove_all_liquidity` | pub fn | rate-limited entry wrapper |
| `remove_partial_liquidity` | pub fn | partial withdraw + fees; preserves `unclaimed_fees_*` |
| `execute_remove_partial_liquidity` | pub fn | rate-limited entry wrapper |
| `execute_remove_partial_liquidity_by_percent` | pub fn | thin wrapper over partial |

`pool/src/liquidity.rs` after the move:

```rust
//! LP-operation handlers live in `pool_core::liquidity`. This re-export
//! preserves every `use crate::liquidity::X;` import in the creator-pool
//! crate and its tests.
pub use pool_core::liquidity::*;
```

### `liquidity_helpers.rs` — split

#### MOVE to `pool-core/src/liquidity_helpers.rs`

Math, validators, position-integrity helpers — called by shared LP ops
and by commit-only `execute_claim_creator_*` alike.

| Item | Kind |
|---|---|
| `OPTIMAL_LIQUIDITY` | `pub const Uint128 = 1_000_000` |
| `calculate_unclaimed_fees` | fn |
| `calculate_fees_owed` | fn |
| `calculate_fees_owed_split` | fn (returns `(adjusted, clipped)`) |
| `calc_capped_fees` | fn |
| `calc_capped_fees_with_clip` | fn (primary fee-payout computer) |
| `build_fee_transfer_msgs` | fn (shape-agnostic via `build_transfer_msg`) |
| `build_transfer_msg` | fn (per-asset dispatch — added in 4b) |
| `check_slippage` | fn |
| `check_ratio_deviation` | fn |
| `calculate_fee_size_multiplier` | fn |
| `integer_sqrt` | fn |
| `calc_liquidity_for_deposit` | fn (first-deposit ratio math) |
| `verify_position_ownership` | fn (CW721 ownership query) |
| `sync_position_on_transfer` | fn (fee-checkpoint reset on NFT transfer — audit H8 surface) |

#### STAY in `pool/src/liquidity_helpers.rs`

Commit-phase-specific wallet claim handlers. Both depend on commit-only
storage Items (`CREATOR_FEE_POT` is shared but only ever written by the
commit-pool fee clipping flow; `CREATOR_EXCESS_POSITION` is commit-only).

| Item | Kind | Why it stays |
|---|---|---|
| `execute_claim_creator_fees` | pub fn | sweeps `CREATOR_FEE_POT` to `COMMITFEEINFO.creator_wallet_address`; creator concept is commit-only |
| `execute_claim_creator_excess` | pub fn | sweeps `CREATOR_EXCESS_POSITION`; this Item only exists in commit-pool state |

`pool/src/liquidity_helpers.rs` after the split:

```rust
//! Commit-phase-only claim handlers. The shared math + validators
//! previously in this file now live in `pool_core::liquidity_helpers`
//! and are re-exported below.
pub use pool_core::liquidity_helpers::*;

use crate::error::ContractError;
use crate::state::{CreatorFeePot, COMMITFEEINFO, CREATOR_EXCESS_POSITION, CREATOR_FEE_POT, POOL_INFO};
use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, MessageInfo, Response, WasmMsg,
};

pub fn execute_claim_creator_fees(...) { /* unchanged body */ }
pub fn execute_claim_creator_excess(...) { /* unchanged body */ }
```

### Cross-step dependencies (important)

`pool-core/src/liquidity.rs` imports two helpers that today live in
`pool/src/generic_helpers.rs`:

- `check_rate_limit` — called by `execute_add_to_position`, `execute_remove_*`
- `enforce_transaction_deadline` — called by every public LP entry point

Both of these move to `pool-core/src/generic.rs` in **Step 3b**, not 3a.

**Resolution options**, in order of preference:

1. **Co-land 3a and 3b as a single commit.** The two steps are
   naturally coupled — neither makes sense deployed alone. This is the
   recommended path. Commit subject: `H14 split (3a+3b/N): liquidity +
   helpers + generic utilities`.

2. **Pre-move `check_rate_limit` and `enforce_transaction_deadline`
   into a new `pool-core/src/generic.rs` as part of 3a**, leaving the
   rest of `generic_helpers.rs` for 3b. This keeps 3a self-contained
   but nibbles 3b's scope.

3. **Temporarily declare them private in pool-core/src/liquidity.rs**
   as copies; delete in 3b. Works but leaves duplication until 3b lands.

**Default recommendation: option 1** — land 3a+3b together.

### Also depends on (already landed or being landed)

- `pool-core::state::{PoolInfo, PoolState, PoolFeeState, PoolSpecs, Position, CreatorFeePot, POOL_INFO, POOL_STATE, POOL_FEE_STATE, POOL_SPECS, POOL_ANALYTICS, POOL_PAUSED, LIQUIDITY_POSITIONS, NEXT_POSITION_ID, OWNER_POSITIONS, CREATOR_FEE_POT, MINIMUM_LIQUIDITY}` — all moved in 2a.
- `pool-core::asset::TokenType`, `pool-core::asset::TokenInfoPoolExt` — moved in 2b.
- `pool-core::swap::update_price_accumulator` — moved in 2c.
- `pool-core::error::ContractError` — moved in the C1-series error.rs commit.

### `packages/pool-core/src/liquidity.rs` imports

```rust
use crate::asset::TokenType;
use crate::error::ContractError;
use crate::liquidity_helpers::{
    build_fee_transfer_msgs, calc_capped_fees_with_clip, calc_liquidity_for_deposit,
    calculate_fee_size_multiplier, calculate_fees_owed_split, check_ratio_deviation,
    check_slippage, sync_position_on_transfer, verify_position_ownership,
};
use crate::state::{
    CreatorFeePot, PoolInfo, PoolSpecs, Position, TokenMetadata,
    CREATOR_FEE_POT, LIQUIDITY_POSITIONS, MINIMUM_LIQUIDITY, NEXT_POSITION_ID,
    OWNER_POSITIONS, POOL_ANALYTICS, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED,
    POOL_SPECS, POOL_STATE,
};
use crate::swap::update_price_accumulator;
use crate::generic::{check_rate_limit, enforce_transaction_deadline};  // 3b co-landed
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo,
    Response, StdError, Timestamp, Uint128, WasmMsg,
};
use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};
```

### `packages/pool-core/src/liquidity_helpers.rs` imports

```rust
use crate::asset::{get_native_denom, TokenType};
use crate::error::ContractError;
use crate::state::{
    PoolFeeState, PoolInfo, Position, LIQUIDITY_POSITIONS, OWNER_POSITIONS,
};
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, Deps, StdError, StdResult, Storage, Uint128, WasmMsg,
};
```

(Note: `get_native_denom` comes from the re-exported `pool_factory_interfaces::asset`, which pool-core re-exports via `pub use pool_factory_interfaces::asset::*` inside `pool-core/src/asset.rs`. So `crate::asset::get_native_denom` resolves correctly.)

### Update `pool-core/src/lib.rs`

```rust
pub mod error;
pub mod state;
pub mod asset;
pub mod swap;
pub mod msg;
pub mod liquidity_helpers;  // 3a
pub mod liquidity;          // 3a  — depends on liquidity_helpers, generic
pub mod generic;            // 3b (co-landed)
```

`liquidity` references `generic`, so put `generic;` above `liquidity;`
if your module declaration order is visually meaningful. Rust does not
care about declaration order for resolution.

### Cargo.toml changes

None for 3a. pool-core already depends on `pool-factory-interfaces`,
`cosmwasm-std`, `cw-storage-plus`, and (via wildcard re-exports) `cw20`.

### Expected compile-error patterns after 3a

(Assuming 3a+3b co-landed per option 1.)

1. **`DepositPrep` not pub** — it's declared `struct DepositPrep` (module
   private). That's fine — it's only used inside `liquidity.rs` itself,
   which moves wholesale. No cross-crate access needed.

2. **Path change for `prep.collect_msgs`** — `prep` is a local of type
   `DepositPrep` built inside pool-core; consumers (`execute_deposit_liquidity`
   and `add_to_position`) are in the same file, so field access stays
   identical.

3. **`CreatorFeePot` import path** — struct is in `pool-core::state`
   after 2a; import resolves.

4. **CW721 message construction** — `Cw721ExecuteMsg::<TokenMetadata>::Mint {...}`
   and `Cw721ExecuteMsg::<()>::UpdateOwnership(...)` both used;
   `TokenMetadata` lives in `pool-core::state` (moved 2a). Typing stays
   clean.

5. **Tests** — `pool/src/testing/liquidity_tests.rs` imports
   `crate::liquidity::X`. Glob re-export handles it.

### Verification after 3a (or 3a+3b combined)

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool
```

Focus: the pool's existing liquidity tests. If they pass unchanged,
the shape-agnostic code path works as before.

### Suggested commit message (if co-landed 3a+3b)

```
H14 split (3a+3b/N): extract liquidity + liquidity_helpers + generic to pool-core

Combined extraction — the two steps share dependencies tightly enough
(pool-core::liquidity imports check_rate_limit and
enforce_transaction_deadline from generic) that splitting them into
separate commits would leave the tree non-building in between.

Moved to pool-core:
  - liquidity.rs wholesale (9 LP ops + DepositPrep + collect_deposit_side)
  - liquidity_helpers.rs minus execute_claim_creator_{fees,excess}
    (fee math, slippage/ratio validators, integer_sqrt,
     fee_size_multiplier, verify_position_ownership,
     sync_position_on_transfer, OPTIMAL_LIQUIDITY)
  - generic_helpers.rs shared subset: check_rate_limit,
    enforce_transaction_deadline, update_pool_fee_growth,
    decimal2decimal256, get_bank_transfer_to_msg, mint_tokens

Stayed in pool/:
  - liquidity_helpers.rs: execute_claim_creator_fees,
    execute_claim_creator_excess (both depend on commit-only Items)
  - generic_helpers.rs: trigger_threshold_payout,
    process_distribution_batch, validate_pool_threshold_payments,
    update_commit_info, calculate_effective_batch_size,
    calculate_committer_reward, ThresholdPayoutMsgs struct,
    DISTRIBUTION_STATE machinery

pool-core::swap also drops its private inline decimal2decimal256
(from 2c) and imports it from pool-core::generic now.

Three glob re-export shims preserve every existing call site:
  - pool/src/liquidity.rs
  - pool/src/liquidity_helpers.rs
  - pool/src/generic_helpers.rs

No behavior change. All existing tests pass unchanged.
```

(If you split 3a and 3b separately per options 2 or 3, adjust
accordingly — the plan is unchanged; only the commit boundary differs.)

## Step 3b — `generic_helpers.rs` split

Source: `pool/src/generic_helpers.rs` (562 lines). The file is a mix of
primitive utilities (rate limit, deadline check, decimal conversions,
bank/mint message builders) and commit-phase-specific heavy lifting
(threshold payout, distribution batching, commit-ledger bookkeeping).

### Items that MOVE to `pool-core/src/generic.rs`

Shared primitives used by swap, liquidity, and (in the case of
`update_pool_fee_growth` and `check_rate_limit`) by commit too.

| Item | Kind | Notes |
|---|---|---|
| `update_pool_fee_growth` | fn | updates `PoolFeeState.fee_growth_global_*` + `fee_reserve_*`; called by every swap and post-threshold commit |
| `check_rate_limit` | fn | reads/writes shared `USER_LAST_COMMIT` Item; called by every rate-limited entry point |
| `enforce_transaction_deadline` | fn | pure check; called by every user-facing entry |
| `decimal2decimal256` | fn | pure — used by swap math. Deletes the temporary private copy `pool-core/src/swap.rs` grew in Step 2c. |
| `get_bank_transfer_to_msg` | fn | thin `BankMsg::Send` builder; used by LP operations and fee payouts |

### Items that STAY in `pool/src/generic_helpers.rs` (creator-pool only)

The entire threshold-payout / distribution machinery. These functions
depend on `ThresholdPayoutAmounts`, `CommitLimitInfo`, `DistributionState`,
`CREATOR_EXCESS_POSITION`, `COMMIT_LEDGER`, `COMMIT_INFO`, `NATIVE_RAISED_FROM_COMMIT` —
every one of which is creator-pool-only state per Step 2a.

| Item | Kind | Notes |
|---|---|---|
| `validate_pool_threshold_payments` | fn | called from creator-pool `instantiate` on the commit-pool path |
| `ThresholdPayoutMsgs` | struct | return type of `trigger_threshold_payout`; bundles `factory_notify` SubMsg + `other_msgs` |
| `trigger_threshold_payout` | fn | ~150 lines; mints creator+bluechip rewards, seeds pool reserves, sets up distribution |
| `process_distribution_batch` | fn | ~130 lines; drains `COMMIT_LEDGER` in MAX_DISTRIBUTIONS_PER_TX-sized batches |
| `calculate_effective_batch_size` | fn | helper to `process_distribution_batch` |
| `calculate_committer_reward` | private fn | helper to `process_distribution_batch` |
| `update_commit_info` | fn | updates `COMMIT_INFO` ledger entry for a committer |
| `mint_tokens` | fn | builds a `Cw20ExecuteMsg::Mint`; only called from `trigger_threshold_payout` and `process_distribution_batch` |

### `pool/src/generic_helpers.rs` after the split

```rust
//! Commit-phase helpers that compose the shared primitives in
//! pool_core::generic with commit-only state (ThresholdPayoutAmounts,
//! CommitLimitInfo, DistributionState, COMMIT_LEDGER, etc.). Existing
//! `use crate::generic_helpers::X;` imports resolve via the re-export
//! below for the shared items, or fall through to the local defs for
//! commit-only items.
pub use pool_core::generic::*;

use crate::error::ContractError;
use crate::state::{
    CommitLimitInfo, DistributionState, PoolFeeState, PoolInfo, PoolState,
    ThresholdPayoutAmounts, Committing, CREATOR_EXCESS_POSITION, COMMIT_INFO,
    COMMIT_LEDGER, DISTRIBUTION_STATE, DISTRIBUTION_STALL_TIMEOUT_SECONDS,
    DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION, DEFAULT_MAX_GAS_PER_TX,
    MAX_DISTRIBUTIONS_PER_TX, NATIVE_RAISED_FROM_COMMIT, POOL_FEE_STATE,
    POOL_STATE,
};
use crate::msg::CommitFeeInfo;  // re-exported from pool_core::msg after 2d
use crate::state::{CreatorExcessLiquidity};  // stays creator-only per 2a
use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, Order, StdError,
    StdResult, Storage, SubMsg, Timestamp, Uint128, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_storage_plus::Bound;

pub struct ThresholdPayoutMsgs { /* unchanged */ }
pub fn validate_pool_threshold_payments(...) { /* unchanged */ }
pub fn trigger_threshold_payout(...) { /* unchanged */ }
pub fn process_distribution_batch(...) { /* unchanged */ }
pub fn calculate_effective_batch_size(...) { /* unchanged */ }
fn calculate_committer_reward(...) { /* unchanged, private */ }
pub fn update_commit_info(...) { /* unchanged */ }
pub fn mint_tokens(...) { /* unchanged */ }
```

### `packages/pool-core/src/generic.rs` imports

```rust
use crate::error::ContractError;
use crate::state::{PoolFeeState, PoolSpecs, PoolState, USER_LAST_COMMIT};
use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, Decimal, Decimal256, DepsMut, Env, StdError,
    StdResult, Timestamp, Uint128,
};
```

### Update `pool-core/src/swap.rs`

Delete the private inline `decimal2decimal256` added in Step 2c and
replace with:

```rust
use crate::generic::decimal2decimal256;
```

Net effect of 3b on pool-core/src/swap.rs: -8 lines (inline fn body),
+1 line (use import).

### Update `pool-core/src/lib.rs`

```rust
pub mod error;
pub mod state;
pub mod asset;
pub mod swap;
pub mod msg;
pub mod generic;            // 3b (co-landed with 3a per recommendation)
pub mod liquidity_helpers;  // 3a
pub mod liquidity;          // 3a — depends on generic
```

### Cargo.toml changes

None. pool-core already has `cosmwasm-std`, `cw-storage-plus`.

Creator-pool's generic_helpers.rs uses `cw20`, `cw_storage_plus::Bound`,
etc. — those deps are already in `pool/Cargo.toml`.

### Expected compile-error patterns after 3b

1. **`decimal2decimal256` duplicate definition** — if Step 2c's private
   inline is not deleted when 3b lands, you get a name-shadowing warning
   inside `pool-core/src/swap.rs`. Fix: delete the inline, import from
   `crate::generic`.

2. **`USER_LAST_COMMIT` visibility** — Item is `pub` in `pool-core::state`
   after 2a. `pool-core::generic::check_rate_limit` imports it as
   `use crate::state::USER_LAST_COMMIT;`. Clean.

3. **`trigger_threshold_payout` / `process_distribution_batch` call
   sites** — `pool/src/commit.rs` imports these as
   `use crate::generic_helpers::{trigger_threshold_payout,
   process_distribution_batch};`. They stay in creator-pool's
   `generic_helpers.rs` so the import path is unchanged. ✓

4. **`mint_tokens` call sites** — only `trigger_threshold_payout` and
   `process_distribution_batch` call `mint_tokens`, both in creator-pool
   after the split. Local path (`crate::generic_helpers::mint_tokens`)
   continues to resolve. ✓

5. **`enforce_transaction_deadline` call sites outside
   liquidity/commit** — `pool/src/liquidity_helpers.rs` (now the
   claim-handler file) calls `crate::generic_helpers::enforce_transaction_deadline`
   inside `execute_claim_creator_fees` and `execute_claim_creator_excess`.
   Resolves through the glob re-export in `pool/src/generic_helpers.rs`. ✓

6. **`update_pool_fee_growth` called from `pool-core::liquidity`** —
   post-3b, `pool-core::liquidity::execute_collect_fees` (and the
   remove-liquidity paths) call `update_pool_fee_growth` via
   `crate::generic::update_pool_fee_growth`. No cross-crate call
   needed. ✓

### Verification after 3b (co-landed with 3a)

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool
```

If split from 3a into a separate commit (not recommended), verify that
the intermediate tree built between 3a alone and 3a+3b — it will NOT,
so don't attempt.

### Step 3 progress tracker

After 3a+3b (co-landed):

- [x] 3a  liquidity + liquidity_helpers
- [x] 3b  generic_helpers
- [ ] 3c  admin (pause / emergency_withdraw / update_config_from_factory shared; execute_recover_stuck_states stays)
- [ ] 3d  query (~15 shared queries; 4 commit-only stay)

Step 3 is ~50% complete once 3a+3b lands. After 3c and 3d, every shared
handler in the creator-pool crate is either in `pool-core` or is a
commit-phase handler.

## Step 3c — `admin.rs` split

Source: `pool/src/admin.rs` (462 lines). The administrative handlers
(pause/unpause/emergency-withdraw/config-update) are mostly shared, but
two details make this the trickiest sub-step of Step 3:

1. `execute_emergency_withdraw` sweeps `CREATOR_FEE_POT` and
   `CREATOR_EXCESS_POSITION` — the former is in pool-core per Step 2a,
   the latter was marked creator-only in Step 2a but is now referenced
   from a handler we want in pool-core.
2. `execute_emergency_withdraw` halts `DISTRIBUTION_STATE` on drain,
   and `DISTRIBUTION_STATE` is commit-only.

The resolution is NOT "pull DISTRIBUTION_STATE/CREATOR_EXCESS_POSITION
into pool-core" (that would leak commit-phase concepts back into the
shared library). Instead we factor the handler so pool-core does the
core drain and creator-pool layers its commit-specific bookkeeping
around it.

### Items that MOVE to `pool-core/src/admin.rs`

Shared admin handlers, pure of commit-phase state references.

| Item | Kind | Notes |
|---|---|---|
| `ensure_not_drained` | fn | reads shared `EMERGENCY_DRAINED` Item; called by every LP/swap path |
| `execute_pause` | fn | auth: `POOL_INFO.factory_addr`; writes `POOL_PAUSED` |
| `execute_unpause` | fn | symmetrical |
| `execute_cancel_emergency_withdraw` | fn | removes `PENDING_EMERGENCY_WITHDRAW`, unpauses |
| `execute_update_config_from_factory` | fn | updates `POOL_SPECS` (lp_fee, min_commit_interval, usd_payment_tolerance_bps) and `ORACLE_INFO.oracle_addr`; all state involved is shared |
| `execute_emergency_withdraw_initiate` | fn (factored, see below) | Phase 1: pauses pool + writes `PENDING_EMERGENCY_WITHDRAW` timestamp |
| `execute_emergency_withdraw_core_drain` | fn (factored, see below) | Phase 2: sweeps `POOL_STATE` reserves + `POOL_FEE_STATE.fee_reserve_*` + `CREATOR_FEE_POT`, writes `EMERGENCY_WITHDRAWAL` audit, flips `EMERGENCY_DRAINED`. Returns `(Response, drain_total_0, drain_total_1)` so the caller can layer extras on top. |

### The emergency-withdraw factoring

Today `execute_emergency_withdraw` is a single ~150-line function that
branches on whether `PENDING_EMERGENCY_WITHDRAW` is populated (phase 1
vs phase 2). The split rewrites it as follows:

`pool-core/src/admin.rs`:

```rust
pub fn execute_emergency_withdraw_initiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // 1. Auth (info.sender == POOL_INFO.factory_addr)
    // 2. ensure_not_drained
    // 3. Require PENDING_EMERGENCY_WITHDRAW is NOT already set
    // 4. POOL_PAUSED := true
    // 5. PENDING_EMERGENCY_WITHDRAW := env.block.time + 24h
    // 6. Return "emergency_withdraw_initiated" response
}

pub fn execute_emergency_withdraw_core_drain(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<CoreDrainResult, ContractError> {
    // 1. Auth
    // 2. Require PENDING_EMERGENCY_WITHDRAW set AND timelock elapsed
    // 3. Load pool_info, pool_state, pool_fee_state
    // 4. Accumulate drain totals: reserves + fee_reserves + CREATOR_FEE_POT
    // 5. Build transfer messages via shared build_transfer_msg
    //    (shape-agnostic per Commit 4b)
    // 6. Zero reserves, clear fee_reserves, remove CREATOR_FEE_POT
    // 7. Write EMERGENCY_WITHDRAWAL audit record
    // 8. EMERGENCY_DRAINED := true
    // 9. Return { drain_messages, total_0, total_1, recipient }
}

pub struct CoreDrainResult {
    pub messages: Vec<CosmosMsg>,
    pub total_0: Uint128,
    pub total_1: Uint128,
    pub recipient: Addr,
    pub total_liquidity_at_withdrawal: Uint128,
}
```

### Items that STAY in `pool/src/admin.rs` (creator-pool only)

| Item | Kind | Why it stays |
|---|---|---|
| `execute_recover_stuck_states` | fn | dispatches on creator-only `RecoveryType` enum |
| `recover_threshold` | private fn | clears `THRESHOLD_PROCESSING` (commit-only Item) |
| `recover_distribution` | private fn | restarts `DISTRIBUTION_STATE` (commit-only) |
| `recover_reentrancy_guard` | private fn | could be shared; kept with siblings for cohesion. Standard-pool doesn't need stuck-state recovery — the failure modes (stalled distribution, stuck threshold processing) only occur in commit flows. |
| `execute_emergency_withdraw` | fn (creator-pool wrapper) | Wraps the two pool-core phase functions and adds commit-only concerns: pre-threshold rejection (IS_THRESHOLD_HIT gate), CREATOR_EXCESS_POSITION sweep, DISTRIBUTION_STATE halt. |

Creator-pool wrapper (`pool/src/admin.rs` after split):

```rust
pub use pool_core::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw,
    execute_pause, execute_unpause, execute_update_config_from_factory,
};

use pool_core::admin::{
    execute_emergency_withdraw_core_drain, execute_emergency_withdraw_initiate,
    CoreDrainResult,
};

/// Creator-pool emergency withdraw: adds pre-threshold rejection,
/// CREATOR_EXCESS_POSITION sweep, and DISTRIBUTION_STATE halt on top
/// of the shared core drain.
pub fn execute_emergency_withdraw(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    // Commit-only gate: reject pre-threshold (committed funds are
    // untracked in reserves; see audit finding context).
    if !IS_THRESHOLD_HIT.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::Std(StdError::generic_err(
            "EmergencyWithdraw is disabled before the commit threshold has been crossed...",
        )));
    }

    // Phase 1 vs Phase 2 dispatch.
    let is_phase_2 = PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_some();
    if !is_phase_2 {
        return execute_emergency_withdraw_initiate(deps, env, info);
    }

    // Phase 2: commit-only prelude — CREATOR_EXCESS_POSITION sweep
    // into the drain, plus halt any in-flight distribution.
    let excess = CREATOR_EXCESS_POSITION.may_load(deps.storage)?;
    if let Some(ref e) = excess {
        // Pre-credit excess balances; core drain doesn't know about them.
        // (Exact mechanism: either stage into POOL_STATE.reserves before
        // the core drain, or pass amounts into a variant of the core
        // drain that accepts pre-credits. Pick whichever is cleaner
        // when you write the code.)
    }

    // Halt distribution (no-op if not distributing).
    if let Ok(mut dist) = DISTRIBUTION_STATE.load(deps.storage) {
        dist.is_distributing = false;
        dist.distributions_remaining = 0;
        DISTRIBUTION_STATE.save(deps.storage, &dist)?;
    }

    let CoreDrainResult { messages, total_0, total_1, recipient, total_liquidity_at_withdrawal } =
        execute_emergency_withdraw_core_drain(deps, env.clone(), info)?;

    // If excess was swept, remove the Item and roll the amounts into
    // the response attributes (not the message set — core drain
    // already built those).
    let (final_0, final_1) = if let Some(e) = excess {
        CREATOR_EXCESS_POSITION.remove(deps.storage);
        (total_0 + e.bluechip_amount, total_1 + e.token_amount)
        // NOTE: the `messages` returned by core drain do NOT include
        // the excess balances — to actually drain them too the core
        // function needs a hook, or creator-pool builds its own extra
        // BankMsg+CW20Transfer for the excess amounts and extends the
        // message list. See "design choice" note below.
    } else {
        (total_0, total_1)
    };

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "emergency_withdraw")
        .add_attribute("recipient", recipient)
        .add_attribute("amount0", final_0)
        .add_attribute("amount1", final_1)
        .add_attribute("total_liquidity", total_liquidity_at_withdrawal)
        // ... remaining attributes unchanged ...
    )
}
```

### Design choice to make during 3c implementation

How to route `CREATOR_EXCESS_POSITION` balances into the core drain's
message list:

**Option A — core drain accepts pre-credit amounts.** Add
`extra_to_drain_0`/`extra_to_drain_1` parameters to
`execute_emergency_withdraw_core_drain`. Creator-pool passes excess
amounts; standard-pool passes zeroes. Core drain builds messages for
the combined totals. Simple, but pool-core's function signature grows.

**Option B — creator-pool builds its own excess-drain messages and
concats.** Core drain returns messages for reserves+fees+pot only.
Creator-pool builds extra BankMsg+CW20Transfer for the excess amounts
itself and extends the message list. Cleaner pool-core API, slightly
duplicated message-building logic on the creator side.

**Recommendation: Option A.** Keep the message construction in one
place (pool-core's `build_transfer_msg`-using core drain), and let
creator-pool just thread the numbers through.

### Update `pool-core/src/lib.rs`

```rust
pub mod error;
pub mod state;
pub mod asset;
pub mod swap;
pub mod msg;
pub mod generic;
pub mod liquidity_helpers;
pub mod liquidity;
pub mod admin;    // 3c
```

### Cargo.toml changes

None.

### Expected compile-error patterns after 3c

1. **`CREATOR_EXCESS_POSITION` not in pool-core** — confirmed per 2a
   classification. Creator-pool's wrapper accesses it directly; pool-core
   never touches it. Clean.

2. **`IS_THRESHOLD_HIT` load in creator-pool wrapper** — resolves
   through the `pub use pool_core::state::*;` re-export in
   `pool/src/state.rs`. `IS_THRESHOLD_HIT` is in pool-core per Step 2a.

3. **`DISTRIBUTION_STATE` load in creator-pool wrapper** — stays in
   creator-pool per Step 2a. Direct `crate::state::DISTRIBUTION_STATE`
   reference. Clean.

4. **Factory forwards** — `factory/src/execute.rs` calls the pool's
   emergency-withdraw by dispatching a `PoolAdminMsg::EmergencyWithdraw {}`
   through a `WasmMsg::Execute`. The pool-side routing (ExecuteMsg
   variant) stays in creator-pool / standard-pool. The factory doesn't
   care which variant of the handler runs. No factory change in 3c.

5. **Standard-pool's `execute_emergency_withdraw`** — not in scope for
   3c (standard-pool doesn't exist yet). Written in Step 4b as:
   ```rust
   pub fn execute_emergency_withdraw(deps, env, info) -> Result<Response, ContractError> {
       if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_some() {
           let CoreDrainResult { messages, total_0, total_1, recipient, .. } =
               execute_emergency_withdraw_core_drain(deps, env, info)?;
           Ok(Response::new().add_messages(messages).add_attribute("action", "emergency_withdraw"))
       } else {
           execute_emergency_withdraw_initiate(deps, env, info)
       }
   }
   ```

### Verification after 3c

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool   # admin_tests should pass
```

Existing `pool/src/testing/admin_tests.rs` tests the creator-pool
entry point behavior (factory-only auth, pre-threshold rejection,
timelock, drain). After 3c those tests continue to exercise the
creator-pool wrapper + pool-core core. Behavior unchanged.

### Suggested commit message

```
H14 split (3c/N): extract shared admin handlers to pool-core

Moves pause / unpause / cancel-emergency-withdraw /
update-config-from-factory / ensure_not_drained to pool-core::admin
wholesale. Factors execute_emergency_withdraw into two phase-specific
pool-core helpers (execute_emergency_withdraw_initiate for phase 1,
execute_emergency_withdraw_core_drain for phase 2) so creator-pool
can wrap them with its commit-only extras (pre-threshold rejection,
CREATOR_EXCESS_POSITION sweep, DISTRIBUTION_STATE halt) without
polluting pool-core with commit-phase concepts.

Stays in creator-pool:
  - execute_recover_stuck_states + private recovery helpers
  - execute_emergency_withdraw (now a thin wrapper around pool-core
    with the commit-only concerns layered in)

Standard-pool's execute_emergency_withdraw (Step 4b) will also call
the two pool-core phase functions directly, with no wrapper needed
because standard pools have neither a threshold gate, a creator excess
position, nor distribution state.

No behavior change on the creator-pool path. Tests in
pool/src/testing/admin_tests.rs continue to pass.
```

## Step 3d — `query.rs` split

Source: `pool/src/query.rs` (524 lines). The top-level `query` dispatch
function stays per-contract (each contract has its own `QueryMsg` enum),
but every individual `query_*` handler is either shared or commit-only.

### Items that MOVE to `pool-core/src/query.rs`

Pure readers of shared state:

| Item | Reads |
|---|---|
| `query_pair_info` | `POOL_INFO` |
| `query_pool_state` | `POOL_STATE` |
| `query_fee_state` | `POOL_FEE_STATE` |
| `query_pool_info` | `POOL_FEE_STATE`, `NEXT_POSITION_ID`, `POOL_STATE` |
| `query_position` | `LIQUIDITY_POSITIONS`, `POOL_FEE_STATE` (+ `calculate_unclaimed_fees`) |
| `query_positions` | iterates `LIQUIDITY_POSITIONS` |
| `query_positions_by_owner` | iterates `OWNER_POSITIONS` |
| `query_config` | `POOL_STATE` |
| `query_simulation` | `POOL_INFO`, `POOL_SPECS`, `compute_swap` |
| `query_reverse_simulation` | `POOL_INFO`, `POOL_SPECS`, `compute_offer_amount` |
| `query_cumulative_prices` | `POOL_INFO`, `POOL_STATE`, `update_price_accumulator` |
| `query_fee_info` | `COMMITFEEINFO` (shared per 2a) |
| `query_is_paused` | `POOL_PAUSED` |
| `query_check_commit` | `IS_THRESHOLD_HIT` (shared per 2a; standard pool always returns `true`) |
| `query_for_factory` | builds `PoolStateResponseForFactory` from `POOL_STATE` + `POOL_INFO`; consumed by factory's oracle |
| `build_factory_response` | private helper for `query_for_factory` |

### `query_analytics` factoring (schema-uniformity surgery)

`PoolAnalyticsResponse` contains a `threshold_status: CommitStatus`
field. Both pool kinds expose it, but the computation differs:

- Creator-pool reads `USD_RAISED_FROM_COMMIT`, `COMMIT_LIMIT_INFO` (both
  commit-only Items per 2a) and returns `CommitStatus::InProgress` or
  `::FullyCommitted` depending on `IS_THRESHOLD_HIT`.
- Standard-pool has no raised/limit concept; always reports
  `CommitStatus::FullyCommitted`, `total_usd_raised: Uint128::zero()`,
  `total_bluechip_raised: Uint128::zero()`.

Resolution — same pattern as `execute_emergency_withdraw` in 3c:
factor the shared response construction out of the status resolution.

`pool-core/src/query.rs`:

```rust
/// Assembles the parts of PoolAnalyticsResponse that don't depend on
/// commit-phase state. Each contract provides the commit-adjacent
/// fields (threshold_status, total_usd_raised, total_bluechip_raised)
/// from whatever state it has access to.
pub fn query_analytics_core(
    deps: Deps,
    threshold_status: CommitStatus,
    total_usd_raised: Uint128,
    total_bluechip_raised: Uint128,
) -> StdResult<PoolAnalyticsResponse> {
    let analytics = POOL_ANALYTICS.load(deps.storage).unwrap_or_default();
    let pool_state = POOL_STATE.load(deps.storage)?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let next_position_id = NEXT_POSITION_ID.load(deps.storage)?;

    let current_price_0_to_1 = if !pool_state.reserve0.is_zero() {
        Decimal::from_ratio(pool_state.reserve1, pool_state.reserve0).to_string()
    } else { "0".to_string() };
    let current_price_1_to_0 = if !pool_state.reserve1.is_zero() {
        Decimal::from_ratio(pool_state.reserve0, pool_state.reserve1).to_string()
    } else { "0".to_string() };

    Ok(PoolAnalyticsResponse {
        analytics,
        current_price_0_to_1,
        current_price_1_to_0,
        total_value_locked_0: pool_state.reserve0,
        total_value_locked_1: pool_state.reserve1,
        fee_reserve_0: pool_fee_state.fee_reserve_0,
        fee_reserve_1: pool_fee_state.fee_reserve_1,
        threshold_status,
        total_usd_raised,
        total_bluechip_raised,
        total_positions: next_position_id,
    })
}
```

Creator-pool wrapper (`pool/src/query.rs`):

```rust
pub fn query_analytics(deps: Deps) -> StdResult<PoolAnalyticsResponse> {
    let usd_raised = USD_RAISED_FROM_COMMIT.load(deps.storage)?;
    let bluechip_raised = NATIVE_RAISED_FROM_COMMIT.load(deps.storage)?;
    let threshold_status = threshold_status_from(deps, usd_raised)?;
    pool_core::query::query_analytics_core(deps, threshold_status, usd_raised, bluechip_raised)
}
```

Standard-pool wrapper (Step 4b):

```rust
pub fn query_analytics(deps: Deps) -> StdResult<PoolAnalyticsResponse> {
    pool_core::query::query_analytics_core(
        deps,
        CommitStatus::FullyCommitted,
        Uint128::zero(),
        Uint128::zero(),
    )
}
```

### Items that STAY in `pool/src/query.rs` (creator-pool only)

| Item | Reads (commit-only) |
|---|---|
| `query_check_threshold_limit` | `USD_RAISED_FROM_COMMIT`, `COMMIT_LIMIT_INFO` |
| `threshold_status_from` | private helper used by both `query_check_threshold_limit` and `query_analytics` |
| `query_pool_committers` | iterates `COMMIT_INFO` Map |
| `query_factory_notify_status` | `PENDING_FACTORY_NOTIFY` |
| `query_last_committed` | (if implemented as a handler vs. inline in `query`) `COMMIT_INFO` load |
| `query_committing_info` (inline in `query` dispatch) | `COMMIT_INFO` load — lives inside `pub fn query` today, not a standalone fn; can stay inline in creator-pool's `query` dispatch |
| `query_analytics` | wrapper — see factoring above |
| **The top-level `pub fn query(deps, env, msg: QueryMsg)` dispatch** | stays in creator-pool; each crate has its own `QueryMsg` enum with different variants |

### `pool/src/query.rs` after the split

```rust
//! Creator-pool query dispatch + commit-only query handlers. Shared
//! handlers live in pool_core::query and are re-exported so existing
//! imports via `use crate::query::X;` resolve unchanged.
pub use pool_core::query::*;

use crate::error::ContractError;
use crate::msg::{
    CommitStatus, LastCommittedResponse, PoolAnalyticsResponse, PoolCommitResponse,
    QueryMsg, FactoryNotifyStatusResponse, CommitterInfo,
};
use crate::state::{
    COMMIT_INFO, COMMIT_LIMIT_INFO, IS_THRESHOLD_HIT, NATIVE_RAISED_FROM_COMMIT,
    USD_RAISED_FROM_COMMIT,
};
use cosmwasm_std::{entry_point, to_json_binary, Addr, Binary, Deps, Env, Order, StdResult, Uint128};
use cw_storage_plus::Bound;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        // Shared — forward to pool-core
        QueryMsg::Pair {} => to_json_binary(&pool_core::query::query_pair_info(deps)?),
        QueryMsg::PoolState {} => to_json_binary(&pool_core::query::query_pool_state(deps)?),
        QueryMsg::FeeState {} => to_json_binary(&pool_core::query::query_fee_state(deps)?),
        QueryMsg::PoolInfo {} => to_json_binary(&pool_core::query::query_pool_info(deps)?),
        QueryMsg::Position { position_id } => to_json_binary(&pool_core::query::query_position(deps, position_id)?),
        QueryMsg::Positions { start_after, limit } => to_json_binary(&pool_core::query::query_positions(deps, start_after, limit)?),
        QueryMsg::PositionsByOwner { owner, start_after, limit } => to_json_binary(&pool_core::query::query_positions_by_owner(deps, owner, start_after, limit)?),
        QueryMsg::Config {} => to_json_binary(&pool_core::query::query_config(deps)?),
        QueryMsg::Simulation { offer_asset } => to_json_binary(&pool_core::query::query_simulation(deps, offer_asset)?),
        QueryMsg::ReverseSimulation { ask_asset } => to_json_binary(&pool_core::query::query_reverse_simulation(deps, ask_asset)?),
        QueryMsg::CumulativePrices {} => to_json_binary(&pool_core::query::query_cumulative_prices(deps, env)?),
        QueryMsg::FeeInfo {} => to_json_binary(&pool_core::query::query_fee_info(deps)?),
        QueryMsg::GetPoolState { pool_contract_address } => pool_core::query::query_for_factory(deps, env, PoolQueryMsg::GetPoolState { pool_contract_address }),
        QueryMsg::GetAllPools {} => pool_core::query::query_for_factory(deps, env, PoolQueryMsg::GetAllPools {}),
        QueryMsg::IsPaused {} => pool_core::query::query_for_factory(deps, env, PoolQueryMsg::IsPaused {}),

        // Commit-only (this crate)
        QueryMsg::IsFullyCommited {} => to_json_binary(&query_check_threshold_limit(deps)?),
        QueryMsg::CommittingInfo { wallet } => {
            let addr = deps.api.addr_validate(&wallet)?;
            let info = COMMIT_INFO.may_load(deps.storage, &addr)?;
            to_json_binary(&info)
        }
        QueryMsg::LastCommited { wallet } => { /* unchanged inline body */ }
        QueryMsg::PoolCommits { pool_contract_address, min_payment_usd, after_timestamp, start_after, limit } =>
            to_json_binary(&query_pool_committers(deps, pool_contract_address, min_payment_usd, after_timestamp, start_after, limit)?),
        QueryMsg::FactoryNotifyStatus {} => to_json_binary(&query_factory_notify_status(deps)?),

        // Hybrid — wrapper computes creator-only pieces, pool-core assembles response
        QueryMsg::Analytics {} => to_json_binary(&query_analytics(deps)?),
    }
}

// Creator-only handlers + the query_analytics wrapper stay below.
pub fn query_check_threshold_limit(...) { /* unchanged */ }
fn threshold_status_from(...) { /* unchanged, private */ }
pub fn query_pool_committers(...) { /* unchanged */ }
pub fn query_factory_notify_status(...) { /* unchanged */ }
pub fn query_analytics(...) { /* wrapper shown above */ }
```

### Update `pool-core/src/lib.rs`

```rust
pub mod error;
pub mod state;
pub mod asset;
pub mod swap;
pub mod msg;
pub mod generic;
pub mod liquidity_helpers;
pub mod liquidity;
pub mod admin;
pub mod query;    // 3d — final pool-core module
```

### Cargo.toml changes

None.

### Expected compile-error patterns after 3d

1. **`CommitStatus` / `PoolAnalyticsResponse` path resolution** — both
   are in `pool_core::msg` after 2d. The creator-pool wrapper's
   `use crate::msg::{CommitStatus, PoolAnalyticsResponse};` resolves
   via the glob re-export. ✓

2. **`threshold_status_from` visibility** — private to `pool/src/query.rs`.
   Only used by `query_check_threshold_limit` and `query_analytics`,
   both in the same file. No change. ✓

3. **`query_analytics_core` signature** — takes `Deps` rather than
   `&Deps`. Trivial, consistent with the rest of pool-core.

4. **QueryResponses derive** — creator-pool's `QueryMsg` uses
   `#[returns(PoolAnalyticsResponse)]` on the `Analytics` variant.
   Since the response type is re-exported from pool-core, the derive
   resolves it through the glob re-export. ✓

5. **Factory forward queries** — factory queries the pool via
   `PoolQueryMsg::GetPoolState {...}`, `::GetAllPools {}`, `::IsPaused {}`.
   These are shared (moved to pool-core). Creator-pool's `query` dispatch
   forwards to `pool_core::query::query_for_factory`. Standard-pool
   (Step 4b) does the same. ✓

### Verification after 3d

```
cargo check -p pool-core
cargo check -p pool
cargo test -p pool   # includes query_tests.rs
```

All existing query tests continue to pass — they construct `QueryMsg::X {...}`
and compare the deserialized response, and neither shape nor values
change.

### Step 3 is complete after 3d

Full pool-core module layout after Step 3:

```
packages/pool-core/src/
├── lib.rs
├── error.rs           (C1)
├── state.rs           (2a)
├── asset.rs           (2b)
├── swap.rs            (2c)
├── msg.rs             (2d)
├── generic.rs         (3b)
├── liquidity_helpers.rs (3a)
├── liquidity.rs       (3a)
├── admin.rs           (3c)
└── query.rs           (3d)
```

Creator-pool (`pool/`) at this point contains only:
- `contract.rs` — execute dispatch (still has `require_commit_pool`
  guards + tagged-enum `PoolInstantiateMsg` dispatch; Step 4 reverts)
- `commit.rs` — commit-phase handlers (unchanged)
- `admin.rs` — `execute_recover_stuck_states` + recovery helpers +
  `execute_emergency_withdraw` wrapper
- `generic_helpers.rs` — threshold payout + distribution batching +
  commit-info + mint_tokens
- `liquidity_helpers.rs` — `execute_claim_creator_{fees,excess}` only
- `query.rs` — commit-only queries + dispatch + `query_analytics` wrapper
- `state.rs` — commit-only Items + structs + (soon-to-be-deleted in 4d)
  `POOL_KIND`/`load_pool_kind`
- `msg.rs` — `ExecuteMsg` / `QueryMsg` / `MigrateMsg` / `PoolInstantiateMsg`
  enum / `CommitPoolInstantiateMsg` / commit-only response types
- `asset.rs`, `error.rs`, `swap_helper.rs`, `liquidity.rs` — all thin
  re-export shims at this point
- `mock_querier.rs` — unchanged
- `testing/` — tests unchanged, resolving through the re-export shims

### Suggested commit message

```
H14 split (3d/N): extract shared queries to pool-core

Moves 16 query handlers to pool-core::query:
  - query_pair_info, query_pool_state, query_fee_state,
    query_pool_info, query_position, query_positions,
    query_positions_by_owner, query_config, query_simulation,
    query_reverse_simulation, query_cumulative_prices,
    query_fee_info, query_is_paused, query_check_commit,
    query_for_factory, build_factory_response

Factors query_analytics the same way execute_emergency_withdraw was
factored in 3c: pool-core exposes query_analytics_core which takes
the commit-adjacent fields (threshold_status, total_usd_raised,
total_bluechip_raised) as parameters and assembles the shared response
body. Creator-pool wraps it with commit-phase computation; standard-
pool (Step 4b) will wrap it with zero/FullyCommitted constants.

Stays in pool/src/query.rs:
  - pub fn query (dispatch — per-contract QueryMsg enum)
  - query_check_threshold_limit + threshold_status_from helper
  - query_pool_committers, query_factory_notify_status
  - query_analytics wrapper
  - inline commit-only variants in the dispatch (CommittingInfo,
    LastCommited)

Step 3 (pool-core operational modules) is complete after this commit.
Step 4 (standard-pool crate + factory dual-code_id + creator-pool
scaffolding revert) comes next.

No behavior change. All existing pool query tests pass unchanged.
```

## Step 4a — `git mv pool/ creator-pool/`

The smallest sub-step in the entire refactor. Rename the directory,
rename the crate, update the workspace member list. Scope is
deliberately tight so git history follows via `git mv` rather than
getting split into a delete+add pair.

### What changes

1. **Rename the directory** via `git mv`:
   ```
   git mv pool creator-pool
   ```
   Git records this as a rename (as long as the file contents don't
   also change simultaneously — keep 4a pure), so `git log --follow`
   continues to work on every source file inside.

2. **Workspace root `Cargo.toml`** — replace `"pool"` with `"creator-pool"`
   in the `members = [...]` array:
   ```diff
   [workspace]
   members = [
       "packages/*",
   -   "pool",
   +   "creator-pool",
       "factory",
       "mockoracle",
       "expand-economy",
       "router",
   ]
   ```

3. **`creator-pool/Cargo.toml`** (the former `pool/Cargo.toml`) —
   rename the crate itself:
   ```diff
   [package]
   - name = "pool"
   + name = "creator-pool"
   ```
   Other fields (version, edition, lib config, dependencies,
   dev-dependencies) stay identical. The crate still depends on
   `pool-core = { path = "../packages/pool-core" }` and
   `pool-factory-interfaces = { path = "../packages/pool-factory-interfaces" }`
   via paths that are unchanged by this rename.

### Tools that reference the artifact filename

The cosmwasm optimizer produces a wasm file named after the crate with
`-` converted to `_`:

- Before: crate `pool` → `artifacts/pool.wasm`
- After: crate `creator-pool` → `artifacts/creator_pool.wasm`

Every script that currently references `pool.wasm` needs to update.
Sweep inventory:

| File | References | Handled in |
|---|---|---|
| `Makefile` | 5 references (cp, cosmwasm-check, wasm store) | **4a** — small, predictable, land with the rename |
| `deploy_full_stack_mock_oracle.sh` | 1 | 5b (script also gains standard_pool.wasm) |
| `run_comprehensive_test.sh` | 1 | 5b |
| `run_full_integration_test.sh` | 1 | 5b |
| `run_full_test.sh` | 1 | 5b |
| `upload_wasms_and_contracts_plus_starter_pool.sh` | 1 | 5b |
| `run_threshold_20pct_guard_test.sh` | 1 | 5b |
| `run_nft_position_transfer_test.sh` | 1 | 5b |
| `run_local_test.sh` | 1 | 5b |
| `optimize.sh` | 0 (invokes `cosmwasm/workspace-optimizer` which walks the workspace; the optimizer picks up all member crates with `[lib] crate-type = ["cdylib"]` automatically) | no change needed |

So 4a ships: directory rename + workspace-root Cargo.toml + crate-name
Cargo.toml + `Makefile` 5 swaps. Shell scripts land in 5b alongside
their existing re-edit for the standard-pool wasm.

### `Makefile` edits

Open and sweep for `pool.wasm`:

```diff
- cp target/$(WASM_TARGET)/release/pool.wasm $(ARTIFACTS)/pool.wasm
+ cp target/$(WASM_TARGET)/release/creator_pool.wasm $(ARTIFACTS)/creator_pool.wasm

- cosmwasm-check $(ARTIFACTS)/pool.wasm
+ cosmwasm-check $(ARTIFACTS)/creator_pool.wasm

  # (three occurrences similar)
```

After 5b the Makefile will gain a parallel set of lines for
`standard_pool.wasm`. For 4a just swap in place.

### What does NOT change in 4a

- **No Rust source edits.** Every `.rs` file inside the renamed directory
  keeps its byte content unchanged. `use crate::X` paths still resolve
  (`crate::` always refers to the enclosing crate regardless of its
  name). External imports like `factory/src/*.rs` don't import from
  `pool` by path (the factory never depends on the pool crate at the
  Rust level — it talks via `pool_factory_interfaces` and sends
  `WasmMsg::Instantiate` with a code_id).

- **No `pool-factory-interfaces` change.** It references `pool-core`
  by path (`../pool-core`) and doesn't know about the pool crate(s).

- **No `pool-core` change.** It only depends on `pool-factory-interfaces`
  by path (`../pool-factory-interfaces`).

- **No factory/mockoracle/expand-economy/router change.** None have a
  path dep on `pool/`. The only Cargo.toml reference would be a dev-dep,
  and sweep shows creator-pool itself has `factory = { path = "../factory", ... }`
  in `[dev-dependencies]`, but the reverse (factory depending on pool)
  does not exist.

### Expected compile-error patterns after 4a

1. **`cargo build` fails to find crate `pool`** — any tooling that
   invoked `cargo build -p pool` must now say `cargo build -p creator-pool`.
   Sweep CI workflows / dev scripts for `-p pool` and update.

2. **`make build` output filename changed** — addressed by the Makefile
   edits above.

3. **Existing `.wasm` artifact** — `artifacts/pool.wasm` from prior
   builds will still exist until `cargo clean` or `make clean`. It's
   stale after the rename. Recommend: `rm -f artifacts/pool.wasm`
   as a post-rename manual step so nobody accidentally deploys it.

4. **IDE/LSP state** — rust-analyzer caches crate paths. Restart the
   language server after running `git mv`.

5. **Docker volume caches** — `optimize.sh` mounts per-project
   `${project_basename}_cache` volumes. The volume name is derived from
   the REPO-LEVEL directory name (not the crate name), so it stays
   the same. No cache purge needed.

### Verification after 4a

```
cargo check --workspace
cargo test -p creator-pool   # was -p pool
make build
ls -la artifacts/
# Expect: creator_pool.wasm, factory.wasm, oracle.wasm, expand_economy.wasm
# Should NOT see: pool.wasm
```

### Suggested commit message

```
H14 split (4a/N): git mv pool/ creator-pool/

Mechanical directory+crate rename. `pool` becomes `creator-pool` to
reflect its narrowed scope (two-phase commit-pool only; the shape-
agnostic xyk logic moved to pool-core in Steps 2–3; the forthcoming
standard-pool contract lands in 4b).

Diff is deliberately scoped to:
  - git mv pool creator-pool
  - Cargo.toml (workspace): "pool" -> "creator-pool" in members
  - creator-pool/Cargo.toml: name = "pool" -> name = "creator-pool"
  - Makefile: 5 x pool.wasm -> creator_pool.wasm

No Rust source changes (git treats the move as a rename — file
history follows). Factory, router, mockoracle, expand-economy, and
the two packages (pool-core, pool-factory-interfaces) all compile
unchanged.

Shell-script updates for the renamed wasm artifact are deferred to
Step 5b, which also rewrites those scripts to upload the new
standard_pool.wasm alongside. Doing them twice (once in 4a, once in
5b) would be noisy; the 4a diff stays focused on directory rename.

No behavior change. All existing tests pass with `cargo test -p
creator-pool`.
```

## Step 4b-i — `standard-pool/` crate skeleton

Minimum viable crate: Cargo manifest, workspace registration, empty
`lib.rs` with module stubs, and the one-line `error.rs` re-export
matching the pattern creator-pool already uses. No contract entry
points yet — those land in 4b-ii. Commit-alone should build cleanly
as an empty library crate.

### `standard-pool/Cargo.toml`

Mirror `creator-pool/Cargo.toml` with the crate renamed and the
commit-pool-only dev-deps trimmed.

```toml
[package]
name = "standard-pool"
version.workspace = true
authors = ["bestselection18 <noahsflood908@gmail.com>"]
edition = "2021"
description = "Bluechip standard xyk pool (no commit phase)"
license = "Apache-2.0"
repository = "https://github.com/bestselection18"

exclude = [
  "artifacts/*",
]

[lib]
crate-type = ["cdylib", "rlib"]

[features]
# for more explicit tests, cargo test --features=backtraces
backtraces = []
# use library feature to disable all instantiate/execute/query exports
library = []

[package.metadata.scripts]
optimize = { workspace = true }

[dependencies]
cosmwasm-schema = { workspace = true }
cw2 = { workspace = true }
cw20 = { workspace = true }

cosmwasm-std = { workspace = true }
cw-storage-plus = { workspace = true }
thiserror = { workspace = true }

pool-factory-interfaces = { path = "../packages/pool-factory-interfaces" }
pool-core = { path = "../packages/pool-core" }
cw-utils = { workspace = true }

[dev-dependencies]
cw-multi-test = { workspace = true }
easy-addr = { workspace = true }
factory = { path = "../factory", features = ["library"] }
oracle = { path = "../mockoracle", features = ["library"] }
cw20-base = { workspace = true }
```

Notes:

- `cdylib` is necessary — standard-pool DOES compile to a wasm artifact
  (unlike pool-core, which is `rlib` only). Each contract produces its
  own wasm.
- `library` feature is kept in parallel with creator-pool's convention:
  when enabled, it strips the `#[entry_point]` exports so the crate
  can be used as a plain Rust library in other crates' tests (e.g.,
  integration tests that want to instantiate a standard-pool in a
  `cw-multi-test` environment without duplicating the contract logic).
- `cw20` + `cw20-base` kept because shared LP code emits
  `Cw20ExecuteMsg::Transfer` / `TransferFrom` / `Mint` messages, so
  the crate compiles against cw20's type definitions. Dev-dep on
  `cw20-base` is for multi-test integrations.
- `factory` and `oracle` dev-deps mirror creator-pool: useful for
  integration tests that exercise the full stack.

### Workspace root `Cargo.toml`

Add `"standard-pool"` to the members array (alphabetical placement or
grouped with `"creator-pool"` — no compiler preference):

```diff
[workspace]
members = [
    "packages/*",
    "creator-pool",
+   "standard-pool",
    "factory",
    "mockoracle",
    "expand-economy",
    "router",
]
```

### `standard-pool/src/lib.rs` (skeleton)

```rust
//! Bluechip Standard Pool — plain xyk pool around two pre-existing
//! assets (any combination of `Native` and `CreatorToken`). No commit
//! phase, no threshold, no distribution; immediately tradeable and
//! depositable at creation.
//!
//! The vast majority of this crate's logic lives in `pool_core`. The
//! modules below are thin entry-point wrappers: they define the
//! `#[entry_point]` exports and route each `ExecuteMsg` / `QueryMsg`
//! variant to the corresponding `pool_core::*` handler.

pub mod contract;
pub mod error;
pub mod msg;
pub mod query;

#[cfg(test)]
mod testing;
```

The `testing` submodule is added so Step 5a has a place to land new
integration tests for standard-pool without re-working lib.rs.

### `standard-pool/src/error.rs`

Follows the same pattern as `creator-pool/src/error.rs` after C1's
`error.rs` extraction:

```rust
//! Re-export of the shared `ContractError` type from `pool-core`.
//! Every existing `use crate::error::ContractError;` resolves through
//! this glob re-export unchanged. Creator-pool uses the identical
//! pattern; both contracts produce the same error type on the wire.
pub use pool_core::error::*;
```

The full `ContractError` enum has commit-phase variants (ShortOfThreshold,
TooFrequentCommits, InvalidThresholdParams, etc.) that standard-pool
never constructs. Rust doesn't warn on unconstructed enum variants of
public types, and sharing the type keeps client-side error handling
uniform across both pool kinds.

### `standard-pool/src/msg.rs` (placeholder)

Landed in 4b-ii. For 4b-i, a one-line placeholder keeps the module
declaration resolvable:

```rust
//! ExecuteMsg/QueryMsg/MigrateMsg definitions land in H14 split Step 4b-ii.
```

### `standard-pool/src/contract.rs` (placeholder)

Similarly:

```rust
//! instantiate/execute/query/migrate/reply entry points land in H14 split
//! Step 4b-ii.
```

### `standard-pool/src/query.rs` (placeholder)

```rust
//! Top-level query dispatch lands in H14 split Step 4b-ii. Shared
//! handler bodies already live in pool_core::query.
```

### `standard-pool/src/testing/mod.rs` (placeholder)

```rust
//! Integration tests for standard-pool land in H14 split Step 5a.
```

### Cargo.toml of `factory/`, `router/`, `mockoracle/`, `expand-economy/` — no changes

None of these crates reference `standard-pool` at the Rust level
(factory only dispatches via `WasmMsg::Instantiate { code_id, msg }`
using the flat `StandardPoolInstantiateMsg` from `pool_factory_interfaces`).

### `Makefile` — add a build step for standard-pool

After 4a's `pool.wasm` → `creator_pool.wasm` rename, the Makefile
builds one pool artifact. 4b-i adds a parallel line for the new wasm:

```diff
- cp target/$(WASM_TARGET)/release/creator_pool.wasm $(ARTIFACTS)/creator_pool.wasm
+ cp target/$(WASM_TARGET)/release/creator_pool.wasm $(ARTIFACTS)/creator_pool.wasm
+ cp target/$(WASM_TARGET)/release/standard_pool.wasm $(ARTIFACTS)/standard_pool.wasm
```

Same for the `cosmwasm-check` lines (add a second invocation for
`standard_pool.wasm`).

### Expected compile-error patterns after 4b-i

1. **`cargo check -p standard-pool`** — empty crate with placeholder
   modules. Expect warnings for unused imports (none yet) but no errors.
   Every placeholder module is a `//!`-doc-comment-only file, which is
   valid Rust.

2. **`cargo check --workspace`** — succeeds. The new crate is
   registered as a workspace member but has no content that depends
   on anything else yet.

3. **`cargo test --workspace`** — runs all existing tests plus a
   zero-test run on standard-pool. Passes.

4. **Makefile**: `make build` produces `creator_pool.wasm` + an empty
   `standard_pool.wasm`. The wasm might not meet the optimizer's size
   check if the crate has `crate-type = ["cdylib"]` but no actual
   `#[entry_point]`s — test this when you run the build. Fix in 4b-ii
   by adding the entry points.

   If 4b-i alone is committed and you want `make build` to succeed,
   temporarily leave standard-pool as `crate-type = ["rlib"]` only
   and add `"cdylib"` in 4b-ii when entry points land. That avoids
   producing an empty cdylib. Recommended ordering: **co-land 4b-i
   and 4b-ii** if you want a clean `make build`, or accept a
   temporarily-unbuildable wasm between the two commits.

### Verification after 4b-i

```
cargo check --workspace     # must pass
cargo check -p standard-pool  # must pass, zero warnings on module stubs
# make build deferred until 4b-ii
```

### Suggested commit message

```
H14 split (4b-i/N): add standard-pool/ crate skeleton

New library+contract crate for the standard (non-commit-phase) xyk
pool. Skeleton only: Cargo.toml, workspace registration, lib.rs
module declarations, error.rs re-export of pool_core. All other
modules are placeholder files with //! doc-only contents; entry
points (instantiate/execute/query/migrate/reply) + msg definitions
land in 4b-ii.

Cargo layout:
  standard-pool/
  ├── Cargo.toml       (mirrors creator-pool; depends on pool-core
  │                     + pool-factory-interfaces; dev-deps on
  │                     factory + oracle for integration tests)
  └── src/
      ├── lib.rs
      ├── error.rs     (pub use pool_core::error::*;)
      ├── msg.rs       (placeholder)
      ├── contract.rs  (placeholder)
      ├── query.rs     (placeholder)
      └── testing/
          └── mod.rs   (placeholder for Step 5a)

Workspace root Cargo.toml: adds "standard-pool" to members.

Makefile: adds a parallel cp line for standard_pool.wasm alongside
the existing creator_pool.wasm. The wasm artifact itself will be
effectively empty after 4b-i alone — recommend co-landing 4b-ii to
avoid an interim non-buildable state.

No factory, router, mockoracle, or expand-economy changes.
No behavior change on creator-pool. cargo check --workspace passes.
```
