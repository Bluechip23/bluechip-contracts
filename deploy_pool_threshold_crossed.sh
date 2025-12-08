#!/bin/bash
set -e

CHAIN_ID="bluechipChain"
KEYRING="test"
FROM="alice"

echo "🚀 Deploying Full Stack with Mock Oracle & Crossing Threshold..."

# Get Alice's address
ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)
echo "Alice: $ALICE_ADDR"

# Get existing contract addresses
CW20_ADDR=$(bluechipChaind query wasm list-contract-by-code 1 --output json | jq -r '.contracts[0]')
CW721_ADDR=$(bluechipChaind query wasm list-contract-by-code 2 --output json | jq -r '.contracts[0]')

echo "CW20: $CW20_ADDR"
echo "CW721: $CW721_ADDR"

# Step 1: Upload pool contract first
echo ""
echo "📤 Uploading pool contract..."
cd /home/jeremy/snap/smartcontracts/bluechip-contracts
RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/pool.wasm artifacts/pool.wasm

POOL_UPLOAD_TX=$(bluechipChaind tx wasm store artifacts/pool.wasm \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas 5000000 \
  --keyring-backend $KEYRING \
  -y --output json | jq -r '.txhash')

sleep 6

# Query for the pool code ID
POOL_CODE_ID=$(bluechipChaind query tx $POOL_UPLOAD_TX --output json | jq -r '.events[] | select(.type == "store_code") | .attributes[] | select(.key == "code_id") | .value')
echo "✅ Pool uploaded as Code ID: $POOL_CODE_ID"

# Step 2: Upload mock oracle
echo ""
echo "📤 Uploading mock oracle..."
cp target/wasm32-unknown-unknown/release/oracle.wasm artifacts/oracle.wasm

ORACLE_UPLOAD_TX=$(bluechipChaind tx wasm store artifacts/oracle.wasm \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas 3000000 \
  --keyring-backend $KEYRING \
  -y --output json | jq -r '.txhash')

sleep 6

# Query for the oracle code ID
ORACLE_CODE_ID=$(bluechipChaind query tx $ORACLE_UPLOAD_TX --output json | jq -r '.events[] | select(.type == "store_code") | .attributes[] | select(.key == "code_id") | .value')
echo "✅ Oracle uploaded as Code ID: $ORACLE_CODE_ID"

# Step 3: Instantiate mock oracle
echo "🔮 Instantiating mock oracle..."
bluechipChaind tx wasm instantiate $ORACLE_CODE_ID '{}' \
  --from $FROM \
  --label "mock_oracle" \
  --chain-id $CHAIN_ID \
  --gas 200000 \
  --keyring-backend $KEYRING \
  --no-admin \
  -y

sleep 6

ORACLE_ADDR=$(bluechipChaind query wasm list-contract-by-code $ORACLE_CODE_ID --output json | jq -r '.contracts[0]')
echo "✅ Mock Oracle: $ORACLE_ADDR"

# Step 4: Set ATOM/USD price (1 ATOM = $10, with expo -8 = 1000000000)
echo "💰 Setting ATOM/USD price to \$10..."
bluechipChaind tx wasm execute $ORACLE_ADDR \
  '{"set_price":{"price_id":"ATOM_USD","price":"250000000000000"}}' \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --keyring-backend $KEYRING \
  -y

sleep 3

# Step 5: Upload factory
echo ""
echo "📤 Uploading factory..."
cp target/wasm32-unknown-unknown/release/factory.wasm artifacts/factory.wasm

FACTORY_UPLOAD_TX=$(bluechipChaind tx wasm store artifacts/factory.wasm \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas 5000000 \
  --keyring-backend $KEYRING \
  -y --output json | jq -r '.txhash')

sleep 6

# Query for the factory code ID
FACTORY_CODE_ID=$(bluechipChaind query tx $FACTORY_UPLOAD_TX --output json | jq -r '.events[] | select(.type == "store_code") | .attributes[] | select(.key == "code_id") | .value')
echo "✅ Factory uploaded as Code ID: $FACTORY_CODE_ID"

# Step 6: Instantiate factory with mock oracle
echo "🏭 Instantiating factory..."
FACTORY_INIT=$(cat <<EOF
{
  "factory_admin_address": "$ALICE_ADDR",
  "commit_amount_for_threshold_bluechip": "0",
  "commit_threshold_limit_usd": "25000",
  "pyth_contract_addr_for_conversions": "$ORACLE_ADDR",
  "pyth_atom_usd_price_feed_id": "ATOM_USD",
  "cw721_nft_contract_id": 2,
  "cw20_token_contract_id": 1,
  "create_pool_wasm_contract_id": $POOL_CODE_ID,
  "bluechip_wallet_address": "$ALICE_ADDR",
  "commit_fee_bluechip": "0.01",
  "commit_fee_creator": "0.05",
  "max_bluechip_lock_per_pool": "25000000000",
  "creator_excess_liquidity_lock_days": 604800,
  "atom_bluechip_anchor_pool_address": "$ALICE_ADDR"
}
EOF
)

bluechipChaind tx wasm instantiate $FACTORY_CODE_ID "$FACTORY_INIT" \
  --from $FROM \
  --label "factory" \
  --admin $ALICE_ADDR \
  --chain-id $CHAIN_ID \
  --gas auto --gas-adjustment 1.3 \
  --keyring-backend $KEYRING \
  -y

sleep 6

FACTORY_ADDR=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE_ID --output json | jq -r '.contracts[0]')
echo "✅ Factory: $FACTORY_ADDR"

# Step 7: Instantiate pool with factory address
echo ""
echo "🏊 Instantiating pool with factory..."
THRESHOLD_PAYOUT='{"creator_reward_amount":"325000000000","bluechip_reward_amount":"25000000000","pool_seed_amount":"350000000000","commit_return_amount":"500000000000"}'
THRESHOLD_B64=$(echo $THRESHOLD_PAYOUT | base64 -w 0)

CREATE_POOL_MSG=$(cat <<EOF
{
  "create": {
    "pool_msg": {
      "pool_token_info": [
        { "bluechip": { "denom": "stake" } },
        { "creator_token": { "contract_addr": "WILL_BE_CREATED_BY_FACTORY" } }
      ],
      "cw20_token_contract_id": 1,
      "factory_to_create_pool_addr": "$FACTORY_ADDR",
      "threshold_payout": "$THRESHOLD_B64",
      "commit_fee_info": {
        "bluechip_wallet_address": "$ALICE_ADDR",
        "creator_wallet_address": "$ALICE_ADDR",
        "commit_fee_bluechip": "0.01",
        "commit_fee_creator": "0.05"
      },
      "creator_token_address": "$ALICE_ADDR",
      "commit_amount_for_threshold": "25000000000",
      "commit_limit_usd": "25000000000",
      "pyth_contract_addr_for_conversions": "$ORACLE_ADDR",
      "pyth_atom_usd_price_feed_id": "ATOM_USD",
      "max_bluechip_lock_per_pool": "10000000000",
      "creator_excess_liquidity_lock_days": 7
    },
    "token_info": {
      "name": "Creator Token",
      "symbol": "CREATOR",
      "decimal": 6
    }
  }
}
EOF
)

CREATE_POOL_TX=$(bluechipChaind tx wasm execute $FACTORY_ADDR "$CREATE_POOL_MSG" \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas auto --gas-adjustment 1.3 \
  --keyring-backend $KEYRING \
  -y --output json | jq -r '.txhash')

sleep 6

# Query for the pool address from the transaction events
POOL_ADDR=$(bluechipChaind query tx $CREATE_POOL_TX --output json | jq -r '.events[] | select(.type == "wasm") | .attributes[] | select(.key == "pool_address") | .value')

echo "✅ Pool Deployed at: $POOL_ADDR"

# Step 8: Commit funds to cross threshold
# Threshold is $25,000. Price is $10. Need 2,500 tokens.
# Committing 30,000 tokens (30,000,000,000 micro-units) to be safe.
echo ""
echo "🚀 Committing funds to cross threshold..."

COMMIT_AMOUNT="10000"
COMMIT_MSG=$(cat <<EOF
{
  "commit": {
    "asset": {
      "info": {
        "bluechip": {
          "denom": "stake"
        }
      },
      "amount": "$COMMIT_AMOUNT"
    },
    "amount": "$COMMIT_AMOUNT"
  }
}
EOF
)

COMMIT_TX=$(bluechipChaind tx wasm execute $POOL_ADDR "$COMMIT_MSG" \
  --amount "${COMMIT_AMOUNT}stake" \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas auto --gas-adjustment 1.3 \
  --keyring-backend $KEYRING \
  -y --output json | jq -r '.txhash')

echo "Commit Transaction Hash: $COMMIT_TX"


sleep 6

echo ""
echo "🎉 Deployment & Threshold Crossing Complete!"
sleep 15

# Step 9: Provide Initial Liquidity
# Alice needs to provide liquidity to enable swaps
# 1. Get the Creator Token address from the pool
echo ""
echo "� Querying pool for Creator Token address..."
TOKEN_ADDR=$(bluechipChaind query wasm contract-state smart $POOL_ADDR '{"pair":{}}' --output json | jq -r '.data.asset_infos[] | select(.creator_token != null) | .creator_token.contract_addr')
echo "Creator Token: $TOKEN_ADDR"

# 2. Approve pool to spend Creator Token
echo ""
echo "🔓 Approving pool to spend Creator Token..."
APPROVE_MSG='{"increase_allowance":{"spender":"'$POOL_ADDR'","amount":"1000000000"}}'
bluechipChaind tx wasm execute $TOKEN_ADDR "$APPROVE_MSG" \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas auto --gas-adjustment 1.3 \
  -y

sleep 6

# 2. Deposit Liquidity
# Pool has ~10,000 stake and ~350,000,000,000 Creator Tokens (Ratio 1:35,000,000)
# We need to match this ratio.
# Let's deposit 10 stake and 350,000,000 Creator Tokens
echo ""
echo "💧 Depositing Liquidity..."
DEPOSIT_MSG='{"deposit_liquidity":{"amount0":"10","amount1":"350000000"}}'
bluechipChaind tx wasm execute $POOL_ADDR "$DEPOSIT_MSG" \
  --amount 10stake \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --gas auto --gas-adjustment 1.3 \
  -y

sleep 6

echo "✅ Liquidity Provided!"
echo "================================"
echo "Mock Oracle: $ORACLE_ADDR"
echo "Factory: $FACTORY_ADDR"
echo "Pool: $POOL_ADDR"
echo "CW20: $CW20_ADDR"
echo "CW721: $CW721_ADDR"
echo "================================"
echo ""
echo "💡 Add to frontend:"
echo "const POOL_CONTRACT = \"$POOL_ADDR\";"
echo ""
echo "✅ The pool should now be in 'Threshold Hit' state."
echo "You can now test Liquidity Provision and Swaps."
