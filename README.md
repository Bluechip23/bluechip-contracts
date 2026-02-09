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

The protocol consists of three main contracts:

```
┌─────────────────────────────────────────────────────────────┐
│                      FACTORY CONTRACT                        │
│  - Creates new creator pools                                 │
│  - Manages global configuration                              │
│  - Handles CW20 and CW721 contract instantiation            │
│  - Internal oracle for BLUECHIP/USD pricing                  │
│  - Triggers expand economy on pool creation                  │
└─────────────────────────────────────────────────────────────┘
                    │                       │
                    │ creates               │ requests expansion
                    ▼                       ▼
┌──────────────────────────────┐  ┌──────────────────────────────┐
│        POOL CONTRACT         │  │    EXPAND ECONOMY CONTRACT   │
│  - Handles commits           │  │  - Mints BLUECHIP tokens on  │
│  - Manages threshold         │  │    pool creation             │
│  - Executes swaps & LP ops   │  │  - Decreasing supply curve   │
│  - Mints NFT LP positions    │  │  - Factory-gated access      │
│  - Distributes fees          │  │  - Owner admin functions     │
│  - Creator excess liquidity  │  │                              │
└──────────────────────────────┘  └──────────────────────────────┘
```

---

## How It Works

### Creating a Pool

Creators can launch their own token pool by calling the factory contract. Calling the contract can be done via the BlueChip website or by using the below JSON.

```json
{
  "create": {
    "pair_msg": {
      "asset_infos": [
        { "native_token": { "denom": "bluechip" } },
        { "token": { "contract_addr": "CREATED_BY_FACTORY" } }
      ],
      "token_code_id": 1,
      "factory_addr": "factory_contract_address",
      "fee_info": {
        "bluechip_address": "protocol_wallet",
        "creator_address": "creator_wallet",
        "bluechip_fee": "0.01",
        "creator_fee": "0.05"
      },
      "commit_limit_usd": "25000000000",
      "oracle_addr": "pyth_oracle_address"
    },
    "token_info": {
      "name": "Creator Token Name",
      "symbol": "TICKER",
      "decimal": 6
    }
  }
}
```

Each pool receives:
- A unique CW20 token for the creator (mint cap: 1.5T)
- A CW721 NFT contract for liquidity positions
- Configurable fee structure (default: 1% protocol + 5% creator)
- BLUECHIP tokens minted via the Expand Economy contract (up to 500M per creation, decreasing over time)

---

## Two-Phase Pool Lifecycle

### Phase 1: Pre-Threshold (Funding Phase)

Before a pool reaches its $25,000 USD threshold, only **commit transactions** are allowed. This phase:

- Tracks all commits in a ledger by USD value (not token quantity)
- Provides price stability during the funding period
- Ensures fair valuation regardless of when users commit
- Prevents liquidity provision and normal swaps

**During this phase:**
- Users send BLUECHIP tokens to subscribe/commit
- Commits are tracked by their USD value at time of commitment
- 6% fee is collected (1% protocol + 5% creator)
- All committers are recorded for proportional token distribution

### Threshold Crossing

When total USD committed reaches the threshold ($25,000,000,000 default):

1. **Creator tokens minted**: ~1T creator tokens are minted and distributed
2. **Creator reward**: 500B creator tokens sent to the creator's wallet
3. **Protocol reward**: 500M creator tokens sent to the Bluechip protocol wallet
4. **Pool seeded**: 2B creator tokens + committed BLUECHIP used to initialize AMM liquidity
5. **Committer distribution**: 500B creator tokens distributed to committers proportionally
6. **Excess handling**: If BLUECHIP exceeds `max_bluechip_lock_per_pool`, excess is time-locked for the creator (see [Creator Limits](#creator-limits--excess-liquidity))
7. **State transition**: Pool moves to active trading phase

```
Token Distribution Formula:
user_tokens = (user_usd_contribution / total_usd_committed) × 500,000,000,000
```

### Phase 2: Post-Threshold (Active Trading)

Once the threshold is crossed, the pool operates as a full AMM:

**Available Operations:**
- **Commits (Subscriptions)**: Still available with 6% fee, provides subscription tracking
- **Simple Swaps**: Standard AMM swaps with LP fees only (no protocol fees)
- **Add Liquidity**: Provide liquidity and receive NFT position
- **Remove Liquidity**: Withdraw liquidity (partial or full)
- **Collect Fees**: Claim accumulated trading fees without burning position

---

## The Commit Function (Subscribe Button)

The commit function is the core user interaction for subscriptions:

```json
{
  "commit": {
    "pool_id": "pool_contract_address"
  }
}
```

**Send with:** Native BLUECHIP tokens attached to the transaction. Commit transactions can only be carried out with BLUECHIP tokens.

### What Happens When You Commit

**Pre-Threshold:**
1. USD value calculated using oracle price
2. 6% fee deducted and distributed (1% protocol, 5% creator)
3. Commitment recorded in ledger
4. If threshold crossed, triggers token distribution

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

Unlike traditional AMMs that issue fungible LP tokens, Bluechip uses NFTs to represent liquidity positions.

### Benefits of NFT Positions

- **Fee Collection Without Burning**: Claim accumulated fees while keeping your position
- **Transferable Positions**: Sell or transfer your liquidity position as an NFT
- **Position Tracking**: Each position tracks its own fee accumulation history
- **Partial Withdrawals**: Remove part of your liquidity while keeping the NFT

### Adding Liquidity

```json
{
  "add_liquidity": {
    "amount0": "1000000",
    "amount1": "1000000",
    "min_liquidity": "900000"
  }
}
```

**Returns:** NFT representing your liquidity position

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

### Removing Liquidity

```json
{
  "remove_liquidity": {
    "position_id": "123",
    "liquidity_amount": "500000"
  }
}
```

Partial removal keeps the NFT; full removal burns it.

---

## Internal Oracle System

Bluechip uses an internal oracle to price the native BLUECHIP token in USD.

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    PYTH ORACLE                               │
│                   (ATOM/USD Price)                           │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                 ATOM/BLUECHIP POOL                           │
│              (Primary Price Reference)                       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│               INTERNAL ORACLE (FACTORY)                      │
│  - Weighted average from multiple pools                      │
│  - TWAP for manipulation resistance                          │
│  - Random pool rotation for security                         │
└─────────────────────────────────────────────────────────────┘
```

### Price Calculation

1. **ATOM/BLUECHIP** price from the primary liquidity pool
2. **ATOM/USD** price from Pyth Network oracle
3. **BLUECHIP/USD** = ATOM/USD × ATOM/BLUECHIP

### Manipulation Resistance

- **TWAP (Time-Weighted Average Price)**: Smooths out temporary price spikes
- **Pool Rotation**: Randomly selects pools to prevent targeted manipulation
- **Liquidity Weighting**: Higher liquidity pools have more influence
- **Outlier Detection**: Rejects prices that deviate significantly from TWAP

---

## Query Endpoints

### Pool State

```json
{
  "get_pool_state": {}
}
```

**Returns:**
```json
{
  "reserve0": "1000000000",
  "reserve1": "5000000000",
  "total_liquidity": "2000000000",
  "is_threshold_hit": true
}
```

### Commit Status

```json
{
  "get_commit_status": {}
}
```

**Returns:**
```json
{
  "total_usd_raised": "25000000000",
  "threshold": "25000000000",
  "is_active": true
}
```

### Position Info

```json
{
  "get_position": {
    "position_id": "123"
  }
}
```

**Returns:**
```json
{
  "position_id": "123",
  "owner": "cosmos1...",
  "liquidity": "1000000",
  "fee_growth_inside_0_last": "0.001",
  "fee_growth_inside_1_last": "0.002"
}
```

### Simulate Swap

```json
{
  "simulate_swap": {
    "offer_asset": {
      "native_token": { "denom": "bluechip" }
    },
    "offer_amount": "1000000"
  }
}
```

---

## Integration Guide

### Embedding the Commit Button

The commit function can be called from any frontend that supports Cosmos transactions:

```javascript
// Using CosmJS
const msg = {
  commit: {
    pool_id: "pool_contract_address"
  }
};

const result = await client.execute(
  senderAddress,
  poolContractAddress,
  msg,
  "auto",
  undefined,
  [{ denom: "bluechip", amount: "1000000" }]
);
```

### Checking Subscription Status

Query the commit ledger to verify subscription status:

```json
{
  "get_commit_info": {
    "address": "cosmos1..."
  }
}
```

---

## Security Considerations

### Reentrancy Protection
- Guard implemented on commit transactions
- State updates occur before external calls

### Oracle Security
- TWAP (3600-second window) prevents flash loan price manipulation
- Multiple pool sampling (5 pools: 4 random + ATOM anchor) reduces single-point-of-failure risk
- Minimum liquidity requirement (10B) for oracle-eligible pools
- Price update rate-limited to every 300 seconds
- Stale price detection (3000-second max age)
- Pool rotation every 3600 seconds to prevent targeted manipulation

### Threshold Mechanics
- Threshold can only be crossed once (irreversible)
- Atomic state transitions during threshold crossing
- USD-based tracking prevents token price manipulation
- Batched distribution for large committer sets (>40)
- Stuck state recovery mechanism (factory admin, after 1 hour timeout)

### Commit Rate Limiting
- Minimum 13 seconds between commits per wallet
- USD payment tolerance of 1% (100 bps) for oracle price drift

### Payout Integrity Validation
- All threshold payout components validated (no zero amounts)
- Total payout capped at 10T tokens
- No individual component can exceed total

---

## Token Economics

### Creator Token Supply

Each creator pool mints a total of **1,002,500,000,000** (≈1T) creator tokens at threshold crossing, distributed as follows:

| Recipient | Amount | % of Total | Purpose |
|-----------|--------|------------|---------|
| Committers | 500,000,000,000 | ~49.9% | Proportional to USD committed |
| Creator | 500,000,000,000 | ~49.9% | Creator reward |
| Protocol (Bluechip Wallet) | 500,000,000 | ~0.05% | Protocol sustainability |
| Pool Liquidity Seed | 2,000,000,000 | ~0.2% | Initial AMM liquidity |

The CW20 token contract is instantiated with a mint cap of **1,500,000,000,000** (1.5T), allowing for future controlled minting beyond the initial threshold distribution.

### Fee Flow

```
Commit Transaction (5000 BLUECHIP)
        │
        ├── 1% (50) → Protocol Wallet
        ├── 5% (250) → Creator Wallet
        └── 94% (4700) → Pool/Swap
```

---

## Expand Economy

The Expand Economy contract manages BLUECHIP token inflation by minting new tokens each time a creator pool is created. This incentivizes early adoption while gradually reducing emissions as the ecosystem grows.

### How It Works

```
┌──────────────┐         ┌───────────────────┐         ┌──────────────────┐
│   Creator    │ ──────► │  Factory Contract  │ ──────► │  Expand Economy  │
│ creates pool │         │  calculate_mint()  │         │  RequestExpansion│
└──────────────┘         └───────────────────┘         └──────────────────┘
                                                               │
                                                               ▼
                                                    Mints BLUECHIP tokens
                                                    to protocol wallet
```

1. A creator calls the factory to create a new pool
2. The factory calculates a mint amount based on time elapsed and total pools created
3. The factory sends a `RequestExpansion` message to the Expand Economy contract
4. The Expand Economy contract sends the calculated amount of BLUECHIP (`stake`) tokens to the protocol wallet

### Mint Formula

```
mint_amount = 500 - ((5x² + x) / ((s / 6) + 333x))
```

Where:
- **x** = total pools created
- **s** = seconds elapsed since the first pool was created
- **Result** is in whole tokens (multiplied by 10⁶ for micro-denomination)

**Properties:**
- **Maximum mint**: 500,000,000 (500M) `stake` tokens per pool creation
- **Decreasing curve**: Mint amount decreases as more pools are created
- **Time decay**: Longer time between pool creations further reduces the mint
- **Floor**: Mint amount cannot go below zero

### Access Control

| Action | Who Can Call |
|--------|-------------|
| `RequestExpansion` | Factory contract only |
| `UpdateConfig` | Owner only |
| `Withdraw` | Owner only |

### Query Endpoints

```json
{ "get_config": {} }
```
Returns the factory address and owner.

```json
{ "get_balance": { "denom": "stake" } }
```
Returns the contract's balance of the specified denomination.

---

## Creator Limits & Excess Liquidity

### Maximum Bluechip Lock Per Pool

Each pool enforces a maximum amount of BLUECHIP tokens that can be locked as liquidity (`max_bluechip_lock_per_pool`). When the BLUECHIP tokens committed to a pool exceed this limit at threshold crossing, the excess is not lost — it is held in a time-locked escrow for the creator.

### Creator Excess Liquidity

When bluechips exceed the per-pool maximum:

1. The excess BLUECHIP and proportional creator tokens are stored in a `CreatorExcessLiquidity` record
2. An unlock timestamp is set based on `creator_excess_liquidity_lock_days` (configured at the factory level)
3. After the lock period expires, the creator can claim the excess

```
Threshold Crossing (e.g., 15B BLUECHIP committed, 10B max per pool)
        │
        ├── 10B BLUECHIP → Pool liquidity (immediate)
        └── 5B BLUECHIP + proportional creator tokens → Locked
                │
                └── Unlocks after X days → Creator calls ClaimCreatorExcessLiquidity
```

### Claiming Excess Liquidity

```json
{
  "claim_creator_excess_liquidity": {}
}
```

**Requirements:**
- Caller must be the creator of the pool
- The lock period must have expired
- Can only be claimed once

### Configuration Defaults

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_bluechip_lock_per_pool` | 10,000,000,000 (10B) | Max BLUECHIP tokens locked as liquidity in a single pool |
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

### Recovery

If distribution gets stuck (>1 hour or 5+ consecutive failures), the factory admin can trigger `RecoverStuckStates` to resume processing.

---

## Admin Operations

### Factory Configuration Updates

Configuration updates use a timelock mechanism:

1. Admin calls `ProposeConfigUpdate` with new values
2. A 1-second timelock is applied
3. After the timelock, `UpdateConfig` applies the pending changes

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

- Processes up to 10 pools per transaction
- Automatically continues with `ContinuePoolUpgrade` until all pools are migrated
- Can target specific pools or upgrade all pools

### Pool Pause/Unpause

The factory admin can pause individual pools, disabling all swap and liquidity operations while preserving state.

---

## Key Constants & Limits

| Parameter | Value | Description |
|-----------|-------|-------------|
| Commit threshold (USD) | 25,000,000,000 | USD value required to activate pool |
| Commit threshold (BLUECHIP) | 100,000,000 | BLUECHIP token threshold |
| Creator token mint cap | 1,500,000,000,000 | Max CW20 supply per pool |
| Max BLUECHIP lock per pool | 10,000,000,000 | Excess is time-locked for creator |
| Creator excess lock period | 7 days | Time before creator can claim excess |
| Commit fee (protocol) | 1% | Sent to Bluechip wallet |
| Commit fee (creator) | 5% | Sent to creator wallet |
| LP swap fee | 0.3% | Distributed to liquidity providers |
| Min commit interval | 13 seconds | Rate limit per wallet |
| Expand economy max mint | 500,000,000 | Max BLUECHIP minted per pool creation |
| Oracle TWAP window | 3600 seconds | Time-weighted average price window |
| Oracle update interval | 300 seconds | Min time between price updates |
| Oracle price max age | 3000 seconds | Price considered stale after this |
| Min pool liquidity | 1,000 | Liquidity required to unpause pool |
| Distribution batch size | 40 | Max committers per distribution tx |
| Default slippage | 0.5% | Default max slippage for swaps |
| Max slippage | 50% | Hard cap on swap slippage |

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

### Testing

```bash
cargo test
```

### Deployment Order

1. Deploy CW20-base contract (store code)
2. Deploy CW721-base contract (store code)
3. Deploy Expand Economy contract
4. Deploy Factory contract with code IDs and Expand Economy address
5. Create ATOM/BLUECHIP oracle pool first
6. Initialize internal oracle
7. Creators can now create pools

### Mainnet Deployment

To deploy to Bluechip Mainnet:

1. Configure your wallet (ensure you have `bluechipd` CLI tool)
2. Run the deployment script:
   ```bash
   ./deploy_mainnet.sh
   ```
3. Update specific configurations in `deploy_mainnet.sh` (Oracle address, Price Feed ID) if necessary.

---


## Links

- Documentation: [docs link]
- Discord: [discord link]
- Twitter: [twitter link]
