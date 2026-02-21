#!/bin/bash
set -e

# ============================================================
# Deployment addresses (Feb 20 2026)
# ============================================================
ALICE_ADDR="cosmos1cyyzpxplxdzkeea7kwsydadg87357qnalx9dqz"
ORACLE_ADDR="cosmos1v3p8d7s3l2gre0v4w2tpfyv97vkw8najurywa99wedgzrtwnv22s5z98sv"
EXP_ADDR="cosmos19zn5nn40hqgwqddctsfn9cpyfw9qzdtlxtgwr28tq072s82v7tgslzlkfp"
FACTORY_ADDR="cosmos1u2zd83434en7nval8n8zh4ylm60fpg0falk8l2ua5zhcn9talzqqxllncz"
CHAIN_ID="bluechipChain"
KEYRING="test"
FROM="alice"

qry() {
  bluechipChaind query wasm contract-state smart "$1" "$2" --output json 2>/dev/null
}

exe() {
  local CONTRACT="$1"
  local MSG="$2"
  local FUNDS="$3"
  if [ -n "$FUNDS" ]; then
    bluechipChaind tx wasm execute "$CONTRACT" "$MSG" \
      --amount "$FUNDS" \
      --from "$FROM" --chain-id "$CHAIN_ID" \
      --keyring-backend "$KEYRING" --gas auto --gas-adjustment 1.3 \
      -y --output json 2>/dev/null
  else
    bluechipChaind tx wasm execute "$CONTRACT" "$MSG" \
      --from "$FROM" --chain-id "$CHAIN_ID" \
      --keyring-backend "$KEYRING" --gas auto --gas-adjustment 1.3 \
      -y --output json 2>/dev/null
  fi
}

refresh_oracle() {
  echo "  [Refreshing oracle: 1 ATOM = \$10]"
  exe "$ORACLE_ADDR" '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' > /dev/null
  sleep 5
}

wait_tx() {
  local TXHASH="$1"
  sleep 8
  bluechipChaind query tx "$TXHASH" --output json 2>/dev/null
}

echo "======================================================"
echo "  BLUECHIP LOCAL FULL STACK TEST  (Feb 20 2026)"
echo "======================================================"
echo "  Alice:          $ALICE_ADDR"
echo "  Mock Oracle:    $ORACLE_ADDR  (Code 15)"
echo "  Expand Economy: $EXP_ADDR    (Code 16)"
echo "  Factory:        $FACTORY_ADDR (Code 17)"
echo "  CW20=1, CW721=2 (pre-existing)"
echo ""

# ============================================================
# STEP 1: Update Expand Economy → real factory address
# ============================================================
echo "=== STEP 1: Update Expand Economy Config ==="
refresh_oracle
TX1_JSON=$(exe "$EXP_ADDR" "{\"update_config\":{\"factory_address\":\"$FACTORY_ADDR\"}}")
TXHASH1=$(echo "$TX1_JSON" | jq -r '.txhash // "none"')
echo "  TX: $TXHASH1"
wait_tx "$TXHASH1" > /dev/null
echo "  Expand Economy config now:"
qry "$EXP_ADDR" '{"get_config":{}}'

# ============================================================
# STEP 2: Create Pool via Factory
# ============================================================
echo ""
echo "=== STEP 2: Create Pool via Factory (CW20 + CW721 + Pool) ==="
refresh_oracle

CREATE_MSG=$(cat <<ENDOFMSG
{
  "create": {
    "pool_msg": {
      "pool_token_info": [
        {"bluechip": {"denom": "stake"}},
        {"creator_token": {"contract_addr": "WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id": 1,
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
      "max_bluechip_lock_per_pool": "25000000000",
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
ENDOFMSG
)

TX2_JSON=$(exe "$FACTORY_ADDR" "$CREATE_MSG")
TXHASH2=$(echo "$TX2_JSON" | jq -r '.txhash // "none"')
echo "  Create Pool TX: $TXHASH2"
TX2_RESULT=$(wait_tx "$TXHASH2")

# Extract addresses from tx events
POOL_ADDR=$(echo "$TX2_RESULT" | jq -r '.events[].attributes[] | select(.key == "pool_address") | .value' 2>/dev/null | head -1)
CREATOR_TOKEN=$(echo "$TX2_RESULT" | jq -r '.events[].attributes[] | select(.key == "token_address" or .key == "creator_token_address") | .value' 2>/dev/null | head -1)

# Fallback: get last contract with code 14
if [ -z "$POOL_ADDR" ] || [ "$POOL_ADDR" = "null" ]; then
  POOL_ADDR=$(bluechipChaind query wasm list-contract-by-code 14 --output json 2>/dev/null | jq -r '.contracts[-1]')
fi

echo "  Pool:          $POOL_ADDR"
echo "  Creator Token: $CREATOR_TOKEN"
echo "  Pool state:"
qry "$POOL_ADDR" '{"pool_state":{}}'
echo "  Pair info:"
qry "$POOL_ADDR" '{"pair":{}}'

# ============================================================
# STEP 3: Commit Logic
# ============================================================
echo ""
echo "=== STEP 3: Commit Logic ==="
echo "  Threshold: 25000 USD units | Oracle: 1 stake = \$10 → need 2500+ stake to cross"

COMMIT_MSG='{"commit":{"deadline":9999999999,"belief_price":"10000000","max_spread":"0.99"}}'

echo ""
echo "  --- Commit #1: 500 stake (\$5000 USD) ---"
refresh_oracle
TX_C1=$(exe "$POOL_ADDR" "$COMMIT_MSG" "500stake")
TXHASH_C1=$(echo "$TX_C1" | jq -r '.txhash // "none"')
echo "  TX: $TXHASH_C1"
C1_RESULT=$(wait_tx "$TXHASH_C1")
echo "  raw_log: $(echo "$C1_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"
echo "  Is fully committed:"
qry "$POOL_ADDR" '{"is_fully_commited":{}}'

echo ""
echo "  --- Commit #2: 1000 stake (\$10000 USD, total \$15000) ---"
refresh_oracle
TX_C2=$(exe "$POOL_ADDR" "$COMMIT_MSG" "1000stake")
TXHASH_C2=$(echo "$TX_C2" | jq -r '.txhash // "none"')
echo "  TX: $TXHASH_C2"
C2_RESULT=$(wait_tx "$TXHASH_C2")
echo "  raw_log: $(echo "$C2_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"
echo "  Pool state:"
qry "$POOL_ADDR" '{"pool_state":{}}'

echo ""
echo "  --- Commit #3: 1200 stake (\$12000 USD → total \$27000 > \$25000 CROSSES THRESHOLD) ---"
refresh_oracle
TX_C3=$(exe "$POOL_ADDR" "$COMMIT_MSG" "1200stake")
TXHASH_C3=$(echo "$TX_C3" | jq -r '.txhash // "none"')
echo "  TX: $TXHASH_C3"
C3_RESULT=$(wait_tx "$TXHASH_C3")
echo "  raw_log: $(echo "$C3_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 300)"
echo "  Is fully committed (should be true):"
qry "$POOL_ADDR" '{"is_fully_commited":{}}'
echo "  Pool state after threshold:"
qry "$POOL_ADDR" '{"pool_state":{}}'

# Continue batched distribution if needed
echo ""
echo "  --- Continuing distribution (up to 3 rounds) ---"
for i in 1 2 3; do
  refresh_oracle
  TX_DIST=$(exe "$POOL_ADDR" '{"continue_distribution":{}}')
  TXHASH_DIST=$(echo "$TX_DIST" | jq -r '.txhash // "none"')
  echo "  ContinueDistribution #$i: $TXHASH_DIST"
  DIST_RESULT=$(wait_tx "$TXHASH_DIST")
  DIST_LOG=$(echo "$DIST_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 150)
  echo "  log: $DIST_LOG"
  # Stop if distribution is complete
  if echo "$DIST_LOG" | grep -qi "complete\|finished\|no.*commit\|not.*distribut" 2>/dev/null; then
    break
  fi
done

echo ""
echo "  Pool commits ledger:"
qry "$POOL_ADDR" '{"pool_commits":{"min_payment_usd":"0"}}'
echo "  Alice commit info:"
qry "$POOL_ADDR" "{\"commiting_info\":{\"wallet\":\"$ALICE_ADDR\"}}"

# ============================================================
# STEP 4: Liquidity Logic
# ============================================================
echo ""
echo "=== STEP 4: Liquidity Logic ==="

echo "  Alice bank balances:"
bluechipChaind query bank balances "$ALICE_ADDR" 2>/dev/null | grep -v WARNING

if [ -n "$CREATOR_TOKEN" ] && [ "$CREATOR_TOKEN" != "null" ]; then
  echo "  Alice creator token balance:"
  qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE_ADDR\"}}"
fi

echo ""
echo "  --- Depositing Liquidity (500 stake) ---"
refresh_oracle
LIQ_MSG='{"deposit_liquidity":{"amount0":"500","amount1":"0","min_out0":"0","min_out1":"0","deadline":9999999999}}'
TX_LIQ=$(exe "$POOL_ADDR" "$LIQ_MSG" "500stake")
TXHASH_LIQ=$(echo "$TX_LIQ" | jq -r '.txhash // "none"')
echo "  Deposit TX: $TXHASH_LIQ"
LIQ_RESULT=$(wait_tx "$TXHASH_LIQ")
echo "  raw_log: $(echo "$LIQ_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"
echo "  Alice positions:"
qry "$POOL_ADDR" "{\"positions_by_owner\":{\"owner\":\"$ALICE_ADDR\"}}"

# Get first position ID
POSITIONS_JSON=$(qry "$POOL_ADDR" "{\"positions_by_owner\":{\"owner\":\"$ALICE_ADDR\"}}")
POS_ID=$(echo "$POSITIONS_JSON" | jq -r '.positions[0].id // .positions[0].token_id // empty' 2>/dev/null | head -1)

if [ -n "$POS_ID" ] && [ "$POS_ID" != "null" ]; then
  echo ""
  echo "  --- Collect Fees (position $POS_ID) ---"
  refresh_oracle
  TX_FEES=$(exe "$POOL_ADDR" "{\"collect_fees\":{\"position_id\":$POS_ID}}")
  TXHASH_FEES=$(echo "$TX_FEES" | jq -r '.txhash // "none"')
  echo "  CollectFees TX: $TXHASH_FEES"
  FEES_RESULT=$(wait_tx "$TXHASH_FEES")
  echo "  raw_log: $(echo "$FEES_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"

  echo ""
  echo "  --- Add To Position (100 stake) ---"
  refresh_oracle
  ADD_MSG="{\"add_to_position\":{\"position_id\":$POS_ID,\"amount0\":\"100\",\"amount1\":\"0\",\"min_out0\":\"0\",\"min_out1\":\"0\",\"deadline\":9999999999}}"
  TX_ADD=$(exe "$POOL_ADDR" "$ADD_MSG" "100stake")
  TXHASH_ADD=$(echo "$TX_ADD" | jq -r '.txhash // "none"')
  echo "  AddToPosition TX: $TXHASH_ADD"
  ADD_RESULT=$(wait_tx "$TXHASH_ADD")
  echo "  raw_log: $(echo "$ADD_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"

  echo ""
  echo "  --- Partial Remove Liquidity (100 units) ---"
  refresh_oracle
  RM_MSG="{\"remove_partial_liquidity\":{\"position_id\":$POS_ID,\"liquidity_to_remove\":\"100\",\"min_out0\":\"0\",\"min_out1\":\"0\",\"deadline\":9999999999}}"
  TX_RM=$(exe "$POOL_ADDR" "$RM_MSG")
  TXHASH_RM=$(echo "$TX_RM" | jq -r '.txhash // "none"')
  echo "  RemovePartial TX: $TXHASH_RM"
  RM_RESULT=$(wait_tx "$TXHASH_RM")
  echo "  raw_log: $(echo "$RM_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 200)"
else
  echo "  No positions found yet (may need threshold to be crossed first)"
fi

# ============================================================
# STEP 5: Swap Logic
# ============================================================
echo ""
echo "=== STEP 5: Swap Logic ==="
echo "  Note: swaps only work post-threshold"
refresh_oracle
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"native_token":{"denom":"stake"}},"amount":"100"},"belief_price":"10000000","max_spread":"0.99","deadline":9999999999}}'
TX_SWAP=$(exe "$POOL_ADDR" "$SWAP_MSG" "100stake")
TXHASH_SWAP=$(echo "$TX_SWAP" | jq -r '.txhash // "none"')
echo "  Swap TX: $TXHASH_SWAP"
SWAP_RESULT=$(wait_tx "$TXHASH_SWAP")
echo "  raw_log: $(echo "$SWAP_RESULT" | jq -r '.raw_log // "ok"' 2>/dev/null | head -c 300)"
echo "  Pool state after swap:"
qry "$POOL_ADDR" '{"pool_state":{}}'

# ============================================================
# FINAL SUMMARY
# ============================================================
echo ""
echo "======================================================"
echo "  FULL STACK TEST COMPLETE"
echo "======================================================"
echo "  Pool:          $POOL_ADDR"
echo "  Creator Token: $CREATOR_TOKEN"
echo "  Factory:       $FACTORY_ADDR"
echo "  Expand Econ:   $EXP_ADDR"
echo "  Oracle:        $ORACLE_ADDR"
echo "======================================================"
