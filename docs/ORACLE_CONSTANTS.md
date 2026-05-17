# Oracle Constants: Rationale and Tuning

This document explains the hardcoded oracle constants in the factory and
creator-pool crates: why each value was chosen, what assumptions it
encodes, and what would need to change to retune it.

If a future deployment needs different values, the **only supported
mechanism** is a coordinated `UpgradePools` migration that ships new
constants in code. There is no runtime `Config` plumbing for these
values and no timelock proposal that touches them. Adding tunability
to any of them requires both a `FactoryInstantiate` field AND a bounded
validator in the timelock-proposal path; see "Adding tunability" at the
end.

## Integration-test overrides (`--features integration_short_timing`)

Most of the constants below are cfg-gated so that the docker-built
`-mock.wasm` artifact (which enables both `mock` and
`integration_short_timing`) compiles them down to test-friendly values.
This lets the shell-script integration suite drive the full deploy →
bootstrap → threshold-cross → rotation flow in minutes instead of
days. Production builds (default features, no `integration_short_timing`)
use the values shown in the Quick Reference below; nothing about the
production behaviour is altered.

| Constant | Prod value | `integration_short_timing` value |
|---|---|---|
| `ADMIN_TIMELOCK_SECONDS` | 172_800 s (48 h) | 120 s |
| `CONFIG_TIMELOCK_SECONDS` (expand-economy) | 172_800 s | 120 s |
| `WITHDRAW_TIMELOCK_SECONDS` (expand-economy) | 172_800 s | 120 s |
| `BOOTSTRAP_OBSERVATION_SECONDS` | 3600 s (1 h) | 30 s |
| `MIN_BOOTSTRAP_OBSERVATIONS` | 6 | 2 |
| `COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS` | 3600 s | 30 s |
| `STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS` | 3600 s | 30 s |
| `ROTATION_INTERVAL` | 3600 s | 60 s |
| `ORACLE_REFRESH_RATE_LIMIT_BLOCKS` | 7200 (~12 h) | 1 block |
| `MIN_POOL_LIQUIDITY_USD` | 5_000_000_000 (~$5 k) | 1_000 |
| `MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE` | 5_000_000_000 | 1_000 |
| `ORACLE_BASKET_ENABLED` | `false` | `true` |
| `update_internal_oracle_price` UpdateTooSoon check | enforced | bypassed |
| `update_internal_oracle_price` warmup_remaining | natural drain | cleared every call |

NEVER ship a wasm built with `integration_short_timing` to production —
every gate listed above is deliberately weakened.

## Quick reference

| Constant | Value | Location |
|---|---|---|
| `TWAP_WINDOW` | 3600 s (1 h) | `factory/src/internal_bluechip_price_oracle.rs:110` |
| `UPDATE_INTERVAL` | 300 s (5 min) | `factory/src/internal_bluechip_price_oracle.rs:111` |
| `ROTATION_INTERVAL` | 3600 s (1 h) | `factory/src/internal_bluechip_price_oracle.rs:112` |
| `MAX_TWAP_DRIFT_BPS` | 3000 (30 %) | `factory/src/internal_bluechip_price_oracle.rs:261` |
| `ANCHOR_CHANGE_WARMUP_OBSERVATIONS` | 5 rounds | `factory/src/internal_bluechip_price_oracle.rs:423` |
| `ORACLE_BASKET_ENABLED` | `false` | `factory/src/internal_bluechip_price_oracle.rs:68` |
| `MIN_POOL_LIQUIDITY_USD` | 5_000_000_000 (~$5k) | `factory/src/internal_bluechip_price_oracle.rs:108` |
| `MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE` | 5_000_000_000 ubluechip | `factory/src/internal_bluechip_price_oracle.rs:81` |
| `MAX_PRICE_AGE_SECONDS_BEFORE_STALE` | 300 s | `factory/src/state.rs:93` |
| `MAX_ORACLE_STALENESS_SECONDS` | 360 s | `creator-pool/src/swap_helper.rs:33` |
| `ORACLE_POOL_COUNT` | 75 pools/round | `factory/src/internal_bluechip_price_oracle.rs:37` |

## Update cadence: `UPDATE_INTERVAL = 300s`, `TWAP_WINDOW = 3600s`

`UPDATE_INTERVAL` is the **minimum** gap a keeper must wait before its
next `UpdateOraclePrice {}` call is accepted. `TWAP_WINDOW` is the
trailing observation window the keeper-published prices are averaged
over.

- **5-minute update floor** balances keeper gas cost against price
  responsiveness. Tighter than 5 min would compound keeper costs on a
  block-time chain (e.g. Osmosis ~6 s) without improving the TWAP enough
  to matter; looser than 5 min lets short-window manipulation slip in
  before the breaker can see it.
- **60-minute TWAP** is wide enough that a sophisticated attacker
  cannot single-block-manipulate the average enough to clear the
  `MAX_TWAP_DRIFT_BPS` breaker (the breaker fires on *aggregate-round*
  drift), and narrow enough that genuine 1-hour price moves register
  promptly.

**Retune if** the deployment chain has block times significantly slower
than Osmosis (e.g. >30 s blocks), or if the trading population is so
illiquid that 60-minute TWAPs lag real prices unacceptably.

## TWAP drift breaker: `MAX_TWAP_DRIFT_BPS = 3000`

Maximum allowed drift between consecutive TWAP observations, in basis
points. 3000 bps = 30 % per `UPDATE_INTERVAL`.

- A genuine 30 % move in 5 minutes is a recognizable extreme-volatility
  event for a chain-anchored token, but not impossible (a depeg, a major
  exchange listing, etc.). Tighter caps would trip on real moves and
  freeze the oracle unnecessarily; looser caps would let a manipulation
  attack land before the breaker engages.
- The breaker uses `>` (strict), so exactly 3000 bps is **accepted**.
  This boundary is pinned by `drift_exactly_thirty_percent_yields_3000_bps`
  in `factory/src/testing/audit_tests.rs`.
- Saturating math is used to make overflow fail-closed: a delta so large
  that `diff * BPS_SCALE` would overflow `u128` saturates to `u128::MAX`
  and unconditionally trips the breaker. Pinned by
  `drift_overflow_saturates_to_max` (same file).

**Retune if** the chain's natural volatility regime is materially
different. A LST or stable-anchored deployment might want a tighter cap
(say 1000-1500 bps); a more volatile memecoin/utility-token deployment
might want a looser cap (5000+ bps).

## Anchor-change warmup: `ANCHOR_CHANGE_WARMUP_OBSERVATIONS = 5`

After any anchor reset (one-shot `SetAnchorPool`, timelocked anchor
change inside `ProposeConfigUpdate`, or `ForceRotateOraclePools`), the
oracle refuses to publish a price downstream until this many successful
TWAP rounds have accumulated.

- 5 rounds × 300 s = **~25 minutes** of warm-up before the oracle is
  considered authoritative again.
- Sized to make single-block reserve manipulation at the *moment* of
  anchor change unprofitable: the manipulated first observation must
  stay within `MAX_TWAP_DRIFT_BPS` of the post-manipulation buffered
  candidate across 5 successive rounds before it lands as the canonical
  price. That's roughly 5 × 30 % = 150 % cumulative drift budget, which
  exceeds the cost of sustaining a price across 25 minutes for any
  realistic attacker.
- Warmup is **strict** for commit valuations (`get_bluechip_usd_price`)
  and **best-effort** for fee-priced callers
  (`usd_to_bluechip_best_effort`, with `pre_reset_last_price` fallback).
  Pinned by `test_warmup_strict_vs_best_effort_bifurcation` in
  `factory/src/testing/audit_tests.rs`.

**Retune if** the deployment chain has slow blocks (extend rounds) or
high anchor-rotation frequency (shorten warmup at the cost of
post-rotation security).

## Basket aggregation: `ORACLE_BASKET_ENABLED = false`

Cross-pool basket aggregation is disabled for v1. Each AMM pool's TWAP
yields a `bluechip-per-non-bluechip-side` rate; averaging those rates
across heterogeneous non-bluechip sides (ATOM vs. USDC vs. OSMO vs.
creator token) without per-pool USD normalization produces a number
with no economic interpretation.

**Re-enabling requires:**
1. Each `AllowlistedOraclePool` carries a per-pool Pyth feed id for the
   non-bluechip side.
2. `calculate_weighted_price_with_atom` converts every pool's
   contribution to a USD-per-bluechip estimate via that pool's Pyth feed
   before summing.
3. `last_price` semantics + the downstream consumer in
   `get_bluechip_usd_price_with_meta` align on whichever representation
   the new aggregation produces.

Until those three are wired, the anchor pool is the sole price source.

## Liquidity floor: `MIN_POOL_LIQUIDITY_USD = $5,000` (per side)

USD-denominated floor for total pool liquidity, enforced at oracle
sampling time. The total-USD floor is converted to a bluechip-side
floor (= total/2, since xyk pools have equal-USD sides at the
spot-implied price) and compared against the bluechip-side reserve.

- $5,000 per side ≈ $10,000 total is the minimum where a single-block
  reserve manipulation costs more than a would-be attacker can recover
  from a 30 % TWAP move (capped by `MAX_TWAP_DRIFT_BPS`).
- The fallback constant
  `MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE = 5_000_000_000`
  ubluechip applies when the oracle has no usable USD price (bootstrap,
  tripped breaker, warm-up). At launch parity (1 ubluechip ≈ $0.001),
  that's 5,000 bluechip — the same per-side equivalent as the legacy
  $5k floor under a balanced pool.

**Retune if** the early-ecosystem pool size distribution is materially
different. A deployment seeding $50k+ pools could safely raise the
floor to $25k+ per side; a long-tail deployment with many small pools
should keep $5k as the lower bound.

## Pyth staleness: `MAX_PRICE_AGE_SECONDS_BEFORE_STALE = 300s`

Maximum acceptable age for the Pyth ATOM/USD price. Applies both to
the live query path AND to the cached fallback used when the live query
fails. Matched to `UPDATE_INTERVAL` so a healthy keeper schedule
always covers the staleness window.

**Retune in lockstep with `UPDATE_INTERVAL` only.** Loosening this
independently lets stale Pyth values leak into commits; tightening
without also tightening `UPDATE_INTERVAL` causes routine commit
freezes between keeper rounds.

## Pool-side staleness: `MAX_ORACLE_STALENESS_SECONDS = 360s`

Pool-side acceptance window for the factory oracle's
`ConversionResponse.timestamp`. Sized to `UPDATE_INTERVAL` (300 s) plus
a 60 s grace buffer for keeper scheduling jitter.

- With a strict 90 s window against a 300 s update cadence, ~70 % of
  every 5-minute cycle would reject every commit with "Oracle price is
  stale" even on a fully healthy system. The 60 s grace prevents
  routine keeper jitter from triggering false-positive freezes.
- Boundary semantics (accept at exactly `ts + window`, reject one
  second past) pinned by `oracle_staleness_boundary_tests` in
  `creator-pool/src/testing/audit_regression_tests.rs`.

**Retune in lockstep with `UPDATE_INTERVAL` and
`MAX_PRICE_AGE_SECONDS_BEFORE_STALE` only.** Drift between any of
these three creates dead bands where one check is tight and another
is loose, which manifests as intermittent commit failures.

## Rotation cadence: `ROTATION_INTERVAL = 3600s`,  `ORACLE_POOL_COUNT = 75`

`ROTATION_INTERVAL` controls how often the eligible-pool sample
rotates. `ORACLE_POOL_COUNT` is the sample size per rotation
(plus the anchor pool).

- 75 pools/round at 1 round/hour keeps the per-rotation gas cost
  bounded regardless of how many total creator pools exist, while
  ensuring every eligible pool is sampled at least once per
  `ceil(N/75)` hours.
- These two together are the main "cost scales with pool count"
  knobs; retune if the post-launch pool count grows past ~10k or if
  per-block gas budgets change materially.

## Adding tunability

If a future operator needs a constant exposed via the
`expand-economy` timelock proposal path, the work is:

1. Add a field to `FactoryInstantiate` (`factory/src/state.rs`) carrying
   the value. Migration: existing factories load the hardcoded constant
   as a default during migrate.
2. Add a corresponding field to `PendingConfig` and the
   `ProposeConfigUpdate` validator. **The validator MUST clamp the
   value to a chain-safe range** — see the
   `emergency_withdraw_delay_seconds` precedent at
   `factory/src/state.rs:456-478` for the bounded-range pattern (60 s –
   7 d, validated at proposal time).
3. Add a 48-hour timelock entry following the existing
   `standard_pool_creation_fee_usd` plumbing.
4. Replace the `const` with a `.load()` of the new field in every
   call site (compile-time error if a site is missed).

Bounded ranges are non-negotiable: an attacker who gets to propose
"`MAX_TWAP_DRIFT_BPS = 10_000_000`" through governance has effectively
disabled the breaker even if the 48 h timelock lets others see the
proposal coming. Always pair the tunability with a validator that
keeps the value inside the safe regime documented above.
