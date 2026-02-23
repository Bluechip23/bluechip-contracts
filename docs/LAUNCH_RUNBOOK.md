# Bluechip Mainnet Launch Runbook

Step-by-step guide for launching the Bluechip chain and deploying all contracts to production.

---

## Prerequisites

Before starting, ensure you have:

- [ ] `bluechipChaind` binary built and tested
- [ ] Genesis file finalized (validators, initial token distribution)
- [ ] Validator set coordinated and ready
- [ ] Docker installed (for contract optimization)
- [ ] Pyth Network oracle contract address for your chain (or mock oracle for initial testing)
- [ ] Admin wallet funded with sufficient `ubluechip` for gas fees
- [ ] Deployment wallet key added: `bluechipChaind keys add deployer`

### Environment Variables

Set these before running any commands:

```bash
export BIN="bluechipChaind"
export CHAIN_ID="bluechip-1"
export NODE="https://bluechip.rpc.bluechip.link"
export DEPLOYER=$(bluechipChaind keys show deployer -a)
export TX_FLAGS="--chain-id $CHAIN_ID --node $NODE --gas auto --gas-adjustment 1.5 --fees 50000ubluechip -y --output json"
```

---

## Step 1: Launch Chain

Start the Bluechip Cosmos chain with coordinated validators.

```bash
# Initialize the chain (done once during genesis)
bluechipChaind init <moniker> --chain-id bluechip-1

# Start the node
bluechipChaind start --home $HOME/.bluechip
```

**Verify the chain is producing blocks:**

```bash
bluechipChaind status --node $NODE --output json | python3 -c "
import json, sys
d = json.load(sys.stdin)
si = d.get('sync_info', d.get('SyncInfo', {}))
h = si.get('latest_block_height', si.get('LatestBlockHeight', 0))
print(f'Block height: {h}')
"
```

**Done when:** Block height is advancing steadily.

---

## Step 2: Keplr & Cosmos Chain Registry

### 2a. Submit to Cosmos Chain Registry

Submit a PR to [cosmos/chain-registry](https://github.com/cosmos/chain-registry) with your chain info. The key values:

```json
{
  "chain_name": "bluechip",
  "chain_id": "bluechip-1",
  "bech32_prefix": "bluechip",
  "slip44": 118,
  "fees": {
    "fee_tokens": [
      {
        "denom": "ubluechip",
        "fixed_min_gas_price": 0.01,
        "low_gas_price": 0.01,
        "average_gas_price": 0.025,
        "high_gas_price": 0.04
      }
    ]
  },
  "staking": {
    "staking_tokens": [{ "denom": "ubluechip" }]
  }
}
```

### 2b. Keplr Wallet Configuration

Until the chain registry PR is merged, use `experimentalSuggestChain` in the frontend. The config is already defined in `frontend/src/components/WalletConnect.tsx` (the `connectMainnet` function):

```typescript
// Key values that must match your actual chain:
chainId: "bluechip-1",
chainName: "Bluechip Mainnet",
rpc: "https://bluechip.rpc.bluechip.link",
rest: "https://bluechip.api.bluechip.link",
bech32Config: {
    bech32PrefixAccAddr: "bluechip",
    // ...
},
currencies: [{
    coinDenom: "BLUECHIP",
    coinMinimalDenom: "ubluechip",
    coinDecimals: 6,
}],
```

**Done when:** You can connect Keplr to `bluechip-1` and see your balance.

---

## Step 3: Build & Optimize Contracts

Build all contracts with the reproducible optimizer before uploading.

```bash
# From the repo root — optimizes all workspace contracts
./optimize.sh

# Or individually:
make optimize-pool
make optimize-factory
```

This produces optimized `.wasm` files in the `artifacts/` directory.

**Verify builds:**

```bash
ls -la artifacts/
# Expected:
#   cw20_base.wasm
#   cw721_base.wasm
#   pool.wasm
#   expand_economy.wasm  (or expand-economy.wasm)
#   factory.wasm
#   oracle.wasm          (mock — only if using mock oracle)
```

---

## Step 4: Store WASM Code On-Chain

Upload each contract's bytecode. Save the returned **code IDs** — you'll need them for instantiation.

### 4a. Store CW20 Base (fungible token standard)

```bash
$BIN tx wasm store artifacts/cw20_base.wasm \
  --from deployer $TX_FLAGS
```

Query the tx to get the code ID:

```bash
# After tx confirms:
$BIN query tx <TXHASH> --node $NODE --output json | python3 -c "
import json, sys
d = json.load(sys.stdin)
for e in d.get('events', []):
    for a in e.get('attributes', []):
        if a.get('key') == 'code_id':
            print('CW20_CODE=' + a['value']); exit()
"
```

```bash
export CW20_CODE=<returned_code_id>
```

### 4b. Store CW721 Base (NFT standard)

```bash
$BIN tx wasm store artifacts/cw721_base.wasm \
  --from deployer $TX_FLAGS
```

```bash
export CW721_CODE=<returned_code_id>
```

### 4c. Store Pool Contract

```bash
$BIN tx wasm store artifacts/pool.wasm \
  --from deployer $TX_FLAGS
```

```bash
export POOL_CODE=<returned_code_id>
```

### 4d. Store Expand Economy Contract

```bash
$BIN tx wasm store artifacts/expand_economy.wasm \
  --from deployer $TX_FLAGS
```

```bash
export EXP_CODE=<returned_code_id>
```

### 4e. Store Factory Contract

```bash
$BIN tx wasm store artifacts/factory.wasm \
  --from deployer $TX_FLAGS
```

```bash
export FACTORY_CODE=<returned_code_id>
```

### 4f. (Optional) Store Oracle Contract

Only if using the mock oracle for initial testing. On mainnet you'll use Pyth Network's deployed contract.

```bash
$BIN tx wasm store artifacts/oracle.wasm \
  --from deployer $TX_FLAGS
```

```bash
export ORACLE_CODE=<returned_code_id>
```

**Checkpoint — record all code IDs:**

```bash
echo "CW20=$CW20_CODE  CW721=$CW721_CODE  POOL=$POOL_CODE  EXP=$EXP_CODE  FACTORY=$FACTORY_CODE"
```

---

## Step 5: Deploy Expand Economy Contract

Expand Economy must be deployed **before** the Factory, because the Factory references it at instantiation. However, Expand Economy also needs the Factory address for access control — so we use a two-step process:

1. Instantiate Expand Economy with the deployer as a temporary `factory_address`
2. After Factory is deployed, update Expand Economy to point to the real Factory

```bash
$BIN tx wasm instantiate $EXP_CODE \
  "{\"factory_address\":\"$DEPLOYER\",\"owner\":\"$DEPLOYER\"}" \
  --label "bluechip-expand-economy" \
  --admin $DEPLOYER \
  --from deployer $TX_FLAGS
```

Get the contract address:

```bash
export EXP_ADDR=<returned_contract_address>
echo "Expand Economy: $EXP_ADDR"
```

---

## Step 6: Deploy Factory Contract (Mock Mode)

The Factory needs all code IDs and the Expand Economy address. This is the central contract that everything else revolves around.

**Critical: Mock Mode.** The Factory is deployed with `atom_bluechip_anchor_pool_address` set to `$DEPLOYER` (the admin address). The Factory code detects this (`anchor_pool == admin`) and enters "mock mode" which:

- **Bypasses the internal oracle** — uses raw Pyth ATOM/USD price instead (see `internal_bluechip_price_oracle.rs:633`)
- **Skips Expand Economy minting** on pool creation (see `mint_bluechips_pool_creation.rs:71`)
- **Returns mock pool list** for oracle sampling (see `internal_bluechip_price_oracle.rs:74`)

This allows the Factory to function and create pools (including the anchor pool itself) before the oracle infrastructure is ready.

```bash
FACTORY_MSG=$(python3 -c "
import json
print(json.dumps({
    'factory_admin_address':              '$DEPLOYER',
    'commit_amount_for_threshold_bluechip': '0',
    'commit_threshold_limit_usd':         '25000',
    'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
    'pyth_atom_usd_price_feed_id':        'ATOM_USD',
    'cw20_token_contract_id':             int('$CW20_CODE'),
    'cw721_nft_contract_id':              int('$CW721_CODE'),
    'create_pool_wasm_contract_id':       int('$POOL_CODE'),
    'bluechip_wallet_address':            '$BLUECHIP_WALLET',
    'commit_fee_bluechip':                '0.01',
    'commit_fee_creator':                 '0.05',
    'max_bluechip_lock_per_pool':         '10000000000',
    'creator_excess_liquidity_lock_days': 7,
    'atom_bluechip_anchor_pool_address':  '$DEPLOYER',
    'bluechip_mint_contract_address':     '$EXP_ADDR',
}))
")

$BIN tx wasm instantiate $FACTORY_CODE "$FACTORY_MSG" \
  --label "bluechip-factory" \
  --admin $DEPLOYER \
  --from deployer $TX_FLAGS
```

```bash
export FACTORY_ADDR=<returned_contract_address>
echo "Factory: $FACTORY_ADDR"
```

**Important variables to set before running:**

| Variable | Description | Example |
|----------|-------------|---------|
| `$ORACLE_ADDR` | Pyth oracle contract on your chain (or mock oracle address) | `bluechip1abc...` |
| `$BLUECHIP_WALLET` | Protocol treasury/revenue wallet | `bluechip1xyz...` |

### 6a. Update Expand Economy to Point to Real Factory

Now that the Factory is deployed, update Expand Economy's access control:

```bash
$BIN tx wasm execute $EXP_ADDR \
  "{\"update_config\":{\"factory_address\":\"$FACTORY_ADDR\",\"owner\":null}}" \
  --from deployer $TX_FLAGS
```

**Verify:**

```bash
$BIN query wasm contract-state smart $EXP_ADDR '{"get_config":{}}' \
  --node $NODE --output json
# factory_address should now equal $FACTORY_ADDR
```

---

## Step 7: Launch Hermes (IBC Relayer)

Hermes relays IBC packets between the Cosmos Hub (for ATOM) and Bluechip. This is required so ATOM can flow to your chain for the anchor pool.

### 7a. Install Hermes

```bash
# Download latest release from https://github.com/informalsystems/hermes/releases
# Or build from source:
cargo install ibc-relayer-cli --bin hermes
```

### 7b. Configure Hermes

Create `~/.hermes/config.toml`:

```toml
[global]
log_level = 'info'

[mode]
[mode.clients]
enabled = true
refresh = true
misbehaviour = true

[mode.connections]
enabled = false

[mode.channels]
enabled = false

[mode.packets]
enabled = true
clear_interval = 100
clear_on_start = true
tx_confirmation = true

# Cosmos Hub
[[chains]]
id = 'cosmoshub-4'
rpc_addr = 'https://rpc.cosmos.network:443'
grpc_addr = 'https://grpc.cosmos.network:443'
account_prefix = 'cosmos'
key_name = 'relayer-cosmos'
store_prefix = 'ibc'
gas_price = { price = 0.025, denom = 'uatom' }
max_gas = 400000
clock_drift = '5s'
trusting_period = '14days'

# Bluechip
[[chains]]
id = 'bluechip-1'
rpc_addr = 'https://bluechip.rpc.bluechip.link:443'
grpc_addr = 'https://bluechip.grpc.bluechip.link:443'
account_prefix = 'bluechip'
key_name = 'relayer-bluechip'
store_prefix = 'ibc'
gas_price = { price = 0.025, denom = 'ubluechip' }
max_gas = 400000
clock_drift = '5s'
trusting_period = '14days'
```

### 7c. Add Relayer Keys

```bash
hermes keys add --chain cosmoshub-4 --mnemonic-file /path/to/cosmos-relayer-mnemonic.txt
hermes keys add --chain bluechip-1 --mnemonic-file /path/to/bluechip-relayer-mnemonic.txt
```

### 7d. Create IBC Channel

```bash
# Create client, connection, and channel between Cosmos Hub and Bluechip
hermes create channel \
  --a-chain cosmoshub-4 \
  --b-chain bluechip-1 \
  --a-port transfer \
  --b-port transfer \
  --new-client-connection
```

### 7e. Start the Relayer

```bash
hermes start
```

**Done when:** You can send ATOM from Cosmos Hub to Bluechip via IBC transfer and see the IBC denom appear on your chain.

The IBC ATOM denom on Bluechip will look like: `ibc/<HASH>` (e.g., `ibc/27394FB092D2ECCD56123C74F36E4C1F926001CEADA9CA97EA622B25F41E5EB2`).

```bash
# Verify IBC ATOM arrived
$BIN query bank balances $DEPLOYER --node $NODE --output json
```

---

## Step 8: Create ATOM/Bluechip Anchor Pool (via Factory)

The Factory is already running in **mock mode** (Step 6), so it can create pools without a functioning oracle. We use this window to create the ATOM/Bluechip anchor pool — the very pool the oracle will depend on.

This is the **most critical pool** — it provides the price reference that the internal oracle uses:

```
Pyth ATOM/USD  →  ATOM/Bluechip pool  →  Bluechip/USD price
```

### 8a. Create the Anchor Pool

Create a pool with `is_standard_pool: true` to mark it as the oracle anchor:

```bash
ANCHOR_MSG=$(python3 -c "
import json
print(json.dumps({
    'create': {
        'pool_msg': {
            'pool_token_info': [
                {'bluechip': {'denom': 'ubluechip'}},
                {'bluechip': {'denom': 'ibc/<ATOM_IBC_DENOM_HASH>'}}
            ],
            'cw20_token_contract_id':          int('$CW20_CODE'),
            'factory_to_create_pool_addr':     '$FACTORY_ADDR',
            'threshold_payout':                None,
            'commit_fee_info': {
                'bluechip_wallet_address':     '$BLUECHIP_WALLET',
                'creator_wallet_address':      '$DEPLOYER',
                'commit_fee_bluechip':         '0.01',
                'commit_fee_creator':          '0.05',
            },
            'creator_token_address':           '$DEPLOYER',
            'commit_amount_for_threshold':     '0',
            'commit_limit_usd':                '25000',
            'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
            'pyth_atom_usd_price_feed_id':    'ATOM_USD',
            'max_bluechip_lock_per_pool':      '10000000000',
            'creator_excess_liquidity_lock_days': 7,
            'is_standard_pool':                True,
        },
        'token_info': {'name': 'ATOM-BLUECHIP LP', 'symbol': 'ATOMBLU', 'decimal': 6},
    }
}))
")

$BIN tx wasm execute $FACTORY_ADDR "$ANCHOR_MSG" \
  --from deployer $TX_FLAGS
```

Get the anchor pool address:

```bash
$BIN query wasm list-contract-by-code $POOL_CODE --node $NODE --output json | python3 -c "
import json, sys
cs = json.load(sys.stdin).get('contracts', [])
print('Anchor Pool: ' + (cs[-1] if cs else 'ERR'))
"
```

```bash
export ANCHOR_POOL=<returned_pool_address>
```

### 8b. Seed the Anchor Pool with Initial Liquidity

The anchor pool needs real ATOM and BLUECHIP liquidity to provide accurate pricing:

```bash
# Deposit initial liquidity (amounts depend on your target starting price)
# Example: 1000 ATOM + equivalent BLUECHIP at your target launch price
$BIN tx wasm execute $ANCHOR_POOL \
  '{"deposit_liquidity":{"amount0":"1000000000","amount1":"1000000000","min_amount0":null,"min_amount1":null,"transaction_deadline":null}}' \
  --amount "1000000000ubluechip,1000000000ibc/<ATOM_HASH>" \
  --from deployer $TX_FLAGS
```

---

## Step 9: Switch Factory from Mock Mode to Live Mode

Now that the anchor pool exists and has liquidity, update the Factory config to point to it. This disables mock mode and activates the full oracle + Expand Economy minting.

> **48-HOUR TIMELOCK WARNING:** The Factory config update uses a 48-hour timelock
> (see `execute.rs:150`). You must propose the change, wait 48 hours, then apply it.
> Plan your launch timeline accordingly — Steps 1-8 should be completed at least
> 48 hours before you want creator pool creation to go live with real oracle pricing.

### 9a. Propose the Config Update

You must submit the **full** `FactoryInstantiate` config (not just the changed field):

```bash
PROPOSED_CONFIG=$(python3 -c "
import json
print(json.dumps({
    'propose_factory_config_update': {
        'config': {
            'factory_admin_address':              '$DEPLOYER',
            'commit_amount_for_threshold_bluechip': '0',
            'commit_threshold_limit_usd':         '25000',
            'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
            'pyth_atom_usd_price_feed_id':        'ATOM_USD',
            'cw20_token_contract_id':             int('$CW20_CODE'),
            'cw721_nft_contract_id':              int('$CW721_CODE'),
            'create_pool_wasm_contract_id':       int('$POOL_CODE'),
            'bluechip_wallet_address':            '$BLUECHIP_WALLET',
            'commit_fee_bluechip':                '0.01',
            'commit_fee_creator':                 '0.05',
            'max_bluechip_lock_per_pool':         '10000000000',
            'creator_excess_liquidity_lock_days': 7,
            'atom_bluechip_anchor_pool_address':  '$ANCHOR_POOL',
            'bluechip_mint_contract_address':     '$EXP_ADDR',
        }
    }
}))
")

$BIN tx wasm execute $FACTORY_ADDR "$PROPOSED_CONFIG" \
  --from deployer $TX_FLAGS
```

The tx response will include an `effective_after` timestamp. Note it.

### 9b. Wait 48 Hours

```bash
# Check remaining time:
$BIN query wasm contract-state smart $FACTORY_ADDR '{"get_pending_config":{}}' \
  --node $NODE --output json
# Look at "effective_after" — config cannot be applied until after this time
```

### 9c. Apply the Config Update

After the 48-hour timelock has expired:

```bash
$BIN tx wasm execute $FACTORY_ADDR '{"update_config":{}}' \
  --from deployer $TX_FLAGS
```

### 9d. Verify Factory Left Mock Mode

```bash
$BIN query wasm contract-state smart $FACTORY_ADDR '{"get_config":{}}' \
  --node $NODE --output json
# atom_bluechip_anchor_pool_address should now equal $ANCHOR_POOL (not $DEPLOYER)
```

**At this point the Factory is fully live:**
- Internal oracle derives Bluechip/USD from the anchor pool + Pyth
- Expand Economy mints tokens on pool creation
- Creator pools can be created with real pricing

---

## Step 10: Verify Oracle is Live

The factory's internal oracle uses the ATOM/Bluechip anchor pool + Pyth ATOM/USD feed to derive Bluechip/USD pricing.

**Verify oracle is functional:**

```bash
# Query the factory for current bluechip price
$BIN query wasm contract-state smart $FACTORY_ADDR '{"get_bluechip_price":{}}' \
  --node $NODE --output json
```

**The oracle should return:**
- A non-zero Bluechip/USD price
- A recent `publish_time` (not stale — within 3000 seconds)

If the oracle returns stale/zero prices, check:
1. Pyth contract is deployed and has ATOM/USD feed
2. Anchor pool has sufficient liquidity
3. Factory's `atom_bluechip_anchor_pool_address` is correct (not still set to deployer)

---

## Step 11: Rewire Frontend for Mainnet

Update all frontend configuration to point to mainnet contracts and endpoints.

### 11a. Create `.env` File

Create `frontend/.env`:

```bash
VITE_FACTORY_ADDRESS=<FACTORY_ADDR>
VITE_ORACLE_ADDRESS=<ORACLE_ADDR>
VITE_POOL_ADDRESSES=<ANCHOR_POOL>
```

### 11b. Values to Verify in Frontend Code

| File | What to Check |
|------|---------------|
| `src/components/WalletConnect.tsx` | Mainnet RPC/REST URLs, chain ID `bluechip-1`, bech32 prefix `bluechip` |
| `src/components/CreatePool.tsx` | `VITE_FACTORY_ADDRESS` fallback, `VITE_ORACLE_ADDRESS` fallback, `cw20CodeId` matches `$CW20_CODE` |
| `src/pages/Portfolio.tsx` | `VITE_FACTORY_ADDRESS` fallback, `VITE_POOL_ADDRESSES` |
| `src/types/FrontendTypes.tsx` | `DEFAULT_CHAIN_CONFIG` — update `chainId`, `rpc`, `rest`, `factoryAddress`, `nativeDenom` |

### 11c. Key Config Changes

**`src/types/FrontendTypes.tsx`** — Update `DEFAULT_CHAIN_CONFIG`:

```typescript
export const DEFAULT_CHAIN_CONFIG: ChainConfig = {
    chainId: 'bluechip-1',
    chainName: 'Bluechip Mainnet',
    rpc: 'https://bluechip.rpc.bluechip.link',
    rest: 'https://bluechip.api.bluechip.link',
    factoryAddress: '<FACTORY_ADDR>',
    nativeDenom: 'ubluechip',
    coinDecimals: 6,
};
```

**`src/components/CreatePool.tsx`** — Update `DEFAULT_CONFIG.cw20CodeId`:

```typescript
const DEFAULT_CONFIG = {
    // ...
    cw20CodeId: <CW20_CODE>,  // Must match the stored code ID from Step 4a
    // ...
};
```

### 11d. Build and Deploy Frontend

```bash
cd frontend
npm install
npm run build
# Deploy dist/ to your hosting provider
```

---

## Post-Launch Verification Checklist

Run through these checks to confirm everything is working:

- [ ] **Chain**: Blocks producing, validators active
- [ ] **Keplr**: Can connect to `bluechip-1`, see balances
- [ ] **IBC**: ATOM transfers from Cosmos Hub arrive on Bluechip
- [ ] **Oracle**: `get_bluechip_price` returns non-zero, non-stale price
- [ ] **Factory**: Can query config — all code IDs and addresses correct
- [ ] **Expand Economy**: `get_config` shows correct factory address
- [ ] **Anchor Pool**: Has liquidity, swaps work
- [ ] **Pool Creation**: Create a test pool via Factory — CW20 + CW721 + Pool all instantiate
- [ ] **Commits**: Can commit to the test pool, USD value calculated correctly
- [ ] **Frontend**: Wallet connect, pool discovery, commit flow all functional

---

## Quick Reference: Contract Dependency Graph

```
Chain Live
  └─► Store Code: CW20, CW721, Pool, Expand Economy, Factory
        ├─► Instantiate Expand Economy (temp factory = deployer)
        ├─► Instantiate Factory in MOCK MODE (anchor_pool = deployer)
        │     Mock mode: oracle bypassed, Expand Economy minting skipped
        ├─► Update Expand Economy → real Factory address
        ├─► Hermes (IBC relayer for ATOM)
        └─► Create ATOM/Bluechip Anchor Pool (via Factory, while in mock mode)
              └─► Seed anchor pool with liquidity
                    └─► Propose Factory config update (anchor_pool = real pool)
                          └─► ⏳ 48-HOUR TIMELOCK ⏳
                                └─► Apply config update → Factory leaves mock mode
                                      └─► Oracle live → creators can create pools
                                            └─► Rewire frontend
```

---

## Troubleshooting

| Problem | Likely Cause | Fix |
|---------|--------------|-----|
| Pool creation fails | Wrong code IDs in Factory config | Check `cw20_token_contract_id`, `cw721_nft_contract_id`, `create_pool_wasm_contract_id` |
| Commits fail with "stale price" | Oracle not updating | Check Pyth feed, anchor pool liquidity, oracle update interval (300s) |
| Expand Economy doesn't mint | Factory address mismatch | Verify `get_config` on Expand Economy returns correct Factory |
| Threshold crossing fails | Oracle returns zero price | Ensure anchor pool has liquidity and oracle is initialized |
| Frontend can't connect | Wrong RPC/chain ID | Check `.env` and `WalletConnect.tsx` mainnet config |
| IBC transfers fail | Hermes not running or channel closed | Check `hermes health-check`, restart relayer |
