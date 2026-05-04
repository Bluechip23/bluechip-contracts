# Fuzzing Review (Phase 1 + Phase 2) and Gap Analysis

## Scope reviewed
- `fuzz.sh`, `FUZZING.md`, `fuzz-stateful/*`, `fuzz/fuzz_targets/*`.
- Workspace and per-crate `Cargo.toml` files.
- Contract msg/state surfaces for `factory`, `creator-pool`, `standard-pool`, `expand-economy`, `router`, `mockoracle`.

---

## Phase 1: protocol orientation summary

### Entry points by contract
- **factory**
  - Instantiate: `instantiate`
  - Execute highlights: config propose/apply/cancel, creator pool creation, standard pool creation, oracle update/rotation, pool pause/unpause/emergency/recovery forwards, threshold notification, upgrade flows, anchor-pool bootstrap, bounty controls. (`factory/src/msg.rs`)
  - Query highlights: factory snapshot, creator token info, oracle conversion/internal oracle query, bounties, pool creation status. (`factory/src/query.rs`)
- **creator-pool**
  - Instantiate: pool init with commit threshold + payout config.
  - Execute highlights: commit, swaps (native + cw20 hook), deposit/remove liquidity, fee collection, distribution progression, pause/unpause/emergency lifecycle, creator claims, retry factory threshold notify. (`creator-pool/src/msg.rs`)
  - Query highlights: pair/config/simulations, fee state, threshold status, committer state, positions, analytics, distribution state, paused state. (`creator-pool/src/msg.rs`)
- **standard-pool**
  - Instantiate: standard XYK pool.
  - Execute highlights: swaps, deposit/remove liquidity, fee collection, pause/unpause/emergency lifecycle, factory config updates.
  - Query highlights: pair/config/simulation/reverse simulation, fee state, positions, pool info/state, paused state. (`standard-pool/src/msg.rs`)
- **expand-economy**
  - Instantiate: owner/factory + denom config.
  - Execute highlights: expansion request, config propose/apply/cancel, withdrawal propose/execute/cancel. (`expand-economy/src/msg.rs`)
  - Query highlights: config, balance by denom. (`expand-economy/src/msg.rs`)
- **router**
  - Instantiate: factory/admin/denom.
  - Execute highlights: multihop, cw20 receive, internal hop execution, assert-received, config propose/apply/cancel. (`router/src/msg.rs`)
  - Query highlights: simulate multihop, config. (`router/src/msg.rs`)
- **mockoracle**
  - Instantiate: empty.
  - Execute: set price.
  - Query: get price, conversion feed response. (`mockoracle/src/msg.rs`)

### Storage items (name -> key type)
- **factory**
  - `FACTORYINSTANTIATEINFO` -> `Item<FactoryInstantiate>`
  - `POOL_CREATION_CONTEXT` -> `Map<u64, PoolCreationContext>`
  - `POOL_COUNTER`, `COMMIT_POOL_COUNTER` -> `Item<u64>`
  - `POOLS_BY_ID` -> `Map<u64, PoolDetails>`
  - `POOLS_BY_CONTRACT_ADDRESS` -> `Map<Addr, PoolStateResponseForFactory>`
  - `POOL_THRESHOLD_MINTED` -> `Map<u64, bool>`
  - plus pending config/upgrade/oracle/bootstrap/rate-limit/bounty maps/items. (`factory/src/state.rs`)
- **creator-pool**
  - Commit-specific: `USD_RAISED_FROM_COMMIT`, `NATIVE_RAISED_FROM_COMMIT`, `THRESHOLD_PROCESSING`, `PENDING_FACTORY_NOTIFY`, `DISTRIBUTION_STATE`, `LAST_THRESHOLD_ATTEMPT`, `CREATOR_EXCESS_POSITION`, `COMMIT_LIMIT_INFO`.
  - Per-user maps: `COMMIT_INFO: Map<&Addr, Committing>`, `COMMIT_LEDGER: Map<&Addr, Uint128>`, `LAST_CONTINUE_DISTRIBUTION_AT: Map<&Addr, u64>`.
  - Also inherits shared `pool_core::state::*` items (reserves, positions, config, fees, etc.). (`creator-pool/src/state.rs`, `packages/pool-core/src/state.rs`)
- **standard-pool**
  - Uses shared `pool_core::state` items (reserves, positions, config/fees, pause/emergency markers).
- **expand-economy**
  - `CONFIG: Item<Config>`
  - `EXPANSION_WINDOW: Item<ExpansionWindow>`
  - `LAST_EXPANSION_AT_RECIPIENT: Map<&str, Timestamp>`
  - `PENDING_WITHDRAWAL`, `PENDING_CONFIG_UPDATE` as `Item<...>`. (`expand-economy/src/state.rs`)
- **router**
  - `CONFIG: Item<Config>`
  - `PENDING_CONFIG: Item<PendingConfigUpdate>`. (`router/src/state.rs`)
- **mockoracle**
  - `PRICES: Map<&str, PriceResponse>`. (`mockoracle/src/oracle_contract.rs`)

### Cross-contract call graph (main paths)
- factory -> creator-pool/standard-pool instantiate on pool creation.
- creator-pool -> factory `NotifyThresholdCrossed` / bounty pathways.
- factory -> expand-economy `RequestExpansion`.
- factory -> pools for admin forwards (`Pause`, `Unpause`, `EmergencyWithdraw`, recovery, config update).
- factory -> Pyth/mockoracle query for price conversion.
- router -> pool swap execute/query and factory query for route validation.
- pool -> cw20 token contracts (`Send`, `Transfer`, allowances).

### Numeric types used in arithmetic
- Widespread: `Uint128`, `u64`, `u32`, `u16`, `Decimal`, timestamps (`u64` seconds), and i64/i32-style oracle fields in mock/Pyth compatibility surfaces.
- Fuzz math targets additionally parse `u128` tuples and vectorized `u128` commitments.

---

## Phase 2: invariant inventory (recommended 22)

> Signature pattern requested:
`fn check_invariant_NAME(app: &App, ...) -> Result<(), String>`

### A) Conservation
1. `fn check_invariant_pool_native_reserve_backed(app: &App, pool: Addr) -> Result<(), String>`
2. `fn check_invariant_pool_cw20_reserve_backed(app: &App, pool: Addr, token: Addr) -> Result<(), String>`
3. `fn check_invariant_total_liquidity_ge_sum_positions(app: &App, pool: Addr) -> Result<(), String>`
4. `fn check_invariant_commit_ledger_sum_le_total_committed_usd(app: &App, pool: Addr) -> Result<(), String>`
5. `fn check_invariant_creator_fee_pot_nonnegative_and_claimable(app: &App, pool: Addr) -> Result<(), String>`

### B) Monotonicity
6. `fn check_invariant_threshold_sticky_once_hit(app: &App, pool: Addr) -> Result<(), String>`
7. `fn check_invariant_usd_raised_non_decreasing_pre_threshold(app: &App, pool: Addr) -> Result<(), String>`
8. `fn check_invariant_pool_threshold_minted_flag_non_reverting(app: &App, pool_id: u64) -> Result<(), String>`
9. `fn check_invariant_distribution_progress_monotone(app: &App, pool: Addr) -> Result<(), String>`
10. `fn check_invariant_expand_economy_window_spent_monotone_within_window(app: &App) -> Result<(), String>`

### C) Authorization
11. `fn check_invariant_only_factory_can_pause_unpause_emergency(app: &App, pool: Addr) -> Result<(), String>`
12. `fn check_invariant_only_factory_can_update_pool_config(app: &App, pool: Addr) -> Result<(), String>`
13. `fn check_invariant_only_creator_can_claim_creator_fees(app: &App, pool: Addr) -> Result<(), String>`
14. `fn check_invariant_only_admin_can_factory_timelock_actions(app: &App) -> Result<(), String>`
15. `fn check_invariant_only_admin_can_router_config_changes(app: &App) -> Result<(), String>`

### D) Arithmetic safety
16. `fn check_invariant_swap_simulation_no_overflow_for_live_state(app: &App, pool: Addr) -> Result<(), String>`
17. `fn check_invariant_threshold_usd_conversion_matches_reference(app: &App, pool: Addr) -> Result<(), String>`
18. `fn check_invariant_expand_formula_output_within_bounds(app: &App, supply: u128, x: u128) -> Result<(), String>`
19. `fn check_invariant_fee_bps_and_spread_bounds(app: &App, pool: Addr) -> Result<(), String>`

### E) Phase exclusivity
20. `fn check_invariant_no_swap_before_commit_threshold_in_creator_pool(app: &App, pool: Addr) -> Result<(), String>`
21. `fn check_invariant_no_commit_after_threshold_cross(app: &App, pool: Addr) -> Result<(), String>`
22. `fn check_invariant_emergency_drained_pool_rejects_state_changes(app: &App, pool: Addr) -> Result<(), String>`

---

## Review of current fuzz setup vs contract behavior

### What is already good
- `fuzz.sh` runs both quick and full stateful proptest and pure-math mirrors; supports long mode with cargo-fuzz targets. (`fuzz.sh`)
- Stateful harness includes legal + intentionally-illegal actions (unauthorized config, fake threshold notify, zero oracle, emergency misuse). (`fuzz-stateful/src/actions.rs`)
- Invariants already check reserve backing, threshold stickiness, liquidity over-claim prevention, and drained-pool operation rejection. (`fuzz-stateful/src/invariants.rs`)
- Three cargo-fuzz targets exist and are wired in `fuzz/Cargo.toml`. (`fuzz/fuzz_targets/*.rs`, `fuzz/Cargo.toml`)

### Potential gaps
1. **`fuzz.sh` quick-mode still runs both quick+full stateful passes** (quick is not really ~30s by default on slower CI hosts).
2. **No explicit invariant for “commit forbidden post-threshold”** (relies on action rejection but not asserted globally each step).
3. **No invariant binding factory `POOL_THRESHOLD_MINTED` to exactly-once economics** (only sticky flag check, not amount/cap correctness).
4. **No cross-check that oracle staleness/zero/negative branches reject exactly where intended** for all relevant entrypoints.
5. **No explicit invariant for timelock correctness** (factory/router/expand-economy propose/apply/cancel temporal constraints).
6. **No invariant for expand-economy rolling cap + recipient rate-limit conservation** during repeated notify/retry storms.
7. **No strict constant-product tolerance invariant over executed swaps** (math target checks formula, stateful harness should bind live reserve transitions too).
8. **Action space misses explicit “stale oracle > max age” action toggle** (only rate mutation value variants are present).
9. **No invariant asserting distribution completion drains `COMMIT_LEDGER` as expected**.
10. **No direct property on NFT position ownership integrity during add/remove/claim sequences**.

