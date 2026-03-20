#!/bin/bash
# =====================================================================
# Deploy Full Stack with Mock Oracle — Local Testing
# =====================================================================
# Deploys all contracts (pool, factory, expand-economy, mock oracle)
# to a local bluechipChain and creates a test pool.
#
# Prerequisites:
#   - Local chain running (bluechipChaind)
#   - CW20 base and CW721 base already stored (code IDs 1 and 2)
#     OR use deploy_robust.sh which downloads and stores them
#   - Contracts built with: make build
#     (this builds mock oracle with --features testing)
#
# Usage: bash deploy_full_stack_mock_oracle.sh
# =====================================================================
set -e

CHAIN_ID="bluechipChain"
KEYRING="test"
FROM="alice"
ARTIFACTS="artifacts"
# Native denom — must match what your local chain genesis provides.
# The frontend expects "ubluechip". If your chain uses "stake", change this.
DENOM="${BLUECHIP_DENOM:-ubluechip}"

echo "Deploying Full Stack with Mock Oracle..."
echo "Native denom: $DENOM"

# Get Alice's address
ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING)
echo "Alice: $ALICE_ADDR"

# Get existing base contract addresses (CW20 = code 1, CW721 = code 2)
CW20_CODE_ID=1
CW721_CODE_ID=2

CW20_ADDR=$(bluechipChaind query wasm list-contract-by-code $CW20_CODE_ID --output json | jq -r '.contracts[0]')
CW721_ADDR=$(bluechipChaind query wasm list-contract-by-code $CW721_CODE_ID --output json | jq -r '.contracts[0]')

echo "CW20 (code $CW20_CODE_ID): $CW20_ADDR"
echo "CW721 (code $CW721_CODE_ID): $CW721_ADDR"

# ─── Helper: store wasm and get code ID ──────────────────────────────────────
store_and_get_code_id() {
  local FILE=$1
  local LABEL=$2
  echo ""
  echo "Uploading $LABEL ($FILE)..."
  local TX_HASH
  TX_HASH=$(bluechipChaind tx wasm store "$ARTIFACTS/$FILE" \
    --from $FROM \
    --chain-id $CHAIN_ID \
    --gas 5000000 \
    --keyring-backend $KEYRING \
    -y --output json | jq -r '.txhash')

  sleep 6

  local CODE_ID
  CODE_ID=$(bluechipChaind query tx "$TX_HASH" --output json | jq -r '.events[] | select(.type == "store_code") | .attributes[] | select(.key == "code_id") | .value')
  echo "  $LABEL uploaded as Code ID: $CODE_ID"
  echo "$CODE_ID"
}

# ─── Step 1: Upload contracts ────────────────────────────────────────────────
POOL_CODE_ID=$(store_and_get_code_id "pool.wasm" "Pool")
ORACLE_CODE_ID=$(store_and_get_code_id "oracle.wasm" "Mock Oracle")
ECON_CODE_ID=$(store_and_get_code_id "expand_economy.wasm" "Expand Economy")

# ─── Step 2: Instantiate Mock Oracle ────────────────────────────────────────
echo ""
echo "Instantiating mock oracle..."
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
echo "  Mock Oracle: $ORACLE_ADDR"

# ─── Step 3: Instantiate Expand Economy (Alice as temporary factory) ─────────
echo ""
echo "Instantiating expand economy..."
bluechipChaind tx wasm instantiate $ECON_CODE_ID \
  "{\"factory_address\":\"$ALICE_ADDR\",\"owner\":\"$ALICE_ADDR\"}" \
  --from $FROM \
  --label "expand_economy" \
  --chain-id $CHAIN_ID \
  --gas 200000 \
  --keyring-backend $KEYRING \
  --no-admin \
  -y

sleep 6

ECON_ADDR=$(bluechipChaind query wasm list-contract-by-code $ECON_CODE_ID --output json | jq -r '.contracts[0]')
echo "  Expand Economy: $ECON_ADDR"

# ─── Step 4: Fund expand economy ────────────────────────────────────────────
echo ""
echo "Funding expand economy with 1000000000${DENOM}..."
bluechipChaind tx bank send $ALICE_ADDR $ECON_ADDR "1000000000${DENOM}" \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --keyring-backend $KEYRING \
  -y
sleep 6

# ─── Step 5: Set ATOM/USD price ($10 = 1000000000 at expo -8) ───────────────
echo ""
echo "Setting ATOM/USD price to \$10..."
bluechipChaind tx wasm execute $ORACLE_ADDR \
  '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --keyring-backend $KEYRING \
  -y

sleep 3

# ─── Step 6: Upload and instantiate Factory ─────────────────────────────────
FACTORY_CODE_ID=$(store_and_get_code_id "factory.wasm" "Factory")

echo ""
echo "Instantiating factory..."
FACTORY_INIT=$(cat <<EOF
{
  "factory_admin_address": "$ALICE_ADDR",
  "commit_amount_for_threshold_bluechip": "0",
  "commit_threshold_limit_usd": "25000",
  "pyth_contract_addr_for_conversions": "$ORACLE_ADDR",
  "pyth_atom_usd_price_feed_id": "ATOM_USD",
  "cw721_nft_contract_id": $CW721_CODE_ID,
  "cw20_token_contract_id": $CW20_CODE_ID,
  "create_pool_wasm_contract_id": $POOL_CODE_ID,
  "bluechip_wallet_address": "$ALICE_ADDR",
  "commit_fee_bluechip": "0.01",
  "commit_fee_creator": "0.05",
  "max_bluechip_lock_per_pool": "25000000000",
  "creator_excess_liquidity_lock_days": 7,
  "atom_bluechip_anchor_pool_address": "$ALICE_ADDR",
  "bluechip_mint_contract_address": "$ECON_ADDR"
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
echo "  Factory: $FACTORY_ADDR"

# ─── Step 7: Link expand economy to real factory ────────────────────────────
echo ""
echo "Linking expand economy to factory..."
bluechipChaind tx wasm execute $ECON_ADDR \
  "{\"update_config\":{\"factory_address\":\"$FACTORY_ADDR\",\"owner\":null}}" \
  --from $FROM \
  --chain-id $CHAIN_ID \
  --keyring-backend $KEYRING \
  -y
sleep 6

# ─── Step 8: Create a test pool via Factory ──────────────────────────────────
echo ""
echo "Creating test pool via factory..."

CREATE_POOL_MSG=$(cat <<EOF
{
  "create": {
    "pool_msg": {
      "pool_token_info": [
        { "bluechip": { "denom": "$DENOM" } },
        { "creator_token": { "contract_addr": "WILL_BE_CREATED_BY_FACTORY" } }
      ],
      "cw20_token_contract_id": $CW20_CODE_ID,
      "factory_to_create_pool_addr": "$FACTORY_ADDR",
      "threshold_payout": null,
      "commit_fee_info": {
        "bluechip_wallet_address": "$ALICE_ADDR",
        "creator_wallet_address": "$ALICE_ADDR",
        "commit_fee_bluechip": "0.01",
        "commit_fee_creator": "0.05"
      },
      "creator_token_address": "$ALICE_ADDR",
      "commit_amount_for_threshold": "0",
      "commit_limit_usd": "25000",
      "pyth_contract_addr_for_conversions": "$ORACLE_ADDR",
      "pyth_atom_usd_price_feed_id": "ATOM_USD",
      "max_bluechip_lock_per_pool": "10000000000",
      "creator_excess_liquidity_lock_days": 7,
      "is_standard_pool": false
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

# Get pool address from the transaction events
POOL_ADDR=$(bluechipChaind query tx $CREATE_POOL_TX --output json | jq -r '.events[] | select(.type == "wasm") | .attributes[] | select(.key == "pool_address") | .value')

echo ""
echo "Deployment Complete!"
echo "================================"
echo "Mock Oracle:     $ORACLE_ADDR"
echo "Expand Economy:  $ECON_ADDR"
echo "Factory:         $FACTORY_ADDR"
echo "Pool:            $POOL_ADDR"
echo "CW20 Code ID:    $CW20_CODE_ID"
echo "CW721 Code ID:   $CW721_CODE_ID"
echo "Native Denom:    $DENOM"
echo "================================"

# ─── Auto-generate frontend .env.local ──────────────────────────────────────
FRONTEND_ENV="frontend/.env.local"
cat > "$FRONTEND_ENV" <<ENVEOF
# Auto-generated by deploy_full_stack_mock_oracle.sh
VITE_FACTORY_ADDRESS=$FACTORY_ADDR
VITE_ORACLE_ADDRESS=$ORACLE_ADDR
VITE_POOL_ADDRESSES=$POOL_ADDR
ENVEOF
echo ""
echo "Frontend .env.local written to $FRONTEND_ENV"
echo ""
echo "To start frontend:  cd frontend && npm run dev"
echo ""
echo "To change prices:"
echo "  bluechipChaind tx wasm execute $ORACLE_ADDR '{\"set_price\":{\"price_id\":\"ATOM_USD\",\"price\":\"NEW_PRICE\"}}' --from alice --chain-id $CHAIN_ID --keyring-backend test -y"
