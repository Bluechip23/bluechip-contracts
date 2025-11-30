#!/bin/bash

# Deploy Pool Contract to Local Chain
# This script deploys the pool contract with mock dependencies for frontend testing

set -e

CHAIN_ID="bluechipChain"
NODE="http://localhost:26657"
KEYRING="test"
FROM="alice"

echo "🚀 Starting Pool Contract Deployment..."

# Step 1: Build the contract
echo "📦 Building contract..."
cd /home/jeremy/snap/smartcontracts/bluechip-contracts
cargo wasm

# Step 2: Optimize (optional, but recommended)
echo "🔧 Optimizing contract..."
docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source="$(basename "$(pwd)")_cache",target=/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0 ./pool

# Step 3: Store the contract
echo "📤 Uploading contract to chain..."
STORE_RESULT=$(bluechipChaind tx wasm store artifacts/pool.wasm \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas auto \
  --gas-adjustment 1.5 \
  --keyring-backend $KEYRING \
  --output json \
  -y)

echo "Waiting for transaction to be included in a block..."
sleep 6

# Extract code ID
CODE_ID=$(echo $STORE_RESULT | jq -r '.logs[0].events[] | select(.type=="store_code") | .attributes[] | select(.key=="code_id") | .value')

if [ -z "$CODE_ID" ]; then
  echo "❌ Failed to get code ID. Checking transaction..."
  TX_HASH=$(echo $STORE_RESULT | jq -r '.txhash')
  bluechipChaind query tx $TX_HASH --node $NODE
  exit 1
fi

echo "✅ Contract stored with Code ID: $CODE_ID"

# Step 4: Instantiate CW20 token (for creator token)
echo "🪙 Creating CW20 token..."
CW20_INIT='{
  "name": "Test Creator Token",
  "symbol": "TCT",
  "decimals": 6,
  "initial_balances": [
    {
      "address": "'$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)'",
      "amount": "1000000000000"
    }
  ],
  "mint": {
    "minter": "'$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)'"
  }
}'

CW20_RESULT=$(bluechipChaind tx wasm instantiate 1 "$CW20_INIT" \
  --from $FROM \
  --label "test_creator_token" \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas auto \
  --gas-adjustment 1.5 \
  --keyring-backend $KEYRING \
  --no-admin \
  --output json \
  -y)

sleep 6

CW20_ADDR=$(bluechipChaind query wasm list-contract-by-code 1 --node $NODE --output json | jq -r '.contracts[-1]')
echo "✅ CW20 Token deployed at: $CW20_ADDR"

# Step 5: Instantiate CW721 NFT (for position NFTs)
echo "🎨 Creating CW721 NFT..."
CW721_INIT='{
  "name": "Pool Position NFT",
  "symbol": "PPNFT",
  "minter": "'$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)'"
}'

CW721_RESULT=$(bluechipChaind tx wasm instantiate 2 "$CW721_INIT" \
  --from $FROM \
  --label "position_nft" \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas auto \
  --gas-adjustment 1.5 \
  --keyring-backend $KEYRING \
  --no-admin \
  --output json \
  -y)

sleep 6

CW721_ADDR=$(bluechipChaind query wasm list-contract-by-code 2 --node $NODE --output json | jq -r '.contracts[-1]')
echo "✅ CW721 NFT deployed at: $CW721_ADDR"

# Step 6: Create pool initialization message
ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)

POOL_INIT='{
  "pool_id": 1,
  "pool_token_info": [
    {
      "bluechip": {
        "denom": "stake"
      }
    },
    {
      "creator_token": {
        "contract_addr": "'$CW20_ADDR'"
      }
    }
  ],
  "cw20_token_contract_id": 1,
  "used_factory_addr": "'$ALICE_ADDR'",
  "threshold_payout": null,
  "commit_fee_info": {
    "bluechip_wallet_address": "'$ALICE_ADDR'",
    "creator_wallet_address": "'$ALICE_ADDR'",
    "commit_fee_bluechip": "0.01",
    "commit_fee_creator": "0.05"
  },
  "commit_threshold_limit_usd": "25000000000",
  "commit_amount_for_threshold": "25000000000",
  "position_nft_address": "'$CW721_ADDR'",
  "token_address": "'$CW20_ADDR'",
  "max_bluechip_lock_per_pool": "10000000000",
  "creator_excess_liquidity_lock_days": 7
}'

echo "🏊 Instantiating pool contract..."
POOL_RESULT=$(bluechipChaind tx wasm instantiate $CODE_ID "$POOL_INIT" \
  --from $FROM \
  --label "test_pool" \
  --admin $ALICE_ADDR \
  --chain-id $CHAIN_ID \
  --node $NODE \
  --gas auto \
  --gas-adjustment 1.5 \
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
echo "💡 Add this to your frontend:"
echo "const POOL_CONTRACT = \"$POOL_ADDR\";"
echo ""

# Save addresses to a file
cat > deployment_addresses.json <<EOF
{
  "pool_contract": "$POOL_ADDR",
  "cw20_token": "$CW20_ADDR",
  "cw721_nft": "$CW721_ADDR",
  "code_id": "$CODE_ID",
  "chain_id": "$CHAIN_ID",
  "deployer": "$ALICE_ADDR"
}
EOF

echo "📝 Addresses saved to deployment_addresses.json"
