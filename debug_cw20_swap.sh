#!/bin/bash
POOL_ADDR="cosmos1g3cgv3z2smd7vrjr3s639ewnwyd5dz4lg0uy9fadhkumca4suqcqzrdgdu"
TOKEN_ADDR="cosmos1quce89l8clsn8s5tmq5sylg370h58xfnkwadx72crjv90jmetp4sjv3h8m"
FROM="alice"

echo "🔍 Querying Pool State..."
bluechipChaind query wasm contract-state smart $POOL_ADDR '{"pool_state":{}}'

echo "🔄 Test Swap: 50,000,000 CW20 (50.0 Token) -> Stake"
# 50M CW20 -> ~1.5 Stake.
# This should succeed.

HOOK_MSG='{"swap":{"max_spread":"0.005"}}'
HOOK_MSG_BASE64=$(echo -n $HOOK_MSG | base64 -w 0)

SEND_MSG='{"send":{"contract":"'$POOL_ADDR'","amount":"50000000","msg":"'$HOOK_MSG_BASE64'"}}'

echo "Executing Swap..."
bluechipChaind tx wasm execute $TOKEN_ADDR "$SEND_MSG" \
  --from $FROM \
  --chain-id bluechipChain \
  --gas auto --gas-adjustment 1.3 \
  -y
