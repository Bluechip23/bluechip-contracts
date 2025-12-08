#!/bin/bash
POOL_ADDR="cosmos1g3cgv3z2smd7vrjr3s639ewnwyd5dz4lg0uy9fadhkumca4suqcqzrdgdu"
TOKEN_ADDR="cosmos1quce89l8clsn8s5tmq5sylg370h58xfnkwadx72crjv90jmetp4sjv3h8m"
FROM="alice"

echo "🔍 Querying Pool State..."
bluechipChaind query wasm contract-state smart $POOL_ADDR '{"pool_state":{}}'

echo ""
echo "🔄 Test 1: Matching Ratio (10 stake : 35M tokens)"
# 10 stake (10000000 u-stake? No, 10 stake = 10000000)
# Wait, let's use small numbers consistent with previous logs.
# Reserve0 is 10010. Reserve1 is 349651398602.
# Ratio ~34,930,209.
# Let's try 10 stake (10) and 350,000,000 tokens.
# 10 * 35M = 350M.
# Wait, reserve0 is 10010 (micro? or whole?)
# The deployment script used 10stake (which is 10000000 if 1stake=10^6).
# But reserve0 says 10010.
# Ah! In deploy script:
# DEPOSIT_MSG='{"deposit_liquidity":{"amount0":"10","amount1":"350000000"}}'
# --amount 10stake.
# If 10stake = 10,000,000.
# And `amount0` in Msg is "10".
# Then `paid` (10,000,000) >>> `actual` (10).
# So it passes.

# BUT if User in Frontend enters "1" (amount0).
# Frontend converts to micro: 1 * 1,000,000 = 1,000,000.
# Msg amount0 = 1,000,000.
# Attached amount = 1,000,000 (stake).
# `paid` = 1,000,000. `actual` <= 1,000,000.
# Should pass.

# What if user enters "0.000001" (1 micro unit)?
# Frontend: 1.
# Msg: 1. Attached: 1.

# Let's verify what "10stake" means in the script.
# --amount 10stake.
# bluechipChaind uses the literal string.
# If bank has "stake", then 10stake is 10 units.

# If reserve0 is 10010, and deployed with "10stake" and msg "10".
# Then 10 units were deposited.

# Checking Test 1:
echo "Approving..."
APPROVE_MSG='{"increase_allowance":{"spender":"'$POOL_ADDR'","amount":"9984614792"}}'
bluechipChaind tx wasm execute $TOKEN_ADDR "$APPROVE_MSG" --from $FROM --chain-id bluechipChain --gas auto --gas-adjustment 1.3 -y
sleep 6

echo "Depositing with mismatch..."
# Token amount 9.9B requires ~300 stake.
# We send 10 stake (amount0="10" in msg, --amount 10stake).
# This should trigger actual > paid.
DEPOSIT_MSG='{"deposit_liquidity":{"amount0":"10","amount1":"9984614792"}}'
bluechipChaind tx wasm execute $POOL_ADDR "$DEPOSIT_MSG" \
  --amount 10stake \
  --from $FROM \
  --chain-id bluechipChain \
  --gas auto --gas-adjustment 1.3 \
  -y
