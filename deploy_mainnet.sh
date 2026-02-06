#!/bin/bash
set -e

# Bluechip Mainnet Configuration
CHAIN_ID="bluechip-1"
RPC="https://rpc.bluechip.zone"
# Start with a public node, or use your own
NODE="--node $RPC"
DENOM="stake"

# Contracts
# TODO: Update with real Mainnet Pyth address
PYTH_ADDR="bluechip1..." 
# BLUECHIP/USD Price Feed (using ATOM until BLUECHIP feed is live)
PRICE_FEED_ID="61c77209618c575dc61e605d2146522c15927c3c544ac472652A5A29D6"

# Wallet
KEY_NAME="mainnet-admin" 
# Ensure this key exists in your keyring-backend or ledger
KEYRING="--keyring-backend file"

echo "🚀 Deploying to Bluechip Mainnet ($CHAIN_ID)..."
echo "Ensure you have 'bluechipd' installed and configured."

# Helper function to get address
get_addr() {
  bluechipd keys show $KEY_NAME -a $KEYRING
}

ADMIN_ADDR=$(get_addr)
echo "Admin Address: $ADMIN_ADDR"

# 1. Upload CW20 Base
echo "📤 Uploading CW20 Base..."
RES=$(bluechipd tx wasm store artifacts/cw20_base.wasm --from $KEY_NAME --chain-id $CHAIN_ID $NODE --gas auto --gas-adjustment 1.3 -y --output json)
CW20_CODE_ID=$(echo $RES | jq -r '.logs[0].events[] | select(.type=="store_code") | .attributes[] | select(.key=="code_id") | .value')
echo "✅ CW20 Code ID: $CW20_CODE_ID"

# 2. Upload CW721 Base
echo "📤 Uploading CW721 Base..."
RES=$(bluechipd tx wasm store artifacts/cw721_base.wasm --from $KEY_NAME --chain-id $CHAIN_ID $NODE --gas auto --gas-adjustment 1.3 -y --output json)
CW721_CODE_ID=$(echo $RES | jq -r '.logs[0].events[] | select(.type=="store_code") | .attributes[] | select(.key=="code_id") | .value')
echo "✅ CW721 Code ID: $CW721_CODE_ID"

# 3. Build & Upload Pool
echo "🔨 Building Pool..."
RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown --bin pool
cp target/wasm32-unknown-unknown/release/pool.wasm artifacts/pool.wasm

echo "📤 Uploading Pool..."
RES=$(bluechipd tx wasm store artifacts/pool.wasm --from $KEY_NAME --chain-id $CHAIN_ID $NODE --gas auto --gas-adjustment 1.3 -y --output json)
POOL_CODE_ID=$(echo $RES | jq -r '.logs[0].events[] | select(.type=="store_code") | .attributes[] | select(.key=="code_id") | .value')
echo "✅ Pool Code ID: $POOL_CODE_ID"

# 4. Build & Upload Factory
echo "🔨 Building Factory..."
RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown --bin factory
cp target/wasm32-unknown-unknown/release/factory.wasm artifacts/factory.wasm

echo "📤 Uploading Factory..."
RES=$(bluechipd tx wasm store artifacts/factory.wasm --from $KEY_NAME --chain-id $CHAIN_ID $NODE --gas auto --gas-adjustment 1.3 -y --output json)
FACTORY_CODE_ID=$(echo $RES | jq -r '.logs[0].events[] | select(.type=="store_code") | .attributes[] | select(.key=="code_id") | .value')
echo "✅ Factory Code ID: $FACTORY_CODE_ID"

# 5. Instantiate Factory
echo "🏭 Instantiating Factory..."
FACTORY_INIT=$(jq -n \
  --arg admin "$ADMIN_ADDR" \
  --arg pyth "$PYTH_ADDR" \
  --arg feed "$PRICE_FEED_ID" \
  --arg cw20 "$CW20_CODE_ID" \
  --arg cw721 "$CW721_CODE_ID" \
  --arg pool "$POOL_CODE_ID" \
  '{
    factory_admin_address: $admin,
    commit_amount_for_threshold_bluechip: "0",
    commit_threshold_limit_usd: "25000",
    pyth_contract_addr_for_conversions: $pyth,
    pyth_atom_usd_price_feed_id: $feed,
    cw20_token_contract_id: ($cw20|tonumber),
    cw721_nft_contract_id: ($cw721|tonumber),
    create_pool_wasm_contract_id: ($pool|tonumber),
    bluechip_wallet_address: $admin,
    commit_fee_bluechip: "0.01",
    commit_fee_creator: "0.05",
    max_bluechip_lock_per_pool: "25000000000",
    creator_excess_liquidity_lock_days: 604800,
    atom_bluechip_anchor_pool_address: $admin
  }')

RES=$(bluechipd tx wasm instantiate $FACTORY_CODE_ID "$FACTORY_INIT" \
  --from $KEY_NAME --label "bluechip_factory_mainnet" --admin $ADMIN_ADDR \
  --chain-id $CHAIN_ID $NODE --gas auto --gas-adjustment 1.3 -y --output json)
FACTORY_ADDR=$(echo $RES | jq -r '.logs[0].events[] | select(.type=="instantiate") | .attributes[] | select(.key=="_contract_address") | .value')
echo "✅ Factory Address: $FACTORY_ADDR"

echo "deployment_mainnet.json created."
cat > deployment_mainnet.json <<EOF
{
  "factory": "$FACTORY_ADDR",
  "cw20_code": $CW20_CODE_ID,
  "cw721_code": $CW721_CODE_ID,
  "pool_code": $POOL_CODE_ID,
  "admin": "$ADMIN_ADDR"
}
EOF
