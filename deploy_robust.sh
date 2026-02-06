#!/bin/bash
set -e

CHAIN_ID="bluechipChain"
KEYRING="test"
FROM="alice"

# Function to run command and filter warning
run_cmd() {
    "$@" 2>&1 | grep -v "WARNING"
}

# Function to run tx and capture hash
run_tx() {
    CMD="bluechipChaind tx wasm $@"
    # echo "Executing: $CMD" >&2
    bluechipChaind tx wasm "$@" --from $FROM --chain-id $CHAIN_ID --gas 5000000 --gas-adjustment 1.3 --keyring-backend $KEYRING -y --output json | grep -v "WARNING" | jq -r '.txhash'
}

# Function to query tx and get attribute (with retries for indexing)
get_tx_attr() {
    TX_HASH=$1
    TYPE=$2
    KEY=$3
    # sleep 2
    for i in {1..5}; do
        RES=$(bluechipChaind query tx $TX_HASH --output json 2>/dev/null | grep -v "WARNING")
        if [ ! -z "$RES" ]; then
             VAL=$(echo $RES | jq -r ".events[] | select(.type == \"$TYPE\") | .attributes[] | select(.key == \"$KEY\") | .value" 2>/dev/null)
             if [ ! -z "$VAL" ]; then
                 echo $VAL
                 return
             fi
        fi
        sleep 2
    done
    echo "ERROR: Could not find attribute $KEY in tx $TX_HASH" >&2
    return 1
}

# Function to store contract and get code id
store_contract() {
    FILE=$1
    echo "📤 Uploading $FILE..." >&2
    TX=$(run_tx store $FILE)
    # echo "Tx Hash: $TX" >&2
    sleep 6
    CODE_ID=$(get_tx_attr $TX "store_code" "code_id")
    echo "✅ Uploaded $FILE as Code ID: $CODE_ID" >&2
    echo $CODE_ID
}

echo "🚀 Deploying Full Stack Robustly..."

ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING | grep -v "WARNING")
echo "Alice: $ALICE_ADDR"

# Download correct versions
echo "⬇️ Downloading base contracts..."
wget -q -O artifacts/cw20_base.wasm https://github.com/CosmWasm/cw-plus/releases/download/v1.0.1/cw20_base.wasm
wget -q -O artifacts/cw721_base.wasm https://github.com/CosmWasm/cw-nfts/releases/download/v0.18.0/cw721_base.wasm

# Step 0: Upload Base Contracts
CW20_CODE_ID=$(store_contract "artifacts/cw20_base.wasm")
CW721_CODE_ID=$(store_contract "artifacts/cw721_base.wasm")

# Step 1: Upload Pool
POOL_CODE_ID=$(store_contract "artifacts/pool.wasm")

# Step 2: Upload Oracle & Economy
ORACLE_CODE_ID=$(store_contract "artifacts/mock_oracle.wasm")
ECON_CODE_ID=$(store_contract "artifacts/expand-economy.wasm")

# Step 3: Instantiate Oracle
echo "🔮 Instantiating mock oracle..."
ORACLE_TX=$(run_tx instantiate $ORACLE_CODE_ID '{}' --label "mock_oracle" --no-admin)
sleep 6
ORACLE_ADDR=$(get_tx_attr $ORACLE_TX "instantiate" "_contract_address")
echo "✅ Mock Oracle: $ORACLE_ADDR"

# Step 3b: Instantiate Economy
echo "📈 Instantiating expand economy..."
# Use Alice as temp factory
ECON_TX=$(run_tx instantiate $ECON_CODE_ID "{\"factory_address\":\"$ALICE_ADDR\"}" --label "expand_economy" --no-admin)
sleep 6
ECON_ADDR=$(get_tx_attr $ECON_TX "instantiate" "_contract_address")
echo "✅ Expand Economy: $ECON_ADDR"

# Fund Economy
echo "💰 Funding expand economy..."
bluechipChaind tx bank send $ALICE_ADDR $ECON_ADDR 1000000000stake --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 6

# Set Price
echo "💰 Setting Price..."
bluechipChaind tx wasm execute $ORACLE_ADDR '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 3

# Step 5: Upload Factory
FACTORY_CODE_ID=$(store_contract "artifacts/factory.wasm")

# Step 6: Instantiate Factory
echo "🏭 Instantiating factory..."
FACTORY_INIT=$(cat <<EOF
{
  "factory_admin_address": "$ALICE_ADDR",
  "commit_amount_for_threshold_bluechip": "0",
  "commit_threshold_limit_usd": "25000000000",
  "pyth_contract_addr_for_conversions": "$ORACLE_ADDR",
  "pyth_atom_usd_price_feed_id": "ATOM_USD",
  "cw721_nft_contract_id": $CW721_CODE_ID,
  "cw20_token_contract_id": $CW20_CODE_ID,
  "create_pool_wasm_contract_id": $POOL_CODE_ID,
  "bluechip_wallet_address": "$ALICE_ADDR",
  "commit_fee_bluechip": "0.01",
  "commit_fee_creator": "0.05",
  "max_bluechip_lock_per_pool": "25000000000",
  "creator_excess_liquidity_lock_days": 604800,
  "atom_bluechip_anchor_pool_address": "$ALICE_ADDR",
  "bluechip_mint_contract_address": "$ECON_ADDR"
}
EOF
)

FACTORY_TX=$(run_tx instantiate $FACTORY_CODE_ID "$FACTORY_INIT" --label "factory" --admin $ALICE_ADDR)
sleep 6
FACTORY_ADDR=$(get_tx_attr $FACTORY_TX "instantiate" "_contract_address")
echo "✅ Factory: $FACTORY_ADDR"

# Link Economy
bluechipChaind tx wasm execute $ECON_ADDR "{\"update_config\":{\"factory_address\":\"$FACTORY_ADDR\"}}" --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 6

# ... (Create Pool 1)
# Note: Use updated constants (500B Creator/2B Pool)
THRESHOLD_PAYOUT='{"creator_reward_amount":"500000000000","bluechip_reward_amount":"500000000","pool_seed_amount":"2000000000","commit_return_amount":"500000000000"}'
THRESHOLD_B64=$(echo -n $THRESHOLD_PAYOUT | base64 -w 0)

CREATE_POOL_MSG=$(cat <<EOF
{
  "create": {
    "pool_msg": {
      "pool_token_info": [
        { "bluechip": { "denom": "stake" } },
        { "creator_token": { "contract_addr": "WILL_BE_CREATED_BY_FACTORY" } }
      ],
      "cw20_token_contract_id": $CW20_CODE_ID,
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

echo "🏊 Creating Pool 1..."
POOL_TX=$(run_tx execute $FACTORY_ADDR "$CREATE_POOL_MSG")
sleep 6
POOL_ADDR=$(get_tx_attr $POOL_TX "wasm" "pool_address")

echo "✅ Pool 1: $POOL_ADDR"

echo "🎉 Done."
