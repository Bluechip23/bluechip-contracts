#!/bin/bash
# =====================================================================
# Robust Full Stack Deploy — Local Testing
# =====================================================================
# Downloads official CW20/CW721 base contracts, builds all custom
# contracts, and deploys everything to a local bluechipChain with
# mock oracle. Handles code ID extraction and tx retries.
#
# Prerequisites:
#   - Local chain running (bluechipChaind)
#   - Rust + wasm32-unknown-unknown target installed
#   - wget and jq installed
#
# Usage: bash deploy_robust.sh
# =====================================================================
set -e

CHAIN_ID="bluechipChain"
KEYRING="test"
FROM="alice"
ARTIFACTS="artifacts"
# Native denom — must match what your local chain genesis provides.
# The frontend expects "ubluechip". If your chain uses "stake", change this.
DENOM="${BLUECHIP_DENOM:-ubluechip}"

# ─── Helpers ─────────────────────────────────────────────────────────────────

# Filter warnings from bluechipChaind output
run_cmd() {
    "$@" 2>&1 | grep -v "WARNING"
}

# Run a wasm tx and return txhash
run_tx() {
    bluechipChaind tx wasm "$@" \
      --from $FROM \
      --chain-id $CHAIN_ID \
      --gas 5000000 --gas-adjustment 1.3 \
      --keyring-backend $KEYRING \
      -y --output json | grep -v "WARNING" | jq -r '.txhash'
}

# Query tx attribute with retries (for indexing delays)
get_tx_attr() {
    local TX_HASH=$1 TYPE=$2 KEY=$3
    for i in {1..5}; do
        local RES
        RES=$(bluechipChaind query tx "$TX_HASH" --output json 2>/dev/null | grep -v "WARNING")
        if [ -n "$RES" ]; then
            local VAL
            VAL=$(echo "$RES" | jq -r ".events[] | select(.type == \"$TYPE\") | .attributes[] | select(.key == \"$KEY\") | .value" 2>/dev/null)
            if [ -n "$VAL" ] && [ "$VAL" != "null" ]; then
                echo "$VAL"
                return
            fi
        fi
        sleep 2
    done
    echo "ERROR: Could not find attribute $KEY in tx $TX_HASH" >&2
    return 1
}

# Store contract and get code ID
store_contract() {
    local FILE=$1
    echo "Uploading $FILE..." >&2
    local TX
    TX=$(run_tx store "$ARTIFACTS/$FILE")
    sleep 6
    local CODE_ID
    CODE_ID=$(get_tx_attr "$TX" "store_code" "code_id")
    echo "  Uploaded $FILE as Code ID: $CODE_ID" >&2
    echo "$CODE_ID"
}

echo "Deploying Full Stack (Robust)..."

ALICE_ADDR=$(bluechipChaind keys show $FROM -a --keyring-backend $KEYRING | grep -v "WARNING")
echo "Alice: $ALICE_ADDR"

# ─── Step 0: Download base contracts ────────────────────────────────────────
echo ""
echo "Downloading base contracts..."
mkdir -p "$ARTIFACTS"
wget -q -O "$ARTIFACTS/cw20_base.wasm" https://github.com/CosmWasm/cw-plus/releases/download/v1.0.1/cw20_base.wasm
wget -q -O "$ARTIFACTS/cw721_base.wasm" https://github.com/CosmWasm/cw-nfts/releases/download/v0.18.0/cw721_base.wasm
echo "  Downloaded cw20_base.wasm and cw721_base.wasm"

# ─── Step 1: Build custom contracts ─────────────────────────────────────────
echo ""
echo "Building contracts..."
make build
echo "  Build complete"

# ─── Step 2: Upload all contracts ───────────────────────────────────────────
echo ""
echo "Uploading contracts..."
CW20_CODE_ID=$(store_contract "cw20_base.wasm")
CW721_CODE_ID=$(store_contract "cw721_base.wasm")
POOL_CODE_ID=$(store_contract "pool.wasm")
ORACLE_CODE_ID=$(store_contract "oracle.wasm")
ECON_CODE_ID=$(store_contract "expand_economy.wasm")
FACTORY_CODE_ID=$(store_contract "factory.wasm")

echo ""
echo "Code IDs: CW20=$CW20_CODE_ID  CW721=$CW721_CODE_ID  POOL=$POOL_CODE_ID"
echo "          ORACLE=$ORACLE_CODE_ID  EXP=$ECON_CODE_ID  FACTORY=$FACTORY_CODE_ID"

# ─── Step 3: Instantiate Mock Oracle ────────────────────────────────────────
echo ""
echo "Instantiating mock oracle..."
ORACLE_TX=$(run_tx instantiate "$ORACLE_CODE_ID" '{}' --label "mock_oracle" --no-admin)
sleep 6
ORACLE_ADDR=$(get_tx_attr "$ORACLE_TX" "instantiate" "_contract_address")
echo "  Mock Oracle: $ORACLE_ADDR"

# ─── Step 4: Instantiate Expand Economy ─────────────────────────────────────
echo ""
echo "Instantiating expand economy..."
ECON_TX=$(run_tx instantiate "$ECON_CODE_ID" "{\"factory_address\":\"$ALICE_ADDR\",\"owner\":\"$ALICE_ADDR\"}" --label "expand_economy" --no-admin)
sleep 6
ECON_ADDR=$(get_tx_attr "$ECON_TX" "instantiate" "_contract_address")
echo "  Expand Economy: $ECON_ADDR"

# Fund expand economy
echo "Funding expand economy..."
bluechipChaind tx bank send "$ALICE_ADDR" "$ECON_ADDR" "1000000000${DENOM}" \
  --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 6

# ─── Step 5: Set oracle price ───────────────────────────────────────────────
echo ""
echo "Setting ATOM/USD price to \$10..."
bluechipChaind tx wasm execute "$ORACLE_ADDR" \
  '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' \
  --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 3

# ─── Step 6: Instantiate Factory ────────────────────────────────────────────
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

FACTORY_TX=$(run_tx instantiate "$FACTORY_CODE_ID" "$FACTORY_INIT" --label "factory" --admin "$ALICE_ADDR")
sleep 6
FACTORY_ADDR=$(get_tx_attr "$FACTORY_TX" "instantiate" "_contract_address")
echo "  Factory: $FACTORY_ADDR"

# ─── Step 7: Link expand economy to factory (48h timelock) ──────────────────
echo ""
echo "Proposing expand economy config update to link to factory..."
echo "NOTE: Config update has a 48-hour timelock. Execute after the timelock expires."
bluechipChaind tx wasm execute "$ECON_ADDR" \
  "{\"propose_config_update\":{\"factory_address\":\"$FACTORY_ADDR\",\"owner\":null}}" \
  --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y | grep -v "WARNING"
sleep 6

echo ""
echo "IMPORTANT: After 48 hours, run this command to apply the config update:"
echo "  bluechipChaind tx wasm execute $ECON_ADDR '{\"execute_config_update\":{}}' --from $FROM --chain-id $CHAIN_ID --keyring-backend $KEYRING -y"

# ─── Step 8: Create test pool ───────────────────────────────────────────────
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

POOL_TX=$(run_tx execute "$FACTORY_ADDR" "$CREATE_POOL_MSG")
sleep 6
POOL_ADDR=$(get_tx_attr "$POOL_TX" "wasm" "pool_address")

echo ""
echo "Deployment Complete!"
echo "================================"
echo "Mock Oracle:     $ORACLE_ADDR"
echo "Expand Economy:  $ECON_ADDR"
echo "Factory:         $FACTORY_ADDR"
echo "Pool:            $POOL_ADDR"
echo "Native Denom:    $DENOM"
echo "================================"
echo ""
echo "Code IDs: CW20=$CW20_CODE_ID  CW721=$CW721_CODE_ID  POOL=$POOL_CODE_ID"
echo "          ORACLE=$ORACLE_CODE_ID  EXP=$ECON_CODE_ID  FACTORY=$FACTORY_CODE_ID"

# ─── Auto-generate frontend .env.local ──────────────────────────────────────
FRONTEND_ENV="frontend/.env.local"
cat > "$FRONTEND_ENV" <<ENVEOF
# Auto-generated by deploy_robust.sh
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
