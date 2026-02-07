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

The protocol consists of two main contracts:

```
┌─────────────────────────────────────────────────────────────┐
│                      FACTORY CONTRACT                        │
│  - Creates new creator pools                                 │
│  - Manages global configuration                              │
│  - Handles CW20 and CW721 contract instantiation            │
│  - Internal oracle for BLUECHIP/USD pricing                  │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ creates
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                       POOL CONTRACT                          │
│  - Handles commits (subscriptions)                           │
│  - Manages threshold mechanics                               │
│  - Executes swaps and liquidity operations                   │
│  - Mints NFT liquidity positions                             │
│  - Distributes fees to creators                              │
└─────────────────────────────────────────────────────────────┘
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
- A unique CW20 token for the creator
- A CW721 NFT contract for liquidity positions
- Configurable fee structure (default: 1% protocol + 5% creator)

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

When total USD committed reaches $25,000:

1. **Token Distribution**: All committers receive creator tokens proportional to their USD contribution
2. **Pool Initialization**: The AMM is seeded with initial liquidity
3. **State Transition**: Pool moves to active trading phase
4. **Creators are sent an allotted amount of creator tokens**
5. **The protocol is sent an allotted amount of creator tokens**

```
Token Distribution Formula:
user_tokens = (user_usd_contribution × total_reward_pool) ÷ total_usd_raised
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
- TWAP prevents flash loan price manipulation
- Multiple pool sampling reduces single-point-of-failure risk
- Minimum liquidity requirements for oracle pools

### Threshold Mechanics
- Threshold can only be crossed once (irreversible)
- Atomic state transitions during threshold crossing
- USD-based tracking prevents token price manipulation

---

## Token Economics

### Token Distribution at Threshold

| Recipient | Allocation | Purpose |
|-----------|------------|---------|
| Committers | ~35% | Proportional to USD committed |
| Creator | ~28% | Creator reward |
| Protocol | Variable | Protocol sustainability |
| Pool Liquidity | ~37% | Initial AMM liquidity |

### Fee Flow

```
Commit Transaction (5000 BLUECHIP)
        │
        ├── 1% (50) → Protocol Wallet
        ├── 5% (250) → Creator Wallet
        └── 94% (4700) → Pool/Swap
```

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
3. Deploy Factory contract with code IDs
4. Create ATOM/BLUECHIP oracle pool first
5. Initialize internal oracle
6. Creators can now create pools

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

- Website: https://www.bluechip.link/home
- Discord: https://discord.gg/gfdWgHFY
- Twitter: https://x.com/BlueChipCreate
