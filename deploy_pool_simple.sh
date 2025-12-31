#!/bin/bash

# Deploy Pool Contract to Local Chain (Simplified for Testing)
# Uses Alice's address as mock factory/oracle

set -e

CHAIN_ID="bluechipChain"
NODE="http://localhost:26657"
KEYRING="test"
FROM="alice"

echo "🚀 Starting Pool Contract Deployment..."
echo "⚠️  Note: This uses mock addresses for testing. Oracle queries will fail."
echo ""

# Get Alice's address first
ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)
echo "Deployer (Alice): $ALICE_ADDR"
echo ""

# Step 1: Build the contract with optimization
echo "📦 Building and optimizing contract..."
cd /home/jeremy/snap/smartcontracts/bluechip-contracts
RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/pool.wasm artifacts/pool.wasm

# Use the optimized wasm from artifacts
WASM_FILE="artifacts/pool.wasm"

if [ ! -f "$WASM_FILE" ]; then
  echo "❌ WASM file not found at $WASM_FILE"
  exit 1
fi

# Check file size
FILE_SIZE=$(stat -f%z "$WASM_FILE" 2>/dev/null || stat -c%s "$WASM_FILE" 2>/dev/null)
FILE_SIZE_MB=$(echo "scale=2; $FILE_SIZE / 1048576" | bc)
echo "✅ Contract built successfully (${FILE_SIZE_MB}MB)"

if [ $FILE_SIZE -gt 1048576 ]; then
  echo "⚠️  Warning: File is larger than 1MB, upload might fail"
fi

# Step 2: Store the contract
echo "📤 Uploading contract to chain..."
STORE_TX=$(bluechipChaind tx wasm store $WASM_FILE \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas 5000000 \
  --keyring-backend $KEYRING \
  --output json \
  -y)

TX_HASH=$(echo $STORE_TX | jq -r '.txhash')
echo "Transaction hash: $TX_HASH"
echo "Waiting for transaction to be included in a block..."

# Wait and retry tx query
for i in {1..10}; do
  sleep 3
  TX_RESULT=$(bluechipChaind query tx $TX_HASH --node $NODE --output json 2>/dev/null || echo '{}')
  CODE_ID=$(echo $TX_RESULT | jq -r '.logs[0].events[]? | select(.type=="store_code") | .attributes[]? | select(.key=="code_id") | .value' 2>/dev/null)
  
  if [ ! -z "$CODE_ID" ] && [ "$CODE_ID" != "null" ]; then
    break
  fi
  echo "Retry $i/10..."
done

if [ -z "$CODE_ID" ] || [ "$CODE_ID" = "null" ]; then
  echo "❌ Failed to get code ID after 10 retries."
  echo "Try querying manually: bluechipChaind query tx $TX_HASH --node $NODE"
  exit 1
fi

echo "✅ Contract stored with Code ID: $CODE_ID"

# Step 3: Instantiate CW20 token (for creator token)
echo "🪙 Creating CW20 token..."

CW20_INIT=$(cat <<EOF
{
  "name": "Test Creator Token",
  "symbol": "TCT",
  "decimals": 6,
  "initial_balances": [
    {
      "address": "$ALICE_ADDR",
      "amount": "1000000000000"
    }
  ],
  "mint": {
    "minter": "$ALICE_ADDR"
  }
}
EOF
)

CW20_TX=$(bluechipChaind tx wasm instantiate 1 "$CW20_INIT" \
  --from $FROM \
  --label "test_creator_token_$(date +%s)" \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas 500000 \
  --keyring-backend $KEYRING \
  --no-admin \
  --output json \
  -y)

sleep 6

CW20_ADDR=$(bluechipChaind query wasm list-contract-by-code 1 --node $NODE --output json | jq -r '.contracts[-1]')
echo "✅ CW20 Token deployed at: $CW20_ADDR"

# Step 4: Instantiate CW721 NFT (for position NFTs)
echo "🎨 Creating CW721 NFT..."
CW721_INIT=$(cat <<EOF
{
  "name": "Pool Position NFT",
  "symbol": "PPNFT",
  "minter": "$ALICE_ADDR"
}
EOF
)

CW721_TX=$(bluechipChaind tx wasm instantiate 2 "$CW721_INIT" \
  --from $FROM \
  --label "position_nft_$(date +%s)" \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas 500000 \
  --keyring-backend $KEYRING \
  --no-admin \
  --output json \
  -y)

sleep 6

CW721_ADDR=$(bluechipChaind query wasm list-contract-by-code 2 --node $NODE --output json | jq -r '.contracts[-1]')
echo "✅ CW721 NFT deployed at: $CW721_ADDR"

# Step 5: Create mock threshold payout (required for pool init)
# Total should equal commit_amount_for_threshold (25000 stake = 25000000000 ustake)
THRESHOLD_PAYOUT=$(cat <<EOF
{
  "creator_reward_amount": "5000000000",
  "bluechip_reward_amount": "2500000000",
  "pool_seed_amount": "12500000000",
  "commit_return_amount": "5000000000"
}
EOF
)

# Convert to base64 for Binary type
THRESHOLD_PAYOUT_B64=$(echo $THRESHOLD_PAYOUT | base64 -w 0)

# Step 6: Create pool initialization message
POOL_INIT=$(cat <<EOF
{
  "pool_id": 1,
  "pool_token_info": [
    {
      "bluechip": {
        "denom": "stake"
      }
    },
    {
      "creator_token": {
        "contract_addr": "$CW20_ADDR"
      }
    }
  ],
  "cw20_token_contract_id": 1,
  "used_factory_addr": "$ALICE_ADDR",
  "threshold_payout": "$THRESHOLD_PAYOUT_B64",
  "commit_fee_info": {
    "bluechip_wallet_address": "$ALICE_ADDR",
    "creator_wallet_address": "$ALICE_ADDR",
    "commit_fee_bluechip": "0.01",
    "commit_fee_creator": "0.05"
  },
  "commit_threshold_limit_usd": "25000000000",
  "commit_amount_for_threshold": "25000000000",
  "position_nft_address": "$CW721_ADDR",
  "token_address": "$CW20_ADDR",
  "max_bluechip_lock_per_pool": "10000000000",
  "creator_excess_liquidity_lock_days": 7
}
EOF
)

echo "🏊 Instantiating pool contract..."
POOL_TX=$(bluechipChaind tx wasm instantiate $CODE_ID "$POOL_INIT" \
  --from $FROM \
  --label "test_pool_$(date +%s)" \
  --admin $ALICE_ADDR \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas 1000000 \
  --keyring-backend $KEYRING \
  --output json \
  -y)

sleep 6

POOL_ADDR=$(bluechipChaind query wasm list-contract-by-code $CODE_ID --node $NODE --output json | jq -r '.contracts[-1]')

echo ""
echo "🎉 Deployment Complete!"
echo "================================"
echo "Pool Contract Address: $POOL_ADDR"
echo "CW20 Token Address: $CW20_ADDR"
echo "CW721 NFT Address: $CW721_ADDR"
echo "Code ID: $CODE_ID"
echo "================================"
echo ""
echo "⚠️  IMPORTANT NOTES:"
echo "1. Oracle queries will fail (using Alice as mock factory)"
echo "2. You can test: Liquidity deposits, swaps (without oracle price checks)"
echo "3. Commit transactions will fail (require oracle for USD conversion)"
echo ""
echo "💡 Add this to your frontend components:"
echo "const POOL_CONTRACT = \"$POOL_ADDR\";"
echo "const CW20_TOKEN = \"$CW20_ADDR\";"
echo ""

# Save addresses to a file
cat > deployment_addresses.json <<EOF
{
  "pool_contract": "$POOL_ADDR",
  "cw20_token": "$CW20_ADDR",
  "cw721_nft": "$CW721_ADDR",
  "code_id": "$CODE_ID",
  "chain_id": "$CHAIN_ID",
  "deployer": "$ALICE_ADDR",
  "note": "Oracle is mocked - commit transactions will fail"
}
EOF

echo "📝 Addresses saved to deployment_addresses.json"
echo ""
echo "🧪 Test the deployment:"
echo "bluechipChaind query wasm contract $POOL_ADDR --node $NODE"
