# Fuzzing Bluechip Contracts

This repo ships two complementary fuzzing layers:

1. **Stateful proptest harness** (`fuzz-stateful/`) — drives sequences
   of cw-multi-test actions against real contract instances and checks
   conservation, monotonicity, authorization, and phase-exclusivity
   invariants after every action.
2. **Pure-math fuzzers** — three targets: the expand-economy decay
   polynomial, the constant-product swap, and the USD-conversion +
   threshold-check. Each is provided in two forms:
   - cargo-fuzz (`fuzz/fuzz_targets/*.rs`) — libFuzzer, nightly only.
   - proptest mirror (`fuzz-stateful/tests/proptest_pure_math.rs`) — runs
     on stable, used by CI.

## Quick start

```sh
./fuzz.sh                # ~10s; runs stateful + pure-math proptests
./fuzz.sh long           # +5min cargo-fuzz per pure-math target
```

`fuzz.sh` exits non-zero on any failure or invariant violation.

## Quick-feedback runs (during development)

```sh
# Stateful, 32 short sequences (matches CI quick gate)
PROPTEST_CASES=32 cargo test -p fuzz-stateful --release fuzz_stateful_quick

# Stateful, 256 full sequences
cargo test -p fuzz-stateful --release fuzz_stateful

# Pure-math (1024 cases each on stable)
cargo test -p fuzz-stateful --release --test proptest_pure_math

# Per-step debug print (action types / outcome counts)
FUZZ_DEBUG=1 PROPTEST_CASES=8 cargo test -p fuzz-stateful --release \
    fuzz_stateful_quick -- --nocapture
```

## Overnight runs

```sh
# Stateful: 4096 sequences (~30s wall-clock at present)
PROPTEST_CASES=4096 cargo test -p fuzz-stateful --release fuzz_stateful

# cargo-fuzz: 8 hours per target (requires nightly + cargo-fuzz)
rustup toolchain install nightly        # one-time
cargo install cargo-fuzz                # one-time
cd fuzz
for t in fuzz_expand_economy_formula fuzz_swap_math fuzz_threshold_check; do
  cargo +nightly fuzz run "$t" -- -max_total_time=28800
done
```

cargo-fuzz writes a per-target corpus under `fuzz/corpus/<target>/` and
crash inputs under `fuzz/artifacts/<target>/`. Both are picked up
automatically on subsequent runs.

## Reproducing a failure from a regression file

When the proptest stateful harness finds an invariant violation it:

1. Prints the full ordered action list in the test output (a complete
   sequence you can paste into a regression test).
2. Writes the failing seed to
   `fuzz-stateful/proptest-regressions/<test_name>.txt`.
3. On every subsequent run, proptest replays that seed first, so the
   case re-fires until you delete the file.

To turn one into a permanent regression test, copy the printed action
list into `fuzz-stateful/tests/fuzz_stateful.rs`:

```rust
#[test]
fn regression_<short_name>() {
    use fuzz_stateful::*;
    let mut world = build_world(true);
    apply(&mut world, Action::CreateCreatorPool { decimals: 6 });
    apply(&mut world, Action::Commit { user_idx: 0, pool_idx: 0, amount: 30_000_000_000 });
    // ...paste the rest...
    check_all(&mut world).expect("invariants must hold post-regression");
}
```

For cargo-fuzz, every crash drops a deterministic-replay file at
`fuzz/artifacts/<target>/crash-<sha>`. Replay with:

```sh
cd fuzz
cargo +nightly fuzz run <target> artifacts/<target>/crash-<sha>
```

## Adding a new action

Every action lives in `fuzz-stateful/src/actions.rs::Action`:

1. Add a new variant — derive macro `proptest_derive::Arbitrary` will
   generate a generator automatically. For non-trivial fields use
   `#[proptest(strategy = "…")]` to constrain the value space.
2. Add a match arm in `apply()`. For legal actions, accept any contract
   error as `OutcomeKind::Rejected`. For illegal-by-design actions
   (auth bypasses, etc.), `panic!()` if the action *succeeds* — that is
   itself an invariant violation.
3. If the action observes a new piece of pool state, also extend
   `world::PoolHandle` and `invariants::check_pool_invariants` to track
   and assert it.

## Adding a new invariant

Each invariant lives in `fuzz-stateful/src/invariants.rs` and is
expected to:

- Take `&mut World` (we may need to thread observed-state updates back
  into the per-pool snapshot).
- Return `Result<(), Violation>`.
- Use `world.app.wrap().query_*` for live state — never read shared
  mutable test state directly.

Violations carry a stable `name` (used for log filtering / dedup) and a
formatted `detail` string. Both surface in the proptest failure output.

## Why a factory shim?

The production factory is ~5kloc of oracle bootstrap (Pyth pull, anchor
pool, internal TWAP, 48h timelocks). The fuzz harness replaces it with
`fuzz-stateful::factory_shim` — a 200-line contract that implements the
exact subset of the factory→pool query/exec interface the pool actually
calls back into:

| Surface | Implementation |
|---|---|
| `FactoryQueryMsg::ConvertBluechipToUsd { amount }` | `amount * rate / 1e6` |
| `FactoryQueryMsg::ConvertUsdToBluechip { amount }` | `amount * 1e6 / rate` |
| `FactoryQueryMsg::GetBluechipUsdPrice {}` | returns stored rate |
| `FactoryExecuteMsg::NotifyThresholdCrossed { pool_id }` | records `MINTED[pool_id] = true`, idempotent |
| `FactoryExecuteMsg::PayDistributionBounty { recipient }` | no-op (auth check only) |
| Harness-only: `SetRate { new_rate, timestamp }` | admin sets the oracle rate |
| Harness-only: `RegisterPool { pool_id, addr }` | so callbacks can authenticate the sender |
| Harness-only: `ThresholdMinted { pool_id }` query | invariant uses this |

This swap keeps the pool's commit/swap/threshold-cross flow under real
contract code while letting fuzz actions move the oracle rate freely
(including to `0` or far-future timestamps).

## Known shortcuts in the current harness

- `expand-economy::RequestExpansion` is not exercised end-to-end — its
  `info.sender == config.factory_address` gate combined with the
  factory-side `Factory{}` denom-validation query would require a
  matching production factory in the harness. The pool→factory_shim
  call records the `MINTED` flag (so the invariant fires); the
  follow-on factory→expand-economy call is replaced by the shim's
  no-op.
- Router multi-hop is wired (`build_world(true)` instantiates it) but
  no successful multi-hop action is currently in the action enum — the
  harness only exercises the unauthorized-internal-call illegal action.
  Add a `RouterMultiHop` variant when you want full coverage.
- The mockoracle is instantiated but the pool reads its rate from the
  factory shim, not the mockoracle. The `AttemptOraclePriceZero`
  action exercises the mockoracle's zero-price rejection independently.

These are explicit scope cuts — see header comments on each module.

## What the harness checks (cross-reference)

Invariants implemented in `fuzz-stateful/src/invariants.rs`:

- `conservation_native_underwater` — pool's bluechip bank balance
  never < `reserve0`.
- `conservation_cw20_underwater` — pool's CW20 balance never <
  `reserve1`.
- `minimum_liquidity_breached` — both reserves zero, or both ≥ 1000.
- `threshold_unsticky` — `IS_THRESHOLD_HIT` once true, never false.
- `usd_raised_decreased` — pre-threshold `USD_RAISED_FROM_COMMIT`
  monotonically non-decreasing.
- `threshold_phase_inconsistent_in_progress` — while `CommitStatus`
  reports `InProgress { raised, target }`, `raised < target` strictly.
  The pool MUST flip to `FullyCommitted` the moment the target is
  reached.
- `threshold_minted_flag_regressed` — factory shim's `MINTED[pool_id]`
  monotonically non-decreasing.
- `swap reduced constant product` — checked inline in the swap
  action handlers (`SwapNativeIn`, `SwapCw20In`): `reserve0 *
  reserve1` after a successful swap is always at least the
  pre-swap value. Panics on regression so proptest captures the
  failing sequence in `proptest-regressions/`.

Authorization invariants are enforced *inline* in `actions.rs::apply`:
the illegal-by-design actions (`AttemptUnauthorizedConfigUpdate`,
`AttemptUnauthorizedThresholdNotify`, `AttemptUnauthorizedRouterInternal`,
`AttemptOraclePriceZero`) panic with `INVARIANT BROKEN` if the contract
incorrectly accepts them.

Pure-math invariants in `proptest_pure_math.rs`:

- `expand_economy_formula` — output bounded, matches reference impl,
  saturating cap honored.
- `swap_math_invariants` — no panic, constant product preserved,
  return + commission ≤ ask reserve.
- `threshold_check_matches_reference` — contract-style USD math equals
  reference, cumulative monotonic.
