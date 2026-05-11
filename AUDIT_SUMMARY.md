# Security Audit Summary — `claude/security-logic-audit-ChIIX`

Single consolidated record of every audit finding from Sections 1 through 7: what was changed, why, what was deliberately not changed, and the trust boundaries that remain.

The audit was structured as seven scope-aligned sections. Each finding carries a severity (Critical / High / Medium / Low / Info) and an explicit disposition (Fixed / Deferred / No Action).

---

## Branch state

Five commits land on `claude/security-logic-audit-ChIIX`. In chronological order:

| Commit    | Section | Audit codes              | Crate(s)             |
|-----------|---------|--------------------------|----------------------|
| `cc984fd` | 1       | M-1, M-2                 | factory              |
| `2a24c78` | 1       | H-1, L-2, L-4            | factory              |
| `6b21785` | 2       | C-1, H-2, L-5, L-6, M-5  | factory              |
| `2616c32` | 3       | L-8                      | expand-economy       |
| `4e8cdfd` | 5/6     | M-7                      | pool-core + pools    |

Test posture at end of audit:
- factory: 213 / 213 passing
- expand-economy: 34 / 34 passing
- pool-core: 25 / 25 passing
- creator-pool: 203 / 206 passing (3 pre-existing emergency_withdraw fixture failures unrelated to the audit)
- standard-pool: 59 / 65 passing (6 pre-existing emergency_withdraw fixture failures unrelated to the audit)

The 9 pre-existing test failures all stem from a missing `factory_addr` / `factory_contract` mock in emergency-withdraw fixtures, predate this branch, and were verified independent of audit work via `git stash` baselines.

---

## Section 1 — factory plumbing

### H-1 (High): anchor-promotion race in pool upgrade timelock — **Fixed** in `2a24c78`

**Problem.** `build_upgrade_batch` froze the `pools_to_upgrade` list at propose time. If a `ProposeConfigUpdate` promoted a standard pool to the anchor between propose and apply of a pool upgrade that already included it, the migrate would fire against the live anchor, breaking the oracle's mid-flight assumption that the anchor wasn't being migrated.

**Fix.** Re-resolve the live anchor at apply / continue time; hard-fail if any pool in the frozen list matches. Forces a Cancel + re-Propose flow that excludes the anchor, which is the correct operator behaviour.

### L-2 (Low): O(N) pool-address lookup — **Fixed** in `2a24c78`

**Problem.** `lookup_pool_by_addr` did a linear scan over `POOLS_BY_ID` on every threshold-cross, distribution bounty, and admin-by-address call.

**Fix.** New `POOL_ID_BY_ADDRESS` reverse-index `Map<Addr, u64>`, populated by `state::register_pool` alongside the other three pool maps. Lookup is now two O(1) loads on the fast path. A fallback linear scan is preserved so test fixtures that bypass `register_pool` continue to resolve, with a doc comment flagging that path as a bug-finder for any production code that bypasses `register_pool`. `migrate.rs` back-fills the index for existing pools.

### L-4 (Low): O(N) rate-limit prune — **Fixed** in `2a24c78`

**Problem.** `PruneRateLimits` iterated the alphabetic primary index, doing O(N) work to find O(stale_count) entries to delete.

**Fix.** New `(timestamp, Addr)` secondary index for each rate-limit primary (`COMMIT_POOL_CREATE_TS_INDEX`, `STANDARD_POOL_CREATE_TS_INDEX`). Prune iterates the secondary index in ascending timestamp order with an early-exit on the first non-stale entry. Both maps are updated in the same tx as the primary, so they revert together on failure and cannot drift.

### M-1 (Medium): late-failing PoolConfigUpdate — **Fixed** in `cc984fd`

**Problem.** `PoolConfigUpdate` validation only ran on the pool's `apply` side. An out-of-range `lp_fee` or `min_commit_interval` would sail through the 48h timelock and reject at apply, forcing operators into a Cancel + re-Propose + another 48h wait.

**Fix.** `PoolConfigUpdate::validate()` mirrors the pool-core bounds (lp_fee 0.1–10%, min_commit_interval ≤ 86400s) and runs at both propose and apply. Operator typos now surface at propose, immediately.

### M-2 (Medium): silent base-amount inflation on zero commit_pool_ordinal — **Fixed** in `cc984fd`

**Problem.** `calculate_mint_amount(s, 0)` returns the full base amount (500_000_000) — the worst possible direction if a corrupted pool record reached this code path.

**Fix.** `calculate_and_mint_bluechip` now hard-rejects `commit_pool_ordinal == 0` with an explicit error pointing operators at the registry. v1 has no legacy data so this branch should never fire under any honest path; if it ever does, an operator must investigate before retrying.

---

## Section 2 — internal oracle

### C-1 (Critical): cross-pool basket aggregation is unit-unsafe — **Fixed** in `6b21785`

**Problem.** Each AMM pool's TWAP yields a raw `bluechip-per-non-bluechip-side` rate. Averaging across pools with heterogeneous non-bluechip sides (ATOM vs USDC vs OSMO vs creator token) without first normalizing each pool to a shared unit produces a result with no economic interpretation. The downstream consumer at `get_bluechip_usd_price_with_meta` reads `last_price` as strictly `bluechip-per-ATOM`, so the only safe aggregation today is "anchor only."

**Fix.** New `ORACLE_BASKET_ENABLED: bool = false` gate. `select_random_pools_with_atom` short-circuits to return `[anchor]` only. The basket aggregation code is preserved for the future per-pool-Pyth milestone; flipping the gate to `true` requires (1) per-pool Pyth feed id on `AllowlistedOraclePool`, (2) `calculate_weighted_price_with_atom` doing per-pool USD normalization before summing, (3) aligning `last_price` semantics with the basket result. Documented in code at the gate.

**Why this is "anchor-only" rather than "rip out basket code":** keeping the basket scaffolding lets the future per-pool-Pyth milestone re-enable cross-pool aggregation by flipping a single const, after the three milestones above land. Ripping it out would force a re-implementation later.

### H-2 (High): stale bootstrap candidate survives rotation — **Fixed** in `6b21785`

**Problem.** Branch (d) of `update_internal_oracle_price` fires only when both `last_price` and `pre_reset_last_price` are zero — i.e., before the very first `ConfirmBootstrapPrice` has ever published. If admin force-rotated or changed the anchor in that window, the next round re-entered branch (d) and found a stale `PENDING_BOOTSTRAP_PRICE` candidate with its old `proposed_at`. Admin could `ConfirmBootstrapPrice` immediately without the 1h observation window re-elapsing against the post-rotation pool sample.

**Fix.** Both `execute_force_rotate_pools` and the anchor-change refresh helper now clear `PENDING_BOOTSTRAP_PRICE`. The post-reset warm-up starts fresh.

### L-5 (Low): Pyth staleness window too tight — **Fixed** in `6b21785`

**Problem.** `MAX_PRICE_AGE_SECONDS_BEFORE_STALE = 90s` was inside typical Pyth publish cadence but caused commits to freeze across any brief publisher hiccup.

**Fix.** Widened to 300s. Gives Pyth headroom for short outages without making the staleness window so wide that stale-but-acceptable becomes useful for time-window manipulation. Applied to live read, cache fallback, and best-effort warm-up.

### L-6 (Low): keeper bounty starves during warm-up — **Fixed** in `6b21785`

**Problem.** Oracle-update keeper bounty used strict `usd_to_bluechip` conversion. During the post-reset warm-up window (~30 min), that path errored and keepers received no compensation while the oracle was still settling — discouraging exactly the keeper activity needed to complete warm-up.

**Fix.** Switched to `usd_to_bluechip_best_effort`. Bounty cap ($0.10) and the 30% TWAP circuit breaker bound the worst-case mispricing to ~$0.03/call. Keepers stay paid; oracle settles faster.

### M-5 (Medium): O(N) eligible-pool scan blew gas limit — **Fixed** in `6b21785`

**Problem.** `get_eligible_creator_pools` iterated every entry in `POOLS_BY_ID` and issued a cross-contract `GetPoolState` query per candidate. At any meaningful pool count, the eligible-pool snapshot refresh exceeded block gas, bricking oracle updates whenever rotation interval coincided with snapshot staleness.

**Fix.** Random-pull-with-reject. Pick a random pool id in `[1, POOL_COUNTER]`, validate (kind == Commit, threshold-minted, bluechip side, liquidity floor), accept on success or toss and re-pick. Capped at 4× the target sample size so a registry dominated by drained or pre-threshold pools cannot brick the refresh by exhausting the loop. Allowlist source (admin-curated, bounded) still does a full scan.

**Why random sampling vs. paginated walk:** sample composition rotates across refreshes (block-time-seeded), so over time eligible pools get fair inclusion. In any single round the snapshot's purpose is sampling, not exhaustive enumeration, so the slight bias is acceptable. A paginated walk would either need cursor state (more storage) or repeat-visit logic.

**Test churn.** Three test helpers were writing directly to `POOLS_BY_ID` without bumping `POOL_COUNTER` or populating `POOL_ID_BY_ADDRESS`. With the new sampler reading `POOL_COUNTER`, that test path returned zero matches. Helpers updated to mirror what production `state::register_pool` writes (this also flushed out the L-2 inconsistency in tests).

---

## Section 3 — expand-economy

### L-8 (Low): pre-existing doctest failure — **Fixed** in `2616c32`

**Problem.** A pseudocode block in `helpers.rs:78` used four-space indentation, which rustdoc interprets as a Rust code block and tries to compile.

**Fix.** Changed to a fenced `text` code block so rustdoc skips compilation.

### M-6 (Medium → Low): rate-limit error breaks legitimate concurrent crosses — **No Action**

**Problem.** `enforce_recipient_rate_limit` errors with `RecipientRateLimited` when the same recipient received an expansion in the last 60s. This is symmetric with the insufficient-balance check at phase 7 — both are transient unavailability conditions — but phase 7 returns a skip response while phase 5 reverts the calling tx. If two creator pools owned by the same user crossed threshold within 60s of each other, the second `NotifyThresholdCrossed` errors.

**Disposition.** Severity downgraded to Low in Section 4 once we confirmed the pool's `factory_notify` is dispatched as `SubMsg::reply_on_error` (`creator-pool/src/commit/threshold_payout.rs:121`). A rate-limit error propagates to the pool's reply handler, which sets `PENDING_FACTORY_NOTIFY=true` and lets the threshold-cross tx succeed. `RetryFactoryNotify` later catches the deferred bluechip mint. User commits never break. Worst-case symptom: a spurious-looking error on operator dashboards and a pending retry. Not worth the test churn to ship as a code change.

### L-7 (Low): cross-validation query before zero-amount short-circuit — **No Action**

**Problem.** `cross_validate_factory_denom` runs one cross-contract `query_wasm_smart` per `RequestExpansion`. After the bluechip mint decay polynomial reaches zero, every threshold-cross still burns this query AND requires the factory to be live for the dormant call to succeed.

**Disposition.** User declined the fix. Cost is negligible per the existing doc; dormant calls are rare. Author's call.

### OP-1, OP-2 (Info): operational notes — **No Action**

- **OP-1**: denom changes need coordinated propose/apply on both factory and expand-economy or `BluechipDenomMismatch` breaks all threshold crosses. Operational runbook, not a code fix.
- **OP-2**: `LAST_EXPANSION_AT_RECIPIENT` grows monotonically. CosmWasm storage is cheap; stale 60s-old timestamps don't change behaviour (`now >= next_allowed` is trivially true). Acceptable.

---

## Section 4 — pool creator commit

### No new code changes

The commit flow (pre-threshold deposit, threshold-crossing split, post-threshold AMM swap, distribution batch loop) was reviewed in full and found clean. Substantive defenses already in place:

- Reentrancy guard wrapping the entry, shared `REENTRANCY_LOCK` across commit/swap/deposit/add/remove/collect.
- Per-user `min_commit_interval` rate limit (default 13s).
- `MIN_COMMIT_USD_PRE_THRESHOLD = $5`, `MIN_COMMIT_USD_POST_THRESHOLD = $1`.
- Oracle rate captured at entry and threaded through threshold-crossing for inverse-conversion consistency — no mid-tx drift.
- Pool-side staleness check at 360s (factory's 300s `UPDATE_INTERVAL` + 60s grace).
- `must_pay` + `sent == amount` exact-funds enforcement; multi-denom / wrong-denom funds rejected.
- Pro-rata fee allocation across threshold portion + excess.
- **Excess swap capped at 3%** of seeded reserve (was 20% — MEV mitigation, prior audit fix).
- **Default 5% max_spread** on excess swap if user didn't specify (was unbounded, prior audit fix).
- Dust-swap guard: rejects if `capped_excess > 0` but `return_amt == 0`.
- `THRESHOLD_PROCESSING` flag prevents stuck-state silent downgrade to pre/post-threshold path.
- `POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK = block.height + 2 + 1` blocks crossing-block followers + next 2 blocks (~18s) from sandwiching the freshly-seeded pool. Crosser's own bounded excess swap runs before the cooldown is observable, so it's unaffected.
- `NATIVE_RAISED_FROM_COMMIT` stores net-of-fees → exact `pools_bluechip_seed = NATIVE_RAISED` math in `trigger_threshold_payout`, no recovery floor mismatch.
- Distribution batches use per-mint `SubMsg::reply_always` — single failing recipient lands in `FAILED_MINTS`, doesn't revert the batch (H-6 audit fix).
- Distribution bounty emitted only when `processed_count > 0` — no farming empty calls.
- Distribution termination driven by ledger emptiness (source of truth), not the `distributions_remaining` counter.
- Floor-division dust residual minted to creator on final batch, gated on `distributed_so_far > 0` so legacy in-progress distributions don't double-mint.

---

## Section 5 — pool swap

### M-7 (Medium): CW20 Receive-hook trusted `cw20_msg.amount` — **Fixed** in `4e8cdfd`

**Problem.** When a CW20 dispatches `Receive` to the pool, the standard flow is: (1) deduct from sender, (2) credit pool, (3) dispatch Receive. The pool's `execute_swap_cw20` credited `cw20_msg.amount` to the offer-side reserve without verifying step 2 actually happened. Standard pools accept arbitrary user-supplied CW20 contracts (no whitelist on `execute_create_standard_pool` — confirmed during audit), so a creator could deploy a CW20 whose transfer hook fabricates `amount` and drains the opposite reserve at AMM rates with no inbound deposit.

**Fix.** Before delegating to `simple_swap`, query the pool's actual CW20 balance via `query_token_balance_strict` and compare to the pre-Receive invariant:

```
balance >= reserve_X + fee_reserve_X + creator_pot.amount_X + cw20_msg.amount
```

Shortfall → new `Cw20SwapBalanceMismatch` error variant. The check uses `<` (not `!=`) so unsolicited donations to the pool don't block legitimate swaps; the surplus is benign orphan that doesn't enable an exploit (an attacker swapping their own donation gets back exactly the AMM rate, no profit).

**Creator pools benefit defensively.** Their CW20 is auto-minted by the pool itself (no malicious admin), so the check is a no-op in practice. But folding it in at the shared entry point matches the posture already established for `execute_deposit_liquidity_with_verify` and `execute_add_to_position_with_verify` — uniform defense, no future regression vector.

**Why synchronous rather than SubMsg+reply.** The classic SubMsg-balance-verify pattern used by deposits snapshots BEFORE the transfer message dispatches. In the Receive-hook case the transfer has already happened by the time our handler runs, so there's no pre-snapshot to take. We reason about the invariant instead: if the pool's prior balance equalled `reserve + fee_reserve + creator_pot`, then post-Receive it must be that plus `cw20_msg.amount`. The `<` (not `!=`) tolerance is what makes this safe in the presence of donations.

**Test updates.** Four existing creator-pool swap tests had CW20 balance mocks that worked only by coincidence — the mock filter (`Binary::to_string().contains("balance")`) never matched because `Binary::to_string()` returns base64. The tests passed before only because no CW20 balance query was issued. Mocks rewritten to return faithful post-Receive balances. One new positive test (`test_cw20_receive_rejects_balance_shortfall`) explicitly exercises the reject path with a hostile-CW20 mock. One standard-pool test (`cw20_hook_swap_dispatches_via_receive`) also got a per-test mock override.

### OQ-3 (Open Question → resolved): standard-pool CW20 whitelist policy

Confirmed during audit: `execute_create_standard_pool` is permissionless and admits any CW20 that responds to `Cw20QueryMsg::TokenInfo`. No whitelist. This is what made M-7 a real concern and made the fix necessary.

---

## Section 6 — pool liquidity

### No new code changes

The liquidity flow (deposit, add-to-position, remove-partial, remove-all, collect-fees) was reviewed in full and found clean:

- **SubMsg balance-verify on deposits + add-to-position** (both pool kinds via `*_with_verify` variants). Reply-handler delta check rejects shortfalls AND inflation, atomically reverting the tx. H-1 audit fix already shipped.
- **First-deposit `MINIMUM_LIQUIDITY = 1000` lock** via `Position.locked_liquidity` — fee accrual on full position, withdrawal blocked on the locked slice.
- **`fee_size_multiplier` clipping**: linear scaling 10–100% based on `liquidity / OPTIMAL_LIQUIDITY`. Clipped portion routes to `CREATOR_FEE_POT` (creator-claimable), not to other LPs.
- **Position NFT stays alive on full remove** (Uniswap V3 empty-position model). NFT remains tradeable and rehydratable via `AddToPosition`. H-NFT-1 audit fix.
- **Three layers of slippage protection** on liquidity ops: `check_slippage` (min amounts), `check_ratio_deviation` (max bps), `assert_max_spread` (on the rebalancing swap).
- **Auto-pause recovery** via deposit when reserves restore above MIN.
- **`EmergencyPending` permits LP exits + CollectFees** but rejects deposits (HIGH-1 audit fix). Lets LPs race the 24h drain timelock; blocks fresh capital from being funneled into a pending drain.
- **Position transfer** lazy-syncs `position.owner` and `OWNER_POSITIONS` index on next interaction.
- **`prepare_deposit` rejects extra-denom funds** so accidental attachments don't orphan.
- **Reentrancy guard** shared across all liquidity-touching paths.

---

## Section 7 — pool threshold crossing + fees

### No new code changes; three carryover items resolved.

### OQ-2 (Open Question → closed): factory validates `commit_threshold_limit_usd > 0`

Closed in `factory/src/execute/config.rs:95-99`. `validate_factory_config` runs at instantiate, propose, AND apply. Zero is rejected with an explicit error. The field is a factory-global config — pool creators can't supply a per-pool override — so the degenerate-state issue flagged earlier ("first commit immediately crosses; all commit_return goes to creator via residual") is unreachable.

### L-3 (Open Question → closed): `nft_ownership_accepted` semantics

Closed. Both pool kinds initialize and flip the flag correctly, with idempotent paths:

- **Standard pool:** factory's `finalize_standard_pool` reply chain dispatches `TransferOwnership` to the NFT and a follow-up `AcceptNftOwnership` execute on the pool in the same tx, closing the pending-ownership window. The pool's `execute_accept_nft_ownership` is factory-only, no-op if flag already set.
- **Creator pool:** lazy-set at first deposit OR at threshold-crossing inside `trigger_threshold_payout`. Both paths idempotent.
- **Drift case** (flag false but NFT already accepted): re-emit of `AcceptOwnership` is rejected by the NFT contract with `NoPendingOwner` → entire tx reverts. Fail-loud, correct behaviour.

### M-3 (Medium → Low): distribution bounty recipient trusts pool wasm — **No Action**

**Problem.** `creator-pool/src/commit/distribution.rs:88-100` passes `info.sender.to_string()` to the factory; the factory at `oracle.rs:217` `addr_validate`s and pays. An honest pool forwards the keeper who called `ContinueDistribution`; a compromised pool wasm could redirect to any wallet.

**Disposition.** Documented trust boundary. Pool wasm upgrades go through `UpgradePools` with a 48h timelock + governance window. Same trust assumption as the entire protocol. Factory-side recipient verification options were considered (querying pool's `LAST_CONTINUE_DISTRIBUTION_AT` for the named recipient; passing keeper+stamp and verifying) — all add complexity without changing the admin-trust ceiling. No fix.

### L-11 (Low, new): no factory-side rate limit on `PayDistributionBounty` — **No Action**

**Problem.** Any registered commit pool can call `PayDistributionBounty` an arbitrary number of times per block. Bounded by block gas (~6 calls/block per pool) and factory bluechip balance. An honest pool emits this only from `execute_continue_distribution` with a 5s keeper rate limit; a compromised pool wasm could spam.

**Disposition.** Threat requires admin-key compromise + the 48h `UpgradePools` timelock + factory bluechip reserve to materially drain. The 48h timelock is the primary mitigation. A factory-side per-pool rate limit (e.g. 5s) was evaluated and rejected because legitimate multi-keeper races on the same pool — each keeper has its own per-address `LAST_CONTINUE_DISTRIBUTION_AT` slot on the pool side, so two keepers can both call within 5s — would see legitimate factory calls rejected. The drain-rate reduction (25× per-pool) doesn't justify breaking the multi-keeper UX for a threat already gated by governance.

### Reviewed and clean (no findings)

- `execute_notify_threshold_crossed`: caller equality with `pool_details.creator_pool_addr`; rejects `PoolKind::Standard`; `POOL_THRESHOLD_MINTED` flag set BEFORE the mint dispatch.
- `calculate_and_mint_bluechip`: uses `commit_pool_ordinal` (not the global counter) so permissionless standard-pool creation can't inflate `x`. Fails loud on `commit_pool_ordinal == 0` (M-2 fix). `MAX_DECAY_X = 1_000_000_000` defense-in-depth against absurd ordinals.
- `trigger_threshold_payout`: instantiate-time + runtime double-validation of the 4-way 1.2T split.
- Factory notify via `SubMsg::reply_on_error` — factory errors don't revert the threshold-cross; `RetryFactoryNotify` is permissionless, idempotent, gated on `PENDING_FACTORY_NOTIFY`.
- Both bounty knobs hard-capped at `MAX_*_BOUNTY_USD = $0.10` — admin can't raise without a factory wasm upgrade.
- Pool creation reply chains (both commit and standard) use `SubMsg::reply_on_success` everywhere — failures atomically revert the entire chain. `register_pool` writes the three registry maps atomically (L-2 fix).
- Pool creation fees: USD-denominated, oracle-converted via best-effort. Pre-anchor bootstrap fallback only when `INITIAL_ANCHOR_SET == false` (HIGH-3 audit fix). Post-anchor oracle unavailability refuses rather than misprices.

---

## Trust boundaries and acknowledged limitations

These are NOT bugs. They are design choices with explicit rationale, captured here so future reviewers understand what the protocol assumes.

| Boundary                                                | Mitigation                                                   |
|---------------------------------------------------------|--------------------------------------------------------------|
| Factory admin key                                       | 48h timelock on every governance-mutating action             |
| Pool wasm upgrade                                       | 48h `UpgradePools` timelock + governance window               |
| Distribution bounty recipient (M-3)                     | Pool wasm honesty → admin trust → 48h timelock                |
| Factory bluechip reserve drain via compromised pool (L-11) | 48h timelock + admin-capped per-call bounty ($0.10)         |
| Denom drift between factory + expand-economy (OP-1)     | Operator runbook: coordinated propose/apply                  |
| Cross-pool oracle aggregation off (C-1)                 | Anchor-only until per-pool Pyth feeds land — `ORACLE_BASKET_ENABLED = false` |
| Pyth feed correctness                                   | Confidence-interval gate (default 200 bps); cache-fallback   |
| Position NFT contract (factory-deployed)                | Factory-controlled at instantiate                            |
| CW20 in standard pools (user-supplied)                  | M-7 balance verify on Receive; H-1 verify on deposit/add     |

---

## What's NOT in this audit

Out of scope for the seven sections walked:
- The CW20 base contract itself (factory deploys a fixed `cw20-base` build for creator pools — relying on its widely-audited correctness).
- The CW721 position-NFT contract (factory-deployed, similar trust posture).
- Off-chain keeper / indexer correctness.
- Pyth's own feed correctness (treated as oracle input; the contract enforces confidence/staleness gates on its output).
- Cosmos-SDK module assumptions (bank, wasm, ibc).
- Governance / on-chain admin key custody.

---

## Future work (carried forward, not blocking)

- **Re-enable cross-pool oracle aggregation (C-1 epilogue):** wire per-pool Pyth feed id onto `AllowlistedOraclePool`, update `calculate_weighted_price_with_atom` to normalize per-pool USD contributions before summing, align `last_price` semantics + downstream consumer at `get_bluechip_usd_price_with_meta`. Then flip `ORACLE_BASKET_ENABLED = true`.
- **M-6 rate-limit-as-skip (Low):** convert `RecipientRateLimited` from error to skip response so operator dashboards stop logging the spurious-looking concurrent-cross errors. Not security-relevant (the pool's `reply_on_error` already catches them); UX cleanup.

---

## Commit-by-commit changelog

```
cc984fd  factory: validate PoolConfigUpdate at propose time; fail loud on zero commit_pool_ordinal
2a24c78  factory: anchor-exclusion at upgrade apply, O(1) pool-addr lookup, timestamp-indexed prune
6b21785  factory: anchor-only oracle (C-1), clear bootstrap candidate on rotation (H-2), 300s
         Pyth staleness (L-5), best-effort bounty (L-6), random-sample auto-eligible pools (M-5)
2616c32  expand-economy: fix L-8 doctest by fencing helpers.rs pseudocode block as text
4e8cdfd  pool-core: M-7 fix — synchronous CW20 balance verify on Receive-hook swap
```
