# Fee and Oracle Mechanics

This document explains how fees are calculated during commits and swaps, and how the internal oracle determines bluechip token pricing.

---

## 1. Commit Fees

When a user commits bluechip tokens to a pool (pre-threshold), two fees are deducted immediately from the committed amount:

| Fee | Recipient | Default |
|-----|-----------|---------|
| `commit_fee_bluechip` | BlueChip wallet | 10% |
| `commit_fee_creator` | Pool creator wallet | 10% |

### How it works

1. User sends `amount` of bluechip tokens via `Commit`
2. `commit_fee_bluechip_amt = amount * commit_fee_bluechip` — sent to the BlueChip wallet
3. `commit_fee_creator_amt = amount * commit_fee_creator` — sent to the creator wallet
4. `amount_after_fees = amount - total_fees` — remains in the pool contract
5. The combined fee rates must be < 100% (enforced at pool instantiation)

### USD valuation

The user's commit is valued in USD using the internal oracle **before** fee deduction:

```
usd_value = get_usd_value(asset.amount)  // full pre-fee amount
```

This `usd_value` is:
- Added to `USD_RAISED_FROM_COMMIT` (tracks progress toward threshold)
- Recorded in `COMMIT_LEDGER[user_address]` (determines token distribution share)

The actual bluechip tokens remaining in the pool (`amount_after_fees`) are tracked separately in `NATIVE_RAISED_FROM_COMMIT`.

### Why USD credit uses pre-fee amount

The USD tracking determines when the threshold is reached and how creator tokens are distributed proportionally among committers. Since all committers pay the same fee rate, using pre-fee amounts preserves proportional fairness — every committer's share is diluted equally by fees.

The pool's actual bluechip reserves at threshold crossing will be `NATIVE_RAISED_FROM_COMMIT` (post-fee total), which is less than what `USD_RAISED_FROM_COMMIT` suggests. The difference equals the total fees paid to the BlueChip and creator wallets.

---

## 2. Swap Fees (LP Fee)

After a pool crosses threshold and becomes an active AMM, every swap incurs an LP fee.

| Parameter | Default | Range |
|-----------|---------|-------|
| `lp_fee` | 0.3% (3 permille) | 0% to < 100% |

### Calculation

The pool uses the constant product formula (`x * y = k`):

```
return_amount = ask_pool - (offer_pool * ask_pool) / (offer_pool + offer_amount)
commission_amount = return_amount * lp_fee
final_return = return_amount - commission_amount
```

The `commission_amount` is retained in the pool and distributed to liquidity providers proportional to their share of total liquidity, weighted by a fee size multiplier based on position size relative to optimal liquidity.

### Spread

Spread represents the difference between the ideal exchange rate (current reserves ratio) and the actual return due to price impact:

```
ideal_return = offer_amount * (ask_pool / offer_pool)
spread = ideal_return - return_amount
```

Users can set `belief_price` and `max_spread` to protect against unfavorable execution.

---

## 3. Internal Bluechip Price Oracle

The oracle provides USD pricing for bluechip tokens. It is used during commits to convert bluechip amounts to USD values.

### Architecture

The oracle lives in the **factory contract** and is queried by individual pool contracts via cross-contract calls (`get_usd_value` / `get_bluechip_value`).

### Price Sources

The oracle aggregates prices from two sources:

1. **ATOM/Bluechip anchor pool** — A dedicated pool that establishes the bluechip/ATOM exchange rate. This pool receives 2x weight in the price calculation.
2. **Creator pools** — Up to 5 randomly selected creator pools that contain bluechip tokens and have sufficient liquidity (≥ 10,000,000,000 units).

The ATOM/USD price comes from the **Pyth Network** oracle, providing an external price feed.

### TWAP (Time-Weighted Average Price)

The oracle uses a Uniswap V2-style TWAP mechanism:

1. **Price accumulators**: Each pool maintains cumulative price accumulators (`price0_cumulative_last`, `price1_cumulative_last`) that are updated on every swap. The accumulator adds `(reserve_ratio * time_elapsed)` at each update.

2. **Cumulative snapshots**: The factory stores a snapshot of each selected pool's cumulative accumulator. On the next oracle update, the TWAP is calculated as:
   ```
   twap = (current_cumulative - previous_cumulative) / time_elapsed
   ```

3. **Observation window**: TWAP observations are retained for a 1-hour window (`TWAP_WINDOW = 3600s`). Observations older than this are discarded.

4. **Final TWAP**: The overall TWAP is a time-weighted average of all observations within the window.

### Update Cycle

| Parameter | Value | Description |
|-----------|-------|-------------|
| `UPDATE_INTERVAL` | 300s (5 min) | Minimum time between oracle updates |
| `TWAP_WINDOW` | 3600s (1 hour) | Observation retention window |
| `ROTATION_INTERVAL` | 3600s (1 hour) | How often pool selection is refreshed |
| `ORACLE_POOL_COUNT` | 5 | Number of pools sampled per rotation |

**Update flow:**
1. Anyone calls `UpdateOraclePrice` on the factory (permissionless, rate-limited to every 5 minutes)
2. If the rotation interval has elapsed, new pools are randomly selected
3. Each selected pool's cumulative accumulator is read and compared to the previous snapshot
4. A liquidity-weighted average price is calculated across all pools
5. The new observation is added to the TWAP window; old observations are pruned
6. The final TWAP price is stored as `last_price`

### Pool Selection

Pools are selected pseudo-randomly using a deterministic hash:
```
seed = SHA256(block_time || block_height || pool_count)
```

Only pools meeting these criteria are eligible:
- Contains a bluechip token
- Has total liquidity ≥ `MIN_POOL_LIQUIDITY` (10B units)
- Is not the ATOM/bluechip anchor pool (which is always included separately)

### Manipulation Resistance

The TWAP design provides several protections:

- **Cross-block averaging**: Price accumulators use reserves from *before* the current swap, so an attacker must hold a skewed position across multiple blocks to affect the TWAP.
- **Multi-pool aggregation**: Price is averaged across up to 5 pools weighted by liquidity, making single-pool manipulation expensive.
- **ATOM pool 2x weight**: The anchor pool has double weight, providing a stable reference point.
- **Observation window**: The 1-hour TWAP window smooths out short-term volatility.
- **Random rotation**: Pool selection changes hourly, preventing targeted manipulation of specific pools.

### Limitations

- **Update lag**: The oracle only updates when someone calls `UpdateOraclePrice`. Between updates (up to 5 minutes), the price is stale. During volatile markets, committed amounts may be valued higher or lower than the real-time market price.
- **TWAP smoothing**: By design, TWAP lags sudden price movements. A 50% price drop will take multiple update cycles to fully reflect in the oracle price.
- **First observation**: When pools are newly rotated in, the first price reading uses spot reserves (not TWAP) as a baseline. TWAP protection only kicks in after the second observation.

---

## 4. Threshold Crossing and Token Distribution

When `USD_RAISED_FROM_COMMIT` reaches `commit_amount_for_threshold_usd`, the pool crosses threshold:

### Token Minting (1.5 trillion total supply, 1.2 trillion allocated at threshold)

| Allocation | Amount | Recipient |
|------------|--------|-----------|
| Creator reward | Configured at pool creation | Creator wallet |
| BlueChip reward | Configured at pool creation | BlueChip wallet |
| Pool seed liquidity | Configured at pool creation | Pool contract (AMM reserves) |
| Committer distribution | Configured at pool creation | Pro-rata to all committers |

The four allocations must sum to exactly 1,200,000,000,000 (1.2T) units (validated at pool instantiation and again at threshold crossing).

### Committer Distribution Formula

Each committer receives creator tokens proportional to their USD contribution:

```
reward = (user_usd_committed / total_committed_usd) * commit_return_amount
```

Where:
- `user_usd_committed` = sum of all `usd_value` entries for this user in `COMMIT_LEDGER`
- `total_committed_usd` = `commit_amount_for_threshold_usd` from pool config
- `commit_return_amount` = the portion of 1.2T tokens allocated for committer rewards

### Batched Distribution

If there are more than 40 committers, distribution is batched:
- Each batch processes up to 40 committers
- Batches are triggered by external `ContinueDistribution` calls (permissionless)
- Distribution state tracks progress via `DISTRIBUTION_STATE`
- If distribution stalls (1 hour timeout or 5 consecutive failures), an admin can restart it via `RecoverStuckStates`

---

## 5. Seed Liquidity

After threshold crossing, the pool's AMM reserves are seeded:

```
reserve0 (bluechip) = min(NATIVE_RAISED_FROM_COMMIT * (1 - fee_rate), max_bluechip_lock_per_pool)
reserve1 (creator token) = pool_seed_amount  // from threshold payout config
```

If the bluechip amount exceeds `max_bluechip_lock_per_pool`, the excess bluechip and proportional creator tokens are held in a `CREATOR_EXCESS_POSITION`, locked for `creator_excess_liquidity_lock_days` (default: 7 days).

**Virtual base liquidity**: `total_liquidity = sqrt(reserve0 * reserve1)` is set with no position assigned. This is a standard defense (similar to Uniswap V2's minimum liquidity lock) that prevents the first LP depositor from inflating their share against the seed reserves. This small amount of liquidity is permanently unowned.
