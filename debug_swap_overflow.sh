#!/bin/bash
POOL_ADDR="cosmos1g3cgv3z2smd7vrjr3s639ewnwyd5dz4lg0uy9fadhkumca4suqcqzrdgdu"
TOKEN_ADDR="cosmos1quce89l8clsn8s5tmq5sylg370h58xfnkwadx72crjv90jmetp4sjv3h8m"
FROM="alice"

echo "🔄 Test Force Error: Call Deposit Liquidity via CW20 Hook (Zero Funds)"
# Hypothesis: User submits a 'deposit_liquidity' hook 
# telling it to use 10,000,000 stake (amount0).
# But since it's a CW20 hook, no native funds (0 stake) are attached.
# Result: 0 - 10,000,000 -> Overflow.

DEPOSIT_HOOK='{"deposit_liquidity":{"amount0":"10000000"}}'
HOOK_MSG_BASE64=$(echo -n $DEPOSIT_HOOK | base64 -w 0)

# We send 0 tokens (or any amount) with the hook.
SEND_MSG='{"send":{"contract":"'$POOL_ADDR'","amount":"1000000","msg":"'$HOOK_MSG_BASE64'"}}'

echo "Executing Malformed Deposit..."
bluechipChaind tx wasm execute $TOKEN_ADDR "$SEND_MSG" \
  --from $FROM \
  --chain-id bluechipChain \
  --gas auto --gas-adjustment 1.3 \
  -y
