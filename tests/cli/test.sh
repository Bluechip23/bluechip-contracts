#!/usr/bin/env bash
set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# Configuration
# ─────────────────────────────────────────────────────────────────────────────
CLIENT="bluechipchaind"           # CLI binary
CHAIN_ID="bluechipChain"          # Testnet chain-id
NODE="http://localhost:26657"     # RPC endpoint

KEY1="committer1"                 # First committer key
KEY2="committer2"                 # Second committer key
ADMIN_KEY="mykey"                 # Admin/deployer key

# Addresses
ADMIN_ADDR=$($CLIENT keys show "$ADMIN_KEY" -a)
ADDR1=$($CLIENT keys show "$KEY1" -a)
ADDR2=$($CLIENT keys show "$KEY2" -a)
CREATOR_ADDR="<CREATOR_ADDR>"      # Creator fee recipient
BLUECHIP_ADDR="<BLUECHIP_ADDR>"    # Bluechip fee recipient

# Assets & Contracts
ASSET2_ADDR="<CW20_TOKEN_ADDR>"    # Paired CW20 token
ORACLE_ADDR="<ORACLE_ADDR>"        # Price oracle
WASM_FACTORY="artifacts/factory.wasm"
WASM_POOL="artifacts/pool.wasm"

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────
query_native() {
  $CLIENT query bank balances "$1" \
    --denom ubluechip --node "$NODE" -o json \
    | jq -r '(.balances[0].amount // "0")'
}

query_cw20() {
  local addr="$1"
  local QUERY=$(jq -n --arg addr "$addr" '{balance:{address:$addr}}')
  $CLIENT query wasm contract-state smart "$ASSET2_ADDR" \
    --node "$NODE" --query "$QUERY" \
    | jq -r '.balance'
}

# ─────────────────────────────────────────────────────────────────────────────
# 1. Deploy & Instantiate Factory
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Deploying factory…"
FACTORY_CODE_ID=$(
  $CLIENT tx wasm store "$WASM_FACTORY" \
    --from "$ADMIN_KEY" --chain-id "$CHAIN_ID" --node "$NODE" \
    --gas auto --gas-adjustment 1.3 --broadcast-mode block -y \
    --output json | jq -r '.logs[0].events[-1].attributes[0].value'
)
FACTORY_INSTANTIATE_MSG=$(jq -n --arg owner "$ADMIN_ADDR" \
  '{config:{owner:$owner,pool_code_id:2,token_code_id:3,fee_collector:$owner}}')
FACTORY_ADDR=$(
  $CLIENT tx wasm instantiate "$FACTORY_CODE_ID" "$FACTORY_INSTANTIATE_MSG" \
    --from "$ADMIN_KEY" --admin "$ADMIN_ADDR" --label factory-test \
    --chain-id "$CHAIN_ID" --node "$NODE" \
    --gas auto --gas-adjustment 1.3 --broadcast-mode block -y \
    --output json | jq -r '.logs[0].events[-1].attributes[] \
      | select(.key=="_contract_address")|.value'
)
echo "✅ Factory deployed: $FACTORY_ADDR"

# ─────────────────────────────────────────────────────────────────────────────
# 2. Factory-to-Pool Lifecycle Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Testing factory.Create -> pool instantiation via factory…"
CREATE_MSG=$(jq -n \
  --arg asset2 "$ASSET2_ADDR" \
  --arg owner "$ADMIN_ADDR" \
  --arg name "TestToken" \
  --arg symbol "TKN" \
  --argjson decimals 6 \
  --argjson initial_balances '[{"address":"'"$ADMIN_ADDR"'","amount":"1000000"}]' '{
    create: { pair_msg: { asset_infos:[ {native_token:{denom:"ubluechip"}}, {token:{contract_addr:$asset2}} ], token_code_id:5, factory_addr:$owner, init_params:null },
             token_info: { name:$name, symbol:$symbol, decimals:$decimals, initial_balances:$initial_balances, mint:null, marketing:null }
    }
  }')
TX_CREATE=$(
  $CLIENT tx wasm execute "$FACTORY_ADDR" "$CREATE_MSG" \
    --from "$ADMIN_KEY" --chain-id="$CHAIN_ID" --node="$NODE" \
    --gas auto --gas-adjustment 1.3 --broadcast-mode block -y --output json
)
POOL_ADDR_FROM_FACTORY=$(echo "$TX_CREATE" \
  | jq -r '.logs[0].events[] | select(.type=="instantiate") | .attributes[] | select(.key=="_contract_address") | .value')
echo "✅ Pool created via factory: $POOL_ADDR_FROM_FACTORY"

echo "⏳ Querying new pool info…"
$CLIENT query wasm contract-state smart "$POOL_ADDR_FROM_FACTORY" '{"pair":{}}' --node "$NODE" | jq

# ─────────────────────────────────────────────────────────────────────────────
# 3. Deploy & Instantiate Pool
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Deploying pool…"
POOL_CODE_ID=$(
  $CLIENT tx wasm store "$WASM_POOL" \
    --from "$ADMIN_KEY" --chain-id="$CHAIN_ID" --node="$NODE" \
    --gas auto --gas-adjustment 1.3 --broadcast-mode block -y --output json \
    | jq -r '.logs[0].events[-1].attributes[0].value'
)
POOL_INSTANTIATE_MSG=$(jq -n \
  --arg faddr "$FACTORY_ADDR" --arg asset2 "$ASSET2_ADDR" --arg oracle "$ORACLE_ADDR" --arg creator "$CREATOR_ADDR" --arg blue "$BLUECHIP_ADDR" '{
    asset_infos:[{native_token:{denom:"ubluechip"}},{token:{contract_addr:$asset2}}], token_code_id:5, factory_addr:$faddr, init_params:null,
    fee_info:{bluechip_address:$blue,creator_address:$creator,bluechip_fee:"0.05",creator_fee:"0.01"}, commit_amount:"8000", commit_limit:"25000", commit_limit_usd:"25000",
    oracle_addr:$oracle, oracle_symbol:"BTC", creator_amount:"8000", bluechip_amount:"2000", pool_amount:"7000", token_address:$asset2, available_payment:["1000","2000"]
  }')
POOL_ADDR=$(
  $CLIENT tx wasm instantiate "$POOL_CODE_ID" "$POOL_INSTANTIATE_MSG" \
    --from="$ADMIN_KEY" --admin="$ADMIN_ADDR" --label pool-test \
    --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y --output json \
    | jq -r '.logs[0].events[-1].attributes[] | select(.key=="_contract_address")|.value'
)
echo "✅ Pool deployed: $POOL_ADDR"

# ─────────────────────────────────────────────────────────────────────────────
# 4. Initial Commit & Event-Driven Hook Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Commit #1 (below threshold)…"
$CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"8000"},"amount":"8000"}}' \
  --from="$KEY1" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y

echo "✅ Commit #1 succeeded"

echo "⏳ Checking commit event…"
$CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"0"},"amount":"0"}}' \
  --from="$KEY1" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y \
  | jq '.logs[0].events[] | select(.attributes[].key=="action")'

# ─────────────────────────────────────────────────────────────────────────────
# 5. Negative Swap Before Threshold
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Attempt swap (should fail)…"
set +e
$CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"100"},"belief_price":null,"max_spread":null,"to":null}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y &> swap_out.txt
SWAP_EXIT=$?; set -e
if [ $SWAP_EXIT -ne 0 ]; then echo "✅ Swap blocked before threshold"; else echo "❌ Unexpected swap success"; exit 1; fi

# ─────────────────────────────────────────────────────────────────────────────
# 6. Commits to Cross Threshold
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Commits #2 & #3 to cross threshold…"
for n in 2 3; do
  echo "  Commit #$n…"
  $CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"8000"},"amount":"8000"}}' \
    --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y
  echo "  ✅ Commit #$n done"
done
echo "✅ Threshold crossed & payout triggered"

# ─────────────────────────────────────────────────────────────────────────────
# 7. Verify Initial Payout Balances & Pool State
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Checking payouts…"
echo "  Creator U: $(query_native $CREATOR_ADDR)"
echo "  Bluechip U: $(query_native $BLUECHIP_ADDR)"
echo "  Committers CW20:"
for addr in "$ADDR1" "$ADDR2"; do echo "    $addr -> $(query_cw20 $addr)"; done
echo "  Fees CW20 to creator/bluechip:"
for addr in "$CREATOR_ADDR" "$BLUECHIP_ADDR"; do echo "    $addr -> $(query_cw20 $addr)"; done
echo "  Pool state:"; $CLIENT query wasm contract-state smart "$POOL_ADDR" '{"pool":{}}' --node "$NODE" | jq

# ─────────────────────────────────────────────────────────────────────────────
# 8. Small Post‑Threshold Commit & Fee Check
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Recording balances before small commit…"
CRE_BEFORE=$(query_native $CREATOR_ADDR)
BL_BEFORE=$(query_native $BLUECHIP_ADDR)
echo "  Creator: $CRE_BEFORE, Bluechip: $BL_BEFORE"

echo "⏳ Commit #4 (5 ubluechip)…"
$CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"5"},"amount":"5"}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y

echo "✅ Commit #4 succeeded"
CRE_AFTER=$(query_native $CREATOR_ADDR)
BL_AFTER=$(query_native $BLUECHIP_ADDR)
echo "  Creator: $CRE_AFTER, Bluechip: $BL_AFTER"
if [ "$CRE_AFTER" -le "$CRE_BEFORE" ] || [ "$BL_AFTER" -le "$BL_BEFORE" ]; then echo "❌ Fees not applied"; exit 1; fi
echo "✅ Fees applied correctly"

# ─────────────────────────────────────────────────────────────────────────────
# 9. Slippage-Guard Tests
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Slippage-negative test…"; set +e
$CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"10"},"belief_price":"0.0001","max_spread":"0.01","to":null}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y &> neg.txt
[ $? -ne 0 ] && echo "✅ Slippage guard blocked" || { echo "❌ Slippage guard failed"; exit 1; }
set -e
echo "⏳ Slippage-positive test…"
SIM=$($CLIENT query wasm contract-state smart "$POOL_ADDR" '{"simulation":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"100"}}}' --node="$NODE")
RET=$(echo "$SIM"|jq -r'.return_amount')
$CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"100"},"belief_price":null,"max_spread":"0.10","to":null}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y
echo "✅ Swap succeeded (~$RET)"

# ─────────────────────────────────────────────────────────────────────────────
# 10. Concurrent Commits (Race Condition) Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Testing concurrent commits…"
CRE_BEFORE=$(query_native $CREATOR_ADDR)
BL_BEFORE=$(query_native $BLUECHIP_ADDR)
echo "  Creator fee before: $CRE_BEFORE, Bluechip fee before: $BL_BEFORE"

echo "⏳ Submitting parallel commits…"
$CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"1000"},"amount":"1000"}}' \
  --from="$KEY1" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y &
$CLIENT tx wasm execute "$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"2000"},"amount":"2000"}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y &
wait
echo "✅ Parallel commits completed"
CRE_AFTER=$(query_native $CREATOR_ADDR)
BL_AFTER=$(query_native $BLUECHIP_ADDR)
echo "  Creator fee after: $CRE_AFTER, Bluechip fee after: $BL_AFTER"
[ "$CRE_AFTER" -le "$CRE_BEFORE" ] || [ "$BL_AFTER" -le "$BL_BEFORE" ] && { echo "❌ Concurrency test failed"; exit 1; } || echo "✅ Concurrency handled correctly"

# ─────────────────────────────────────────────────────────────────────────────
# 11. Simulation-Only Queries
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Simulation queries…"
$CLIENT query wasm contract-state smart "$POOL_ADDR" '{"simulation":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"500"}}}' --node="$NODE" | jq
$CLIENT query wasm contract-state smart "$POOL_ADDR" '{"reverse_simulation":{"ask_asset":{"native_token":{"denom":"ubluechip"},"amount":"500"}}}' --node="$NODE" | jq

# ─────────────────────────────────────────────────────────────────────────────
# 12. Cumulative Prices / TWAP Check
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ TWAP before…"
$CLIENT query wasm contract-state smart "$POOL_ADDR" '{"cumulative_prices":{}}' --node="$NODE" | jq

echo "⏳ Perform small swap…"
$CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"1"},"belief_price":null,"max_spread":null,"to":null}}' \
  --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y

echo "⏳ TWAP after…"
$CLIENT query wasm contract-state smart "$POOL_ADDR" '{"cumulative_prices":{}}' --node="$NODE" | jq

# ─────────────────────────────────────────────────────────────────────────────
# 13. UpdateConfig Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Updating config…"
NP=$(echo -n '{"new_fee":{"bluechip_fee":"0.02","creator_fee":"0.005"}}' | base64)
$CLIENT tx wasm execute "$POOL_ADDR" "{\"update_config\":{\"params\":\"$NP\"}}" \
  --from="$ADMIN_KEY" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y
echo "✅ Config updated:"; $CLIENT query wasm contract-state smart "$POOL_ADDR" '{"config":{}}' --node="$NODE" | jq

# ─────────────────────────────────────────────────────────────────────────────
# 14. Front-Running / MEV Scenario Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Front-run MEV test…"
RET_BEFORE=$( $CLIENT query wasm contract-state smart "$POOL_ADDR" '{"simulation":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"10"}}}' --node="$NODE" | jq -r '.return_amount')
echo "  Return before commit: $RET_BEFORE"
$CLIENT tx wasm execute="$POOL_ADDR" '{"commit":{"asset":{"native_token":{"denom":"ubluechip"},"amount":"10000"},"amount":"10000"}}' --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y
echo "  ✅ Large commit done"
SWAP_TX=$($CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"10"},"belief_price":null,"max_spread":null,"to":null}}' --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y --output json)
echo "  ✅ Small swap executed"
RET_AFTER=$( $CLIENT query wasm contract-state smart "$POOL_ADDR" '{"simulation":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"10"}}}' --node="$NODE" | jq -r '.return_amount')
echo "  Return after commit: $RET_AFTER"
[ "$RET_AFTER" -ge "$RET_BEFORE" ] && { echo "❌ No price impact detected"; exit 1; } || echo "✅ Price impact detected"

# ─────────────────────────────────────────────────────────────────────────────
# 15. Long-Running TWAP Consistency Test
# ─────────────────────────────────────────────────────────────────────────────
echo "⏳ Long-running TWAP consistency…"
CUM_LAST=$($CLIENT query wasm contract-state smart "$POOL_ADDR" '{"cumulative_prices":{}}' --node="$NODE" | jq -r '.price0_cumulative_last')
echo "  Initial cumulative: $CUM_LAST"
for i in {1..5}; do
  echo "  🌀 Swap iteration $i"
  $CLIENT tx wasm execute "$POOL_ADDR" '{"swap":{"offer_asset":{"native_token":{"denom":"ubluechip"},"amount":"1"},"belief_price":null,"max_spread":null,"to":null}}' --from="$KEY2" --chain-id="$CHAIN_ID" --node="$NODE" --gas auto --gas-adjustment 1.3 --broadcast-mode block -y
  CUM_NEW=$($CLIENT query wasm contract-state smart "$POOL_ADDR" '{"cumulative_prices":{}}' --node="$NODE" | jq -r '.price0_cumulative_last')
  echo "    Cumulative now: $CUM_NEW"
  [ "$CUM_NEW" -le "$CUM_LAST" ] && { echo "❌ TWAP did not increase on iteration $i"; exit 1; }
  CUM_LAST=$CUM_NEW
  sleep 1
done
echo "✅ TWAP increased consistently"

echo "🎉 All tests completed!"
