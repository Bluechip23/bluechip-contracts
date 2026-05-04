# Fuzz harness findings — setup + expansion run

## TL;DR

After two passes (initial setup + an expansion that more than doubled
the action enum and added new invariants), no invariant violations
remain across:

| Harness | Cases | Wall-clock | Result |
|---|---|---|---|
| `fuzz_stateful` (stateful proptest, 30-action sequences, 20-action enum) | 1024 | 2.41s | clean |
| `fuzz_stateful_quick` (stateful proptest, 5–15 action sequences) | 256 | 0.81s | clean |
| `proptest_pure_math::expand_economy_formula` | 8192 | <1s | clean |
| `proptest_pure_math::swap_math_invariants` | 8192 | <1s | clean |
| `proptest_pure_math::threshold_check_matches_reference` | 8192 | <1s | clean |
| `smoke_create_commit_swap` | 1 | <1s | clean |
| `smoke_emergency_withdraw_lifecycle` (initiate → cancel → re-initiate → 24h timelock → drain → drain-blocks-ops invariant) | 1 | <1s | clean |

> Note on cargo-fuzz: the `fuzz/fuzz_targets/*` libFuzzer harnesses are
> written and compile-ready, but the sandbox running this initial setup
> doesn't have a nightly toolchain or `cargo-fuzz` installed, so the
> 5-minute libFuzzer runs called for in the spec couldn't be executed
> here. The proptest mirror in `proptest_pure_math.rs` ran 24,576 cases
> total across the same three property checks; same invariants, stable
> toolchain, no findings.

## What was actually exercised

A typical 30-action sequence with `FUZZ_DEBUG=1` looks like:

```
[fuzz] sequence done: 11 steps, ok=2, rejected=4, expected_err=5
[fuzz] sequence done: 8 steps, ok=1, rejected=5, expected_err=2
[fuzz] sequence done: 13 steps, ok=4, rejected=5, expected_err=4
[fuzz] sequence done: 12 steps, ok=3, rejected=6, expected_err=3
```

Action mix per sequence (averaged across the 256-case quick run):
- ~25% successful state mutations (CreateCreatorPool, Commit, Swap, AdvanceBlock, UpdateOraclePrice rate updates)
- ~50% rejected by contract validation (insufficient balance, rate-limit, threshold gating, etc.) — these are *expected* contract behavior, not bugs
- ~25% illegal-by-design actions that correctly errored (`Attempt*` variants)

The five illegal-by-design actions all consistently error:
- `AttemptUnauthorizedConfigUpdate` → `Unauthorized`
- `AttemptUnauthorizedThresholdNotify` → `unauthorized notify` from factory shim
- `AttemptUnauthorizedRouterInternal` → router internal-only auth check
- `AttemptOraclePriceZero` → mockoracle "price must be > 0"
- `UpdateOraclePrice { new_rate: 0 }` → factory shim "rate must be > 0"

## Action enum after expansion

The action enum now covers (added in the expansion pass marked **NEW**):
- `CreateCreatorPool`, `CreateStandardPool`
- `Commit`, `SwapNativeIn`, `SwapCw20In`
- `DepositLiquidity`, `RemoveLiquidityPercent`
- `UpdateOraclePrice`, `AdvanceBlock`
- `AttemptUnauthorizedConfigUpdate` (illegal)
- `AttemptUnauthorizedThresholdNotify` (illegal)
- `AttemptUnauthorizedRouterInternal` (illegal)
- `AttemptOraclePriceZero` (illegal)
- **NEW** `EmergencyInitiate`, `EmergencyCancel`, `EmergencyExecute`
  — full 24h-timelocked emergency-withdraw lifecycle from factory.
- **NEW** `ContinueDistribution` — permissionless keeper batch advance.
- **NEW** `ClaimCreatorFees`, `ClaimCreatorExcess` — creator-only.
- **NEW** `RouterSingleHop` — native→CW20 swap through the router with
  `minimum_receive` slippage assertion.
- **NEW** `AttemptUnauthorizedEmergency`, `AttemptUnauthorizedCreatorClaim`
  (illegal — non-factory / non-creator paths).

Invariants checked after each action:
- `conservation_native_underwater`, `conservation_cw20_underwater` —
  pool's bank/cw20 balance always ≥ its claimed reserves.
- `minimum_liquidity_breached` — both reserves zero or both ≥ 1000.
- `threshold_unsticky` — `IS_THRESHOLD_HIT` once true, never false.
- `usd_raised_decreased` — pre-threshold USD raised non-decreasing.
- `threshold_minted_flag_regressed` — factory shim's `MINTED[pool_id]`
  monotonically non-decreasing.
- **NEW** `positions_overclaim_liquidity` — sum of every active LP
  position's liquidity never exceeds `pool_state.total_liquidity`.
- **NEW** `drained_pool_accepted_swap` /
  `drained_pool_accepted_cw20_swap` — once we observe a successful
  emergency drain, both bluechip and CW20 swap probes against the
  pool must error.

## Notable false positive caught and corrected

While building the expansion's `total_liquidity_mismatch` invariant
the smoke test fired with `sum(positions)=0` vs `total_liquidity=38.6B`
on a freshly-threshold-crossed pool. Investigation showed this is
**intended behavior**: the threshold-crossing seed liquidity (the
`pool_seed_amount = 350B` creator tokens + matched bluechip) is
permanently locked into pool reserves and is intentionally NOT
recorded as a `Position`. No LP can claim it; only the trading
spread captures it.

Tightening the invariant to equality would have been wrong. The
correct shape is **one-sided**: positions can never collectively
claim more liquidity than `total_liquidity`. The reverse —
`total_liquidity > sum(positions)` — is allowed because of the
locked seed. This is now `positions_overclaim_liquidity` in
`invariants.rs`.

This is a useful illustration of the auditor-vs-fuzzer distinction
called out in the README: the fuzzer caught a discrepancy I'd
encoded as an invariant, but only an auditor (or careful read of the
threshold-crossing handler) could correctly judge that the
discrepancy was by-design rather than a bug.

## Issues found and fixed during harness construction (not contract bugs)

These were friction points hit while wiring up the harness and resolved
in this same branch. They're not security issues — just integration
friction:

1. **cw20-base symbol regex** — `cw20-base` validates instantiated CW20
   symbol against `[a-zA-Z\-]{3,12}`. Initial `format!("CT{pool_id}")`
   produced symbols like `CT1` which both fail the length check (n=3
   passes, but the digit fails the charset). Fixed by mapping pool_id
   digits to letters in `world::short_ticker`.
2. **CW20 cap underflow** — initial design minted 5 × 10^18 raw units to
   each of 5 users while setting the CW20 mint cap to 2 × 10^15. The
   cw20-base instantiate refuses to accept initial balances exceeding
   the cap. Fixed by lowering `INITIAL_CW20_PER_USER` to 10^14 and
   removing the cap entirely.
3. **Pool oracle query wrapper** — the pool's `get_oracle_conversion_with_staleness`
   wraps every factory query under a `FactoryQueryWrapper::InternalBlueChipOracleQuery(...)`
   variant defined privately in `creator_pool::swap_helper`. This wire
   format isn't part of the public `pool_factory_interfaces::FactoryQueryMsg`
   that the production factory accepts directly — the factory's
   `query.rs` has its own `InternalBlueChipOracleQuery` variant with
   the same name in its `QueryMsg`. Production's two contracts agree
   on this implicitly via matching enum tags. Documented this in the
   factory_shim and added support for the wrapper variant.
4. **CW20 minter rotation at pool creation** — the threshold-crossing
   commit handler mints 1.2T creator tokens (creator/bluechip/pool-seed/
   commit-return). The pool itself sends those `Cw20ExecuteMsg::Mint`
   calls, so the CW20's minter must equal the pool address. Production
   factory does this via the staged-instantiate reply chain (CW20
   instantiated with factory as minter, then minter rotated to the
   pool in a later reply). The harness now explicitly mirrors that
   rotation: after instantiating the pool, it sends
   `cw20_base::ExecuteMsg::UpdateMinter { new_minter: Some(pool) }`
   from the factory_shim. Without this, the very first
   threshold-crossing commit fails with `Unauthorized` from the CW20.

None of the above represent contract security bugs. (3) is potentially
a maintainability concern — the wire-format coupling between two
private enums depends on serde's case-conversion of identical
identifiers — but it's well-known cosmwasm style and out of scope for a
fuzz finding.

## What did NOT trip — and why that's not nothing

The contracts already passed an audit (see `audit_tests.rs` and
`audit_regression_tests.rs` in each pool crate); the invariants we're
fuzzing largely re-check what those regression tests guarantee on
specific inputs. The fuzz harness adds confidence by:

- Driving **arbitrary action interleavings** rather than fixed
  scripts. The proptest-derived `Action` generator covers ordering
  permutations the audit tests don't.
- Checking invariants **after every action** rather than at the end of
  a fixed scenario.
- Sampling **decimal stress** (6/8/18 dec creator tokens) where the
  audit tests pin to 6.

Things the harness doesn't currently exercise (documented in
`FUZZING.md`):
- expand-economy `RequestExpansion` end-to-end (factory shim only
  records the notification flag).
- Router multi-hop happy paths (only the unauthorized-internal-call is
  in the action enum).
- `EmergencyWithdraw` lifecycle (no `Action` variant).
- `ContinueDistribution` / post-threshold distribution (no variant).
- `ClaimCreatorExcessLiquidity` / `ClaimCreatorFees`.

These are the next obvious actions to add; their absence is a coverage
gap, not an invariant gap.

## Replay & long-run instructions

```sh
# Reproduce this report's results:
PROPTEST_CASES=1024 cargo test -p fuzz-stateful --release fuzz_stateful
cargo test -p fuzz-stateful --release --test proptest_pure_math

# Overnight run:
PROPTEST_CASES=65536 cargo test -p fuzz-stateful --release fuzz_stateful

# cargo-fuzz (requires nightly + `cargo install cargo-fuzz`):
cd fuzz
cargo +nightly fuzz run fuzz_expand_economy_formula -- -max_total_time=300
cargo +nightly fuzz run fuzz_swap_math               -- -max_total_time=300
cargo +nightly fuzz run fuzz_threshold_check         -- -max_total_time=300
```
