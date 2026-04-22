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
