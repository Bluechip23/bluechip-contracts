# bluechip-contracts

A decentralized subscription and creator economy protocol built on Cosmos SDK using CosmWasm smart contracts.

## Overview

Bluechip is a DeFi protocol that enables content creators to launch their own tokens and build portable, decentralized subscription communities. Unlike traditional subscription platforms where audiences are locked to a single platform, Bluechip allows creators to take their community anywhere while subscribers earn tokens proportional to their support.

### Key Advantages

**Decentralized Subscriptions**
- Subscription transactions (carried out onchain as commit transactions) are recorded onchain, not controlled by any central platform
- Creators own their subscriber relationships directly
- Websites are connected to the Subscription contract not vice versa
- Subscription data is capable of being connected across multiple websites and platforms

**Portable Communities**
- Creators can integrate the "subscription button" into any website, app, or platform
- Community follows the creator, not the platform
- High engagement from community members who are also tokenholders

**Subscriber Token Rewards**
- When subscribing (committing), users receive an equal value of creator tokens
- Subscribers become tokenholders in the creator's success
- Tokens can be reinvested into the liquidity pool to earn trading fees

**Collaboration & Sponsorship Ready**
- Built-in fee structure supports creator revenue and protocol sustainability
- Sponsors can integrate with creator pools
- Cross-creator collaborations enabled through the token ecosystem

---

## Architecture

The protocol is organized as four core production contracts (factory, creator-pool, standard-pool, expand-economy), one auxiliary contract (router for multi-hop swaps), one test-only contract (mockoracle), and three shared library packages (pool-core, pool-factory-interfaces, easy-addr). The diagram below covers the four production contracts:

```
┌─────────────────────────────────────────────────────────────┐
│                      FACTORY CONTRACT                        │
│  - Creates creator pools (permissioned) and standard pools   │
│    (permissionless, paid in USD-denominated bluechip)        │
│  - Manages global configuration via 48h timelock             │
│  - Handles CW20 / CW721 / pool wasm instantiation            │
│  - Internal oracle for bluechip/USD pricing (TWAP + warm-up) │
│  - Notifies expand-economy on threshold-crossings            │
│  - Anchor-pool one-shot bootstrap + force-rotate             │
│  - Keeper bounties (oracle update, distribution batches)     │
└─────────────────────────────────────────────────────────────┘
        │                  │                  │
        │ creates           │ creates          │ requests expansion
        ▼                  ▼                  ▼
┌────────────────────┐  ┌────────────────────┐  ┌────────────────────┐
│   CREATOR POOL     │  │   STANDARD POOL    │  │  EXPAND ECONOMY    │
│  - Commit phase    │  │  - Plain xyk AMM   │  │  - Mints bluechip  │
│  - Threshold cross │  │    around any two  │  │    on threshold    │
│  - Post-threshold  │  │    pre-existing    │  │    crossings       │
│    AMM             │  │    assets          │  │  - 24h rolling cap │
│  - Distribution    │  │  - SubMsg-based    │  │  - 48h timelocks   │
│    batches +       │  │    deposit balance │  │    on config /     │
│    keeper bounty   │  │    verification    │  │    withdrawal      │
│  - Threshold-cross │  │    (FoT / rebase   │  │  - Owner / factory │
│    NFT auto-accept │  │    safe)           │  │    role separation │
│                    │  │  - Factory-driven  │  │  - Cosmos-SDK      │
│                    │  │    NFT auto-accept │  │    denom format    │
│                    │  │                    │  │    validation      │
└────────────────────┘  └────────────────────┘  └────────────────────┘
        │                          │
        │ depend on                │
        ▼                          ▼
┌─────────────────────────────────────────────────────────────┐
│              POOL-CORE  (shared library package)             │
│  - Constant-product AMM math + slippage / spread guards      │
│  - Position-NFT helpers (deposit, add, remove, collect fees) │
│  - First-depositor MINIMUM_LIQUIDITY inflation lock          │
│  - Reentrancy lock shared across every hot path              │
│  - Auto-pause when reserves drop below MINIMUM_LIQUIDITY     │
│  - Two-phase emergency withdraw (24h timelock)               │
│  - Strict per-asset fund collection (no orphaned coins)      │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│       POOL-FACTORY-INTERFACES  (shared types package)        │
│  - Wire-format types both pools and the factory speak        │
│  - CW721 message shapes, asset / token-info types,           │
│    factory-bound message envelopes                           │
└─────────────────────────────────────────────────────────────┘
```

### Pool Kinds

| Kind | Created By | Has Commit Phase | Mints CW20 | Cross-Threshold Mint |
|------|-----------|------------------|------------|---------------------|
| **Creator Pool** | Factory admin (rate-limited 1h/address) | Yes — funds via subscriptions until USD threshold | Yes (1.5M cap, factory-minted) | Yes (notifies expand-economy) |
| **Standard Pool** | Anyone (pays USD-denominated fee) | No — tradeable / depositable from instantiate | No (wraps two pre-existing assets) | No (does NOT participate in mint formula) |

Both pool kinds share the same liquidity/swap/position logic via `pool-core`, the same emergency-withdraw machinery, and the same factory message envelope. The differences live in the entry-point crates and the commit-phase code that creator-pool exclusively owns.

---

## How It Works

### Creating a Creator Pool

Creators launch their own token pool by calling the factory's `Create`. Only the pair shape and the new CW20 metadata are caller-supplied; every other knob (commit threshold, fee splits, threshold payout, lock caps, oracle config) is read from factory config at the time of the call. The CW20 contract address is filled in by the factory during the reply chain.

```json
{
  "create": {
    "pool_msg": {
      "pool_token_info": [
        { "bluechip": { "denom": "ubluechip" } },
        { "creator_token": { "contract_addr": "WILL_BE_CREATED_BY_FACTORY" } }
      ]
    },
    "token_info": {
      "name": "Creator Token Name",
      "symbol": "TICKER",
      "decimal": 6
    }
  }
}
```

**Funds attached:** Same `must_pay` shape as `CreateStandardPool` — exactly one coin entry of the canonical bluechip denom, amount ≥ the required USD-denominated creation fee. Multi-denom or wrong-denom payloads error at the boundary; surplus is refunded.

Each creator pool receives:
- A unique CW20 token for the creator (mint cap: 1,500,000)
- A CW721 NFT contract for liquidity positions
- Factory-configured fee structure (default: 1% protocol + 5% creator)
- Bluechip tokens minted via the Expand Economy contract on threshold-crossing (up to 500 per pool, decreasing over time)

A per-address rate limit (1 hour) on `Create` calls keeps an attacker from cheaply inflating the commit-pool ordinal that drives the expand-economy decay schedule.

### Creating a Standard Pool

Anyone can create a plain xyk pool around two pre-existing assets via `CreateStandardPool`. The caller pays a USD-denominated fee in bluechip; the factory converts USD to bluechip via the internal oracle at call time, with a hardcoded fallback for the very first pool (the ATOM/bluechip anchor pool itself).

```json
{
  "create_standard_pool": {
    "pool_token_info": [
      { "bluechip": { "denom": "ubluechip" } },
      { "creator_token": { "contract_addr": "cosmos1..." } }
    ],
    "label": "ubluechip-MYTOKEN-xyk"
  }
}
```

**Funds attached:** Exactly one coin entry of the canonical bluechip denom (e.g. `ubluechip`), amount ≥ the required USD-denominated fee. The handler uses `cw_utils::must_pay` — any other shape (multi-denom, wrong denom, no funds when fee is enabled) errors at the boundary and the tx reverts; the bank module auto-returns all attached funds on revert, so no in-tx refund path is needed. Surplus over the required amount is refunded to the caller in the same tx.

Standard pools:
- Are immediately tradeable / depositable at creation (no threshold)
- Do NOT mint a fresh CW20 — they wrap pre-existing assets
- Do NOT participate in the expand-economy mint formula (defense-in-depth guard inside `calculate_and_mint_bluechip` rejects them)
- Use `pool-core`'s SubMsg-based deposit balance verification path so fee-on-transfer or rebasing CW20s cannot corrupt reserve accounting (mismatch reverts the entire transaction)
- Receive an explicit factory callback that accepts NFT ownership in the same transaction as creation, closing the pending-ownership window before any user can interact

---

## Two-Phase Pool Lifecycle (Creator Pool)

This section covers the **creator-pool** flow only. Standard pools skip the commit phase entirely and start in active-trading mode.

### Phase 1: Pre-Threshold (Funding Phase)

Before a pool reaches its $25,000 USD threshold, only **commit transactions** are allowed. This phase:

- Tracks all commits in a ledger by USD value (not token quantity)
- Provides price stability during the funding period
- Ensures fair valuation regardless of when users commit
- Prevents liquidity provision and normal swaps

**During this phase:**
- Users send bluechip tokens to subscribe/commit
- Commits are tracked by their USD value at time of commitment
- 6% fee is collected (1% protocol + 5% creator)
- All committers are recorded for proportional token distribution

### Threshold Crossing

When total USD committed reaches the threshold ($25,000 default):

1. **Creator tokens minted**: ~1,200,000 creator tokens are minted and distributed
2. **Creator reward**: 325,000 creator tokens sent to the creator's wallet
3. **Protocol reward**: 25,000 creator tokens sent to the Bluechip protocol wallet
4. **Pool seeded**: 350,000 creator tokens + committed bluechip used to initialize AMM liquidity
5. **Committer distribution**: 500,000 creator tokens distributed to committers proportionally
6. **Excess handling**: If bluechip exceeds `max_bluechip_lock_per_pool`, excess bluechip and proportional creator tokens are held in time-locked escrow for the creator (see [Creator Limits](#creator-limits--excess-liquidity))
7. **NFT auto-accept**: The pool sends `Cw721 AcceptOwnership` for its position-NFT contract in the same transaction as the threshold crossing — no pending-ownership window
8. **Expand-economy notification**: The factory's `NotifyThresholdCrossed` reply chain dispatches a bluechip mint via the expand-economy contract (subject to the 24h rolling cap)
9. **State transition**: Pool moves to active trading phase

```
Token Distribution Formula:
user_tokens = (user_usd_contribution / total_usd_committed) × 500,000
```

### Phase 2: Post-Threshold (Active Trading)

Once the threshold is crossed, the pool operates as a full AMM:

**Available Operations:**
- **Commits (Subscriptions)**: Still available with 6% fee, provides subscription tracking
- **Simple Swaps**: Standard AMM swaps with LP fees only (no protocol fees)
- **Add Liquidity**: Provide liquidity and receive NFT position
- **Remove Liquidity**: Withdraw liquidity (partial or full)
- **Collect Fees**: Claim accumulated trading fees without burning position

A 2-block post-threshold cooldown delays the first swap to prevent bundling a manipulative swap into the same block as the threshold-crossing tx.

---

## The Commit Function (Subscribe Button)

The commit function is the core user interaction for subscriptions.

```json
{
  "commit": {
    "asset": {
      "info": { "bluechip": { "denom": "ubluechip" } },
      "amount": "1000000"
    },
    "transaction_deadline": null,
    "belief_price": null,
    "max_spread": null
  }
}
```

**Send with:** Native bluechip tokens attached to the transaction, in the same `amount` as `asset.amount`. Commit transactions can only be carried out with bluechip tokens. The handler uses `cw_utils::must_pay` for strict denom-and-amount validation, so attaching the wrong denom or a different amount fails fast.

### What Happens When You Commit

**Pre-Threshold:**
1. USD value calculated using the oracle rate captured once at handler entry (no mid-tx drift)
2. 6% fee deducted and distributed (1% protocol, 5% creator)
3. Commitment recorded in ledger
4. If threshold crossed, triggers atomic token distribution

**Post-Threshold:**
1. 6% fee deducted and distributed
2. Remaining amount swapped through AMM
3. User receives creator tokens
4. Transaction flagged as subscription for tracking

**Rate Limiting:** A minimum of 13 seconds must elapse between commits from the same wallet to prevent spam.

### Fee Structure

| Fee Type | Recipient | Amount | When Applied |
|----------|-----------|--------|--------------|
| Protocol Fee | Bluechip Wallet | 1% | Commits only |
| Creator Fee | Creator Wallet | 5% | Commits only |
| LP Fee | Liquidity Providers | ~0.3% | All swaps |

**Note:** Regular swaps (non-commits) only pay LP fees, not the 6% subscription fee.

---

## NFT Liquidity Positions

Both creator pools and standard pools represent liquidity positions as NFTs (logic shared via `pool-core`).

### Benefits of NFT Positions

- **Fee Collection Without Burning**: Claim accumulated fees while keeping your position
- **Transferable Positions**: Sell or transfer your liquidity position as an NFT
- **Position Tracking**: Each position tracks its own fee accumulation history
- **Partial Withdrawals**: Remove part of your liquidity while keeping the NFT

### First-Depositor Inflation Lock

The first depositor on an empty pool has `MINIMUM_LIQUIDITY = 1000` LP units locked into their position. The locked units cannot be withdrawn (the position itself can still earn and collect fees), neutralising the classic "donate-then-deposit" share-price-inflation attack on a freshly seeded pool.

### Adding Liquidity

```json
{
  "deposit_liquidity": {
    "amount0": "1000000",
    "amount1": "1000000",
    "min_amount0": "990000",
    "min_amount1": "990000",
    "transaction_deadline": null
  }
}
```

**Returns:** NFT representing your liquidity position.

On a **standard pool** the deposit is dispatched as a SubMsg with `reply_on_success`. The pool's reply handler re-queries the CW20 balance and asserts that `post − pre == credited`. Any mismatch (fee-on-transfer / rebase) reverts the entire transaction so reserve accounting cannot drift away from the pool's actual on-chain balance.

### Adding to Existing Position

```json
{
  "add_to_position": {
    "position_id": "123",
    "amount0": "500000",
    "amount1": "500000"
  }
}
```

**Note:** Any uncollected fees are automatically claimed when adding to a position.

### Collecting Fees

```json
{
  "collect_fees": {
    "position_id": "123"
  }
}
```

Fees are calculated using a global fee growth accumulator:

```
fees_owed = (fee_growth_global - fee_growth_at_last_collection) × position_liquidity
```

Small positions are subject to a fee-size multiplier; the clipped portion is routed to a creator-fee pot rather than being lost, so dust positions can't farm fees disproportionately.

### Removing Liquidity

```json
{
  "remove_partial_liquidity": {
    "position_id": "123",
    "liquidity_to_remove": "500000",
    "min_amount0": null,
    "min_amount1": null,
    "max_ratio_deviation_bps": 100,
    "transaction_deadline": null
  }
}
```

`RemovePartialLiquidityByPercent { percentage }` and `RemoveAllLiquidity {}` are convenience variants over the same handler. Partial removal keeps the NFT; full removal burns it. Pulling reserves below `MINIMUM_LIQUIDITY` auto-pauses the pool (separate auto-pause flag); the pause clears automatically as soon as a deposit restores reserves above the floor.

---

## Internal Oracle System

Bluechip uses an internal oracle to price the native bluechip token in USD.

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    PYTH ORACLE                               │
│                   (ATOM/USD Price)                           │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                 ATOM/bluechip POOL                           │
│              (Primary Price Reference)                       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│               INTERNAL ORACLE (FACTORY)                      │
│  - Anchor-only TWAP in v1 (basket aggregation disabled)      │
│  - Warm-up gate after anchor change / force-rotate           │
│  - Per-update circuit breaker (30% drift cap)                │
│  - Bifurcated strict vs. best-effort price reads             │
└─────────────────────────────────────────────────────────────┘
```

See `docs/ORACLE_CONSTANTS.md` for the full rationale on every hardcoded constant in the oracle and the path to make any of them governance-tunable.

### Price Calculation

1. **ATOM/bluechip** price from the anchor pool (TWAP over `TWAP_WINDOW = 3600s`)
2. **ATOM/USD** price from Pyth Network oracle
3. **bluechip/USD** = ATOM/USD × ATOM/bluechip

### Anchor-only mode (v1)

`ORACLE_BASKET_ENABLED = false` in v1. The anchor pool is the sole price source; basket aggregation across multiple pools is gated off until per-pool USD normalization is wired in. The eligible-pool curation, sampling, and rotation logic is present in code but does not influence `last_price` while the basket gate is off.

### Manipulation Resistance

- **Anchor-only TWAP**: time-weighted price over the 1h `TWAP_WINDOW`, sampled at `UPDATE_INTERVAL = 300s` minimum cadence.
- **TWAP Circuit Breaker**: Each update is rejected if it deviates by more than `MAX_TWAP_DRIFT_BPS = 30%` from the prior published price (the very first update bypasses the breaker by definition).
- **Warm-Up Gate**: After bootstrap, an admin-driven anchor change, or `ForceRotateOraclePools`, the oracle requires `ANCHOR_CHANGE_WARMUP_OBSERVATIONS = 5` successive successful TWAP rounds before downstream USD conversions resume — preventing the very first post-reset observation from being locked in by an attacker who briefly perturbed the new anchor's reserves. Strict callers (commit valuation) hard-fail during warm-up; best-effort callers (CreateStandardPool fee, distribution bounty) fall back to `pre_reset_last_price` when available.
- **Stale-Price Rejection**: Pyth prices older than `MAX_PRICE_AGE_SECONDS_BEFORE_STALE = 300s` are rejected. The staleness check uses `u64` saturating subtraction and rejects negative `publish_time` plus any timestamp more than 5 seconds in the future, so a buggy publisher cannot wrap signed-`i64` arithmetic to pass the cap vacuously.
- **Pool-side staleness window**: pool-level `MAX_ORACLE_STALENESS_SECONDS = 360s` (matches `UPDATE_INTERVAL + 60s grace`) gates commit acceptance against cache freshness; the boundary is `>` (strict), so exactly `ts + 360s` accepts.

### Force-Rotate (Admin)

`ForceRotateOraclePools` is a 2-step admin action gated by the standard 48-hour `PENDING_ORACLE_ROTATION` timelock. On execution, the oracle clears its cumulative snapshots, clears the price cache, re-arms the warm-up gate, and re-selects its sample set — preventing a compromised admin from instantly rotating the oracle's sample set without a community-observable window.

### Keeper Bounty

`UpdateOraclePrice` is permissionless and pays a USD-denominated bounty (capped at $0.10) to the caller, paid out in bluechip after USD→bluechip conversion. The existing per-update interval gates frequency, so the bounty cannot be spammed.

---

## Query Endpoints

### Pool State (LP-side)

```json
{
  "pool_state": {}
}
```

**Returns** `PoolStateResponse`: `nft_ownership_accepted`, `reserve0`, `reserve1`, `total_liquidity`, `block_time_last`. The factory-facing `get_pool_state {}` returns a different (richer) shape, `PoolStateResponseForFactory`; LP / SDK consumers should use `pool_state {}`.

### Commit Status

```json
{
  "is_fully_commited": {}
}
```

**Returns** the on-chain `CommitStatus` enum: either the bare string `"fully_committed"` or `{ "in_progress": { "raised": "...", "target": "25000000000" } }`. Standard pools always return `"fully_committed"` (no commit phase).

### Position Info

```json
{
  "position": { "position_id": "123" }
}
```

`positions { start_after, limit }` and `positions_by_owner { owner, start_after, limit }` page through the same shape.

### Simulate Swap

```json
{
  "simulation": {
    "offer_asset": {
      "info": { "bluechip": { "denom": "ubluechip" } },
      "amount": "1000000"
    }
  }
}
```

`reverse_simulation { ask_asset }` solves for the offer amount that produces a given output.

### Pool Analytics

```json
{
  "analytics": {}
}
```

Provides a comprehensive snapshot of pool state for indexers and analytics dashboards (TVL, fee reserves, threshold status, position count, swap/commit counters, current spot prices in both directions).

---

## Integration Guide

### Embedding the Commit Button

```javascript
// Using CosmJS
const amount = "1000000"; // micro-units
const msg = {
  commit: {
    asset: {
      info: { bluechip: { denom: "ubluechip" } },
      amount
    },
    transaction_deadline: null,
    belief_price: null,
    max_spread: null
  }
};

const result = await client.execute(
  senderAddress,
  poolContractAddress,
  msg,
  "auto",
  undefined,
  [{ denom: "ubluechip", amount }]
);
```

### Standard-Pool Deposit (CW20 Approval Required)

```javascript
// Approve the standard pool to spend the CW20 first.
await client.execute(senderAddress, cw20Address, {
  increase_allowance: { spender: standardPoolAddress, amount: "1000000" }
}, "auto");

// Deposit native + CW20.
await client.execute(
  senderAddress,
  standardPoolAddress,
  {
    deposit_liquidity: {
      amount0: "1000000",
      amount1: "1000000",
      min_amount0: null,
      min_amount1: null,
      transaction_deadline: null
    }
  },
  "auto",
  undefined,
  [{ denom: "ubluechip", amount: "1000000" }]
);
```

The standard-pool reply handler will reject the transaction if the CW20 has a transfer fee or rebase that makes the credited delta differ from `amount1`.

### Checking Subscription Status

```json
{
  "committing_info": { "wallet": "bluechip1..." }
}
```

`last_commited { wallet }` (note the on-chain typo) returns the wallet's most recent commit timestamp and per-commit USD / bluechip amounts; useful for enforcing the 13-second per-wallet rate limit client-side before broadcasting. `pool_commits { pool_contract_address, min_payment_usd, after_timestamp, start_after, limit }` pages the full committer ledger for a pool — the response carries `committers` and a `page_count` (size of THIS page after filtering, not the pre-filter total).

---

## Security Considerations

### Reentrancy Protection
- Single shared `REENTRANCY_LOCK` covering commit, swap, and every liquidity path on both pool kinds, so a hostile CW20's transfer hook can't reach any handler from any other path mid-execution.
- State updates occur before external calls.

### Oracle Security
- Anchor-only TWAP in v1 (`ORACLE_BASKET_ENABLED = false`); basket aggregation gated off until per-pool USD normalization is wired in.
- Warm-up gate (5 successive observations) re-arms after every bootstrap, anchor change, and admin-triggered force-rotate, preventing first-observation-after-reset from being locked in. Bifurcated: strict callers hard-fail during warm-up; best-effort callers fall back to `pre_reset_last_price` when available.
- Per-update TWAP circuit breaker (30% max drift) rejects out-of-band price moves on every update after the first.
- Stale-price rejection at 300 seconds (Pyth). The staleness check uses `u64` saturating subtraction and explicitly rejects negative `publish_time` values plus any `publish_time` more than 5 seconds in the future, so a buggy or malicious Pyth publisher cannot wrap signed-`i64` arithmetic in release wasm to make a far-past or far-future timestamp pass the cap vacuously.
- Pool-side staleness window (360s) matches the 300s update cadence plus a 60s keeper-jitter grace; boundary is strict (`>`), so exactly `ts + 360s` accepts.
- Keeper bounty USD-denominated and hard-capped at $0.10 — caps yearly drain at ~$10.5k worst-case if admin is compromised.

### Threshold Mechanics
- Threshold can only be crossed once (irreversible).
- Atomic state transitions during threshold crossing — the entire payout (creator share, protocol share, committer distribution, AMM seeding, NFT auto-accept, expand-economy notification) lives in a single tx.
- USD-based tracking prevents token-price manipulation around the threshold.
- Batched distribution for large committer sets (>40), with per-call keeper bounty paid by the factory.
- Stuck-state recovery via `RecoverStuckStates` (factory admin, after timeout); handler refuses to operate on already-drained pools.
- 2-block post-threshold cooldown delays the first swap so an attacker can't bundle a manipulative swap into the threshold-crossing block.

### Per-Address Rate Limit on Pool Creation
- 1-hour cooldown per `info.sender` on creator-pool creation. Defends against trivial spam-creates that would inflate the commit-pool ordinal and gas-amplify per-pool storage scans. Coordinated multi-address spam still has to fund and sign from each new address it rotates through.

### Standard-Pool / Pool-Core Defenses
- **SubMsg-based deposit balance verification** (standard-pool only): each CW20-side TransferFrom is anchored by a `reply_on_success` SubMsg whose handler re-queries the post-balance and asserts equality with the credited delta. Strict equality (not `≥`) so both fee-on-transfer shortfalls and inflate-on-transfer overages revert.
- **Strict per-asset fund collection**: deposits reject any attached coin whose denom isn't one of the pool's configured native sides. Pre-fix, accidentally attached IBC / tokenfactory denoms would have orphaned in the pool's bank balance.
- **First-depositor MINIMUM_LIQUIDITY lock**: 1000 LP units locked on the first deposit's position; cannot be withdrawn. Fees still accrue against the locked amount.
- **Auto-pause on low reserves**: a remove-liquidity that drops reserves below `MINIMUM_LIQUIDITY` flips a separate `POOL_PAUSED_AUTO` flag (distinct from admin pauses). Deposits are permitted while auto-paused so the recovery path stays open; swaps and removes are not. Auto-flag clears as soon as a deposit restores reserves above the floor.
- **NFT pending-ownership window closed**: standard-pool's `AcceptNftOwnership` factory callback runs in the same tx as pool creation; creator-pool auto-accepts at threshold-cross. No window where the NFT contract has the pool as `pending_owner` but not actual owner.
- **Migrate downgrade guard**: every contract's migrate handler parses cw2-stored version and compile-time `CONTRACT_VERSION` as semver and refuses any migrate where stored > current.

### Two-Phase Emergency Withdraw
- Phase 1 (`EmergencyWithdraw` while no pending) sets the timelock and pauses the pool.
- Phase 2 (`EmergencyWithdraw` while pending and >24h elapsed) drains reserves to the configured bluechip wallet.
- `CancelEmergencyWithdraw` is available to the factory admin during Phase 1.
- Standard pools route the drain to the configured `bluechip_wallet_address` (NOT the factory contract — funds sent there would be permanently locked since the factory has no withdrawal mechanism).

### Swap Validation
- Zero-amount CW20 swaps are rejected.
- Creator tokens are enforced to use 6 decimals to match hardcoded payout amounts.

### Commit Rate Limiting
- Minimum 13 seconds between commits per wallet.
- Oracle rate is captured once at commit entry and threaded through every conversion in the handler — no mid-tx drift between the USD valuation and the threshold check.

### Threshold Crossing Protection
- Excess swap at threshold crossing is capped at 3% of pool reserves, preventing a single large committer from dominating the first trade.
- Any excess beyond the cap is refunded to the committer.
- The factory-notify SubMsg is `reply_on_error`: a notify failure does NOT revert the crossing tx — it sets `PENDING_FACTORY_NOTIFY` so `RetryFactoryNotify` (permissionless) can re-send. The reply handler is a surgical mutator of that flag alone — no crossing-side storage is touched on the retry path.

### Payout Integrity Validation
- All threshold payout components validated (no zero amounts).
- No individual component can exceed the total.

---

## Token Economics

### Creator Token Supply

Each creator pool mints a total of **1,200,000** creator tokens at threshold crossing, distributed as follows:

| Recipient | Amount | % of Total | Purpose |
|-----------|--------|------------|---------|
| Committers | 500,000 | ~41.7% | Proportional to USD committed |
| Creator | 325,000 | ~27.1% | Creator reward |
| Protocol (Bluechip Wallet) | 25,000 | ~2.1% | Protocol sustainability |
| Pool Liquidity Seed | 350,000 | ~29.2% | Initial AMM liquidity |

The CW20 token contract is instantiated with a mint cap of **1,500,000**, allowing for future controlled minting beyond the initial threshold distribution.

### Fee Flow

```
Commit Transaction (5000 bluechip)
        │
        ├── 1% (50) → Protocol Wallet
        ├── 5% (250) → Creator Wallet
        └── 94% (4700) → Pool/Swap
```

---

## Expand Economy

The Expand Economy contract manages bluechip token inflation by minting new tokens each time a creator pool crosses its threshold. This incentivizes early adoption while gradually reducing emissions as the ecosystem grows.

### How It Works

```
┌──────────────┐     ┌────────────────────┐     ┌───────────────────┐
│   Creator    │ ──► │  Factory Contract   │ ──► │  Expand Economy   │
│ pool crosses │     │ NotifyThresholdCross│     │  RequestExpansion │
│  threshold   │     │ calculate_mint()    │     │  (24h cap, factory│
└──────────────┘     └────────────────────┘     │   role gated)     │
                                                 └───────────────────┘
                                                          │
                                                          ▼
                                              Mints bluechip tokens
                                              to protocol wallet
```

1. A commit pushes a creator pool past its USD threshold.
2. The pool fires `NotifyThresholdCrossed` to the factory (subject to one-shot `POOL_THRESHOLD_MINTED` — never twice for the same pool).
3. The factory rejects standard pools, then computes the mint amount via the decay formula.
4. The factory sends `RequestExpansion` to the Expand Economy contract.
5. Expand-economy validates the request (factory-only, denom cross-check against the factory's configured `bluechip_denom`, daily cap, sufficient balance) and dispatches a `BankMsg::Send` to the protocol wallet.

### Mint Formula

```
mint_amount = 500 - ((5x² + x) / ((s / 6) + 333x))
```

Where:
- **x** = `commit_pool_ordinal` — a commit-pool-only counter (NOT the global `pool_id`). Standard-pool creations cannot inflate `x`.
- **s** = seconds elapsed since the first threshold-crossing
- **Result** is in whole tokens (multiplied by 10⁶ for micro-denomination)

**Properties:**
- **Maximum mint**: 500 bluechip per threshold-crossing
- **Decreasing curve**: Mint amount decreases as more commit pools cross threshold
- **Time decay**: Longer time between threshold-crossings further reduces the mint
- **Floor**: Mint amount cannot go below zero

### Daily Expansion Cap

`DAILY_EXPANSION_CAP = 100,000,000,000 ubluechip` (= 100,000 bluechip) bounds the worst-case daily drain if the configured factory address is ever compromised. The window is a single bucket that resets opportunistically on the first call after `DAILY_WINDOW_SECONDS = 86,400` has elapsed since the bucket's start. Skipped requests (insufficient balance, dormant decay) do not burn cap budget.

### Cross-Validation

Every `RequestExpansion` cross-validates the factory's configured `bluechip_denom` against this contract's stored denom. A mismatch (admin updated one side without the other) returns an explicit error rather than silently funding rewards in the wrong denom.

### Access Control

| Action | Who Can Call |
|--------|-------------|
| `RequestExpansion` | Factory contract only (sender check + denom cross-validation) |
| `ProposeConfigUpdate` / `ExecuteConfigUpdate` / `CancelConfigUpdate` | Owner (48h timelock) |
| `ProposeWithdrawal` / `ExecuteWithdrawal` / `CancelWithdrawal` | Owner (48h timelock) |
| `migrate` | Chain admin (downgrade rejected) |

Every execute path is non-payable — `cw_utils::nonpayable` rejects any attached funds at dispatch, so coins attached to a propose/cancel/request call cannot orphan in the contract's bank balance.

The owner-supplied `bluechip_denom` (instantiate or config update) is validated against the cosmos-sdk denom format (`^[a-zA-Z][a-zA-Z0-9/:._-]{2,127}$`) at submission time, surfacing typos at propose rather than 48h later when the bank module would have rejected.

### Query Endpoints

```json
{ "get_config": {} }
```
Returns `{ factory_address, owner, bluechip_denom }`.

```json
{ "get_balance": { "denom": "ubluechip" } }
```
Returns the contract's bank balance of the specified denomination.

---

## Creator Limits & Excess Liquidity

### Maximum Bluechip Lock Per Pool

Each pool enforces a maximum amount of bluechip tokens that can be locked as liquidity at threshold crossing (`max_bluechip_lock_per_pool`). This prevents the ecosystem from having all bluechip locked into unowned liquidity positions and incentivises creators to join while bluechip is lower in value. When committed bluechip exceeds this limit at threshold crossing, the excess is held in a time-locked escrow for the creator rather than being lost.

### Creator Excess Liquidity

When bluechip exceeds the per-pool maximum:

1. The excess bluechip and proportional creator tokens are stored in a `CreatorExcessLiquidity` record
2. An unlock timestamp is set based on `creator_excess_liquidity_lock_days` (configured at the factory level)
3. After the lock period expires, the creator can claim the excess tokens directly to their wallet

```
Threshold Crossing (e.g., 15B bluechip committed, 10B max per pool)
        │
        ├── 10B bluechip → Pool liquidity (immediate)
        └── 5B bluechip + proportional creator tokens → Time-locked
                │
                └── Unlocks after X days → Creator calls ClaimCreatorExcessLiquidity
                        │
                        ├── Bluechip tokens → Sent directly to creator wallet
                        └── Creator tokens → Sent directly to creator wallet
```

### Claiming Excess Liquidity

```json
{
  "claim_creator_excess_liquidity": {}
}
```

Tokens are sent directly to the creator's wallet (not deposited as liquidity). The creator can then choose to deposit them as liquidity or use them as they wish.

**Requirements:**
- Caller must be the creator of the pool
- The lock period must have expired
- Can only be claimed once

### Configuration Defaults

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_bluechip_lock_per_pool` | 10,000,000,000 (10B) | Max bluechip tokens locked as liquidity in a single pool |
| `creator_excess_liquidity_lock_days` | 7 days | Time lock before creator can claim excess |

---

## Batched Threshold Distribution

When a pool crosses its USD threshold, creator tokens must be distributed to all committers proportionally. For pools with many committers, this is handled in batches.

### Distribution Logic

- **Small pools (≤ 40 committers)**: All distributions happen in a single transaction
- **Large pools (> 40 committers)**: Distributions are batched across multiple transactions using a `DistributionState` tracker

```
Distribution State Machine:
┌──────────┐     ┌───────────┐     ┌───────────┐
│  Start   │ ──► │  Batch N  │ ──► │ Complete  │
│          │     │ (≤40 each)│     │           │
└──────────┘     └───────────┘     └───────────┘
                      │  ▲
                      └──┘ (continue until all distributed)
```

Each committer receives tokens proportional to their USD contribution:

```
user_tokens = (user_usd_contribution / total_usd_committed) × commit_return_amount
```

### Keeper Bounty

`ContinueDistribution` is permissionless and pays a USD-denominated bounty (capped at $0.10) to the caller for each successful batch, paid out in bluechip from the factory's pre-funded native balance. A 5-second per-address cooldown on `ContinueDistribution` prevents a single keeper from monopolising the bounty.

### Recovery

If distribution gets stuck (>1 hour or 5+ consecutive failures), the factory admin can trigger `RecoverStuckStates` to resume processing. The handler refuses to operate on already-drained pools (defense-in-depth against state corruption between drain and recovery attempts).

---

## Admin Operations

### Factory Configuration Updates

Configuration updates use a 48-hour timelock:

1. Admin calls `ProposeConfigUpdate` with new values (factory-side validation rejects empty strings, invalid bech32, non-positive fees, fee-sum overflow, malformed Pyth address / feed id)
2. The proposal does NOT overwrite an existing pending update — `Cancel` first if you need to replace
3. After the 48h timelock expires, `UpdateConfig` applies the pending changes

### Pool Configuration Updates

Individual pool settings can also be updated through a timelocked process:

1. Admin calls `ProposePoolConfigUpdate` with the pool ID and new values
2. A 48-hour timelock is applied
3. After the timelock expires, `ExecutePoolConfigUpdate` applies the changes to the target pool

### Pool Upgrades

Pools can be migrated to new contract code in batches:

```json
{
  "upgrade_pools": {
    "new_code_id": 42,
    "pool_ids": [1, 2, 3],
    "migrate_msg": "<binary>"
  }
}
```

- 48h timelock before execution
- Anchor pool is excluded from upgrade lists (reject at propose time)
- `pool_ids` is deduplicated before applying
- Processes up to 10 pools per transaction
- Automatically continues with `ContinuePoolUpgrade` until all pools are migrated
- Skips paused pools rather than reverting the entire batch

### Force-Rotate Oracle Pools

`ProposeOracleRotation` → wait 48h → `ForceRotateOraclePools`. On execution, the oracle clears cumulative snapshots, clears the price cache, re-arms the warm-up gate, and re-selects its sample set.

### Anchor Pool Bootstrap

`SetAnchorPool { pool_id }` is a one-shot bootstrap callable until `INITIAL_ANCHOR_SET = true`. The handler enforces that the anchor pool's non-bluechip side matches the factory's configured `atom_denom` exactly. After the one-shot fires, any subsequent change must go through the standard 48h `ProposeConfigUpdate` flow.

### Pool Pause/Unpause

The factory admin can pause individual pools, disabling all swap and liquidity operations while preserving state. Admin pauses are tracked separately from the auto-pause flag — admin-paused pools require explicit `Unpause`, while auto-paused pools clear themselves when reserves recover.

### Migration

Every contract (factory, creator-pool, standard-pool, expand-economy) exports a migrate entry point that:
- Tolerates a missing cw2 entry (legacy / test fixtures)
- Parses both the stored cw2 version and the compile-time `CONTRACT_VERSION` as semver
- **Refuses any migrate where stored > current** (downgrade protection)
- Bumps the cw2 record on success

---

## Key Constants & Limits

All values below are the **production** defaults. Constants marked with
**🧪** are cfg-gated and shortened under `--features integration_short_timing`
for shell-script integration tests (the docker `mock` build variant); see
the **Cargo features** section under Development for the full list of
overrides. Constants without 🧪 are pinned regardless of build flavour.

| Parameter | Value | Description |
|-----------|-------|-------------|
| Commit threshold (USD) | 25,000 | USD value required to activate creator pool |
| Creator token mint cap | 1,500,000 | Max CW20 supply per creator pool |
| Max bluechip lock per pool | 10,000,000,000 | Excess is time-locked for creator |
| Creator excess lock period | 7 days | Time before creator can claim excess |
| Commit fee (protocol) | 1% | Sent to Bluechip wallet |
| Commit fee (creator) | 5% | Sent to creator wallet |
| LP swap fee | 0.3% | Distributed to liquidity providers |
| Max excess swap at threshold | 3% of pool reserves | Caps single-committer dominance of the first trade |
| Creator token decimals | 6 | Enforced to match hardcoded payout amounts |
| Min commit interval | 13 seconds | Per-wallet commit rate limit |
| First-depositor lock | 1000 LP | `MINIMUM_LIQUIDITY` locked into first deposit |
| Distribution batch size | 40 | Max committers per distribution tx |
| Distribution keeper cooldown | 5 seconds | Per-address, prevents bounty monopoly |
| Commit-pool create rate limit 🧪 | 3600 seconds | Per-address, per `Create` call |
| Standard-pool create rate limit 🧪 | 3600 seconds | Per-address, per `CreateStandardPool` call |
| Default slippage | 0.5% | Default max slippage for swaps |
| Max slippage | 50% | Hard cap on swap slippage |
| Post-threshold swap cooldown | 2 blocks | Delays first swap after threshold |
| Emergency withdraw timelock | 86,400 s (24h) | Phase 1 → Phase 2 delay |
| Admin timelock (factory) 🧪 | 172,800 s (48h) | Config / upgrade / force-rotate |
| Admin timelock (expand-economy) 🧪 | 172,800 s (48h) | Config update + withdrawal |
| Oracle TWAP window | 3600 seconds | Time-weighted price window |
| Oracle update interval 🧪 | 300 seconds | Min between price updates |
| Oracle stale-price max age (Pyth) | 300 seconds | Live Pyth + cached-Pyth max age |
| Oracle stale-price max age (pool-side) | 360 seconds | Pool-side acceptance window for `ConversionResponse` |
| Oracle rotation interval 🧪 | 3600 seconds | Sample re-selection cadence (basket disabled in v1) |
| Oracle warm-up observations 🧪 | 5 | Required after anchor change / rotate (force-cleared per call under integration_short_timing) |
| Oracle TWAP drift cap | 30% (3000 bps) | Per-update circuit breaker |
| Min eligible pools for TWAP | 3 | Below this the oracle falls back to anchor-only |
| Min pool liquidity (oracle eligibility) 🧪 | $5,000 USD | Per-side bluechip-denominated floor (USD-converted) |
| Min bootstrap observations 🧪 | 6 | Required before `ConfirmBootstrapPrice` |
| Bootstrap observation window 🧪 | 3600 s (1h) | Min wait before `ConfirmBootstrapPrice` |
| Oracle snapshot refresh rate limit 🧪 | 7200 blocks (~12h) | Min between `RefreshOraclePoolSnapshot` calls |
| `ORACLE_BASKET_ENABLED` 🧪 | `false` | When `true` the oracle samples eligible pools; when `false` it stays anchor-only |
| Eligible-pool refresh window | 72,000 blocks (~5d) | Snapshot rebuild cadence |
| Oracle update bounty cap | $0.10 USD (6 dec) | Per successful update |
| Distribution batch bounty cap | $0.10 USD (6 dec) | Per successful batch |
| Expand-economy daily cap | 100,000,000,000 ubluechip | Rolling 24h cap on `RequestExpansion` |
| Expand-economy window | 86,400 seconds | Single-bucket reset interval |
| Pyth `publish_time` future-skew tolerance | 5 seconds | Max allowed clock skew between Pyth publishers and chain block time |

---

## Development

### Building

```bash
# Build all contracts
cargo wasm

# Optimize for deployment
docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source="$(basename "$(pwd)")_cache",target=/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0
```

The optimizer is driven by each crate's `[[package.metadata.optimizer.builds]]`
entries (see `factory/Cargo.toml` and `expand-economy/Cargo.toml`) and emits
three variants per build:

| Artifact suffix | Cargo features | Use |
|---|---|---|
| `<crate>.wasm` | none (default) | **Production.** Real 48h timelocks, full warmup gate, $5k liquidity floor, 300s keeper cooldown, anchor-only oracle. |
| `<crate>-mock.wasm` | `mock, integration_short_timing` | Shell-script integration tests. 120s timelocks, warmup cleared per call, UpdateTooSoon bypassed, lowered floors, basket oracle on. Mockoracle queries enabled. NEVER ship. |
| `<crate>-mock_only.wasm` | `mock` only | Mockoracle queries enabled but every timing constant pinned to production values. Used for end-to-end verification that prod-timing gates fire correctly on a real chain. |

The Makefile's `optimize-factory`/`optimize-expand-economy` targets rename the
`-mock` artifact onto `<crate>.wasm` so the test-deploy toolchain finds it
unchanged; the `-mock_only` artifact keeps its suffix.

### Cargo features (factory and expand-economy)

- **`mock`** — enables the test-infrastructure surface only: the mockoracle's
  `BLUECHIP_USD` price short-circuit inside `update_internal_oracle_price`, the
  `testing/` test module gate, and a few helper functions consumed by unit tests.
  Production behaviour (timelocks, warmup, liquidity floors, cooldowns,
  breaker thresholds, basket mode) is **unchanged** — the test suite verifies
  the production paths under this feature.
- **`integration_short_timing`** — shortens every timing constant (admin
  timelock 48h → 120s, bootstrap observation 1h → 30s, rotation interval 1h
  → 60s, etc.), lowers the per-side liquidity floor to a few microbluechip,
  bypasses the 300s `UpdateOraclePrice` cooldown, clears `warmup_remaining` on
  every keeper call, and flips `ORACLE_BASKET_ENABLED` to `true`. Layered on
  top of `mock` so the shell-script integration suite can drive a full
  end-to-end deploy in minutes instead of days. **MUST NEVER ship to
  production** — these constants are deliberately weakened.

### Testing

```bash
# Unit tests — production-equivalent semantics (mock feature only)
cargo test -p factory --features mock --lib --release
cargo test -p creator-pool --release --lib
cargo test -p standard-pool --release --lib
cargo test -p expand-economy --release --lib
cargo test -p pool-core --release --lib
```

The workspace ships with extensive coverage (current PASS counts):
- factory: 247 tests
- creator-pool: 222 tests
- standard-pool: 76 tests
- expand-economy: 39 tests
- pool-core: 25 tests

### Repository Layout

```
bluechip-contracts/
├── factory/                          # Factory contract
├── creator-pool/                     # Creator pool (commit + AMM)
├── standard-pool/                    # Plain xyk pool
├── expand-economy/                   # Bluechip mint reservoir
├── mockoracle/                       # Test-only Pyth-shaped oracle
├── router/                           # Multi-hop swap router
├── packages/
│   ├── pool-core/                    # Shared AMM library
│   ├── pool-factory-interfaces/      # Shared wire-format types
│   └── easy-addr/                    # Test-only deterministic-addr helper
├── fuzz/                             # cargo-fuzz pure-math targets (excluded from default workspace)
├── fuzz-stateful/                    # proptest stateful harness (workspace member)
├── keepers/                          # Off-chain bots (oracle / distribution)
└── frontend/                         # Reference UI
```

See `FUZZING.md` for the fuzz harness layout and `FUZZ_REVIEW.md` for the latest coverage review.

### Deployment Order

1. Deploy CW20-base and CW721-base wasms (store code)
2. Deploy `expand-economy` contract
3. Deploy `factory` contract with the code IDs and the expand-economy address
4. Set `bluechip_mint_contract_address` on the factory if not already set
5. Use `CreateStandardPool` to create the ATOM/bluechip anchor pool first
6. Call `SetAnchorPool { pool_id }` on the factory (one-shot bootstrap, must match `atom_denom`)
7. Initialize the internal oracle (first `UpdateOraclePrice` call seeds snapshots)
8. Wait for the oracle warm-up gate to clear (5 successful TWAP rounds)
9. Creators can now create commit pools; anyone can create additional standard pools

### Local / Mock-Oracle Deployment

Shell scripts that wrap the deployment sequence (`deploy_full_stack_mock_oracle.sh`,
`deploy_osmo_testnet.sh`, `deploy_osmo_testnet_anchor.sh`, and the various
`test_*.sh` / `verify_*.sh` scenario drivers) are intentionally **not
checked in** — they reference per-deployment state files
(`osmo_testnet.state`, `commit_pools.txt`) and operator-specific config
(chain endpoint, wallet keyring, gas prices) that don't belong in the
repo. Maintain them in a separate ops/runbook or a private fork.

The on-chain deployment sequence those scripts encode is the **Deployment
Order** above, plus three operator-side prerequisites:

1. **Mockoracle (testnet only)** — instantiate `mockoracle/` against a
   `cw20_base` / `cw721_base` already on chain. Pre-seed `ATOM_USD` and
   `BLUECHIP_USD` price feeds before instantiating the factory; the
   factory's `--features mock` BLUECHIP_USD short-circuit only fires
   if the feed is present.
2. **Pyth keeper (mainnet / testnet against real Pyth)** — a loop that
   pulls fresh VAAs from a Hermes endpoint and submits them to the
   `pyth_contract_addr_for_conversions` every ~30 s. Without a fresh
   Pyth price the factory falls back to its cached value within
   `MAX_PRICE_AGE_SECONDS_BEFORE_STALE = 300s`, then errors. The
   reference implementation queries
   `${HERMES_ENDPOINT}/api/latest_vaas?ids[]=${PYTH_OSMO_USD_FEED_ID}`,
   computes the per-VAA fee via the Pyth contract's `get_update_fee`
   query, and submits `update_price_feeds` with the VAA payload.
3. **Oracle bootstrap** — after `SetAnchorPool` lands, drive
   `UpdateOraclePrice` plus a few swaps on the anchor pool so the
   internal oracle accumulates TWAP observations. On production builds
   this takes ≥ `MIN_BOOTSTRAP_OBSERVATIONS = 6` rounds spaced by
   `UPDATE_INTERVAL = 300s` (~30 min), plus the
   `BOOTSTRAP_OBSERVATION_SECONDS = 3600s` (1 h) window before
   `ConfirmBootstrapPrice` can publish the candidate.

Mainnet deployment runs the same sequence but against a real Pyth
contract address — drop the mockoracle upload + price-seed steps,
keep the Pyth keeper running before any commit pool is created.

---

## Links

- Website: https://www.bluechip.link/home
- Discord: https://discord.gg/gfdWgHFY
- Twitter: https://x.com/BlueChipCreate
