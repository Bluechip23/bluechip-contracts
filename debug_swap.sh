#!/bin/bash
POOL_ADDR="cosmos19frgzsmvj5ylyf7xnxrfxst2h53s3crpwkd26lycurgp69jgjxas04x8l9"
TOKEN_ADDR="cosmos1fweq070y8lvxmtn7j3p7yltphg8uuksk7p875suwppyzg259rt0sfq8m85"
FROM="alice"

echo "🔍 Querying Pool State..."
bluechipChaind query wasm contract-state smart $POOL_ADDR '{"pool_state":{}}'

echo ""
echo "🔄 Attempting Swap (CW20 -> Native)..."
# Swap 100 tokens
MSG=$(echo -n '{"swap":{}}' | base64 -w 0)
bluechipChaind tx wasm execute $TOKEN_ADDR \
  "{\"send\":{\"contract\":\"$POOL_ADDR\",\"amount\":\"100\",\"msg\":\"$MSG\"}}" \
  --from $FROM \
  --chain-id bluechipChain \
  --gas auto --gas-adjustment 1.3 \
  -y
