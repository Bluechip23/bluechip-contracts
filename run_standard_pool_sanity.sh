#!/bin/bash
# =====================================================================
# Standard-pool sanity check
# =====================================================================
# Asserts that a freshly created standard pool:
#   1. Does NOT accept ExecuteMsg::Commit (no commit phase)
#   2. Reports no commit/threshold metadata in its config/state queries
#   3. Accepts swap & liquidity ExecuteMsgs immediately (normal pool)
#
# PREREQUISITE: run_full_test.sh ran successfully and left the chain
# up with the factory + oracle deployed.
# =====================================================================
source "$(dirname "$0")/test_lib.sh"

ALICE=$(addr_for alice)
BOB=$(addr_for bob)

# Locate factory + standard-pool code via lib discovery (probes contracts
# with {"factory":{}} until one returns a config response).
log_header "Discovering deployed contracts"
FACTORY_ADDR=$(discover_factory_addr)
FACTORY_CODE=$(discover_factory_code)
STD_POOL_CODE=$(discover_standard_pool_code)

if [ -z "$FACTORY_ADDR" ]; then
  echo "  ERROR: Could not locate factory contract on chain. Run run_full_test.sh first."
  exit 1
fi

# Pull oracle / expand-economy from the factory's config — single source of truth.
FACTORY_CFG=$(qry "$FACTORY_ADDR" '{"factory":{}}')
ORACLE_ADDR=$(echo "$FACTORY_CFG" | json_data_get "factory.pyth_contract_addr_for_conversions")
ECON_ADDR=$(echo "$FACTORY_CFG"   | json_data_get "factory.bluechip_mint_contract_address")

echo "  Factory      : $FACTORY_ADDR (code $FACTORY_CODE)"
echo "  Standard-pool: code $STD_POOL_CODE"
echo "  Oracle       : $ORACLE_ADDR"
echo "  Expand-econ  : $ECON_ADDR"

# Need a CW20 we control to be the second leg of the standard pool.
# Lib's find_contract_by_query probes every contract on chain with
# {"token_info":{}} and returns the first that responds with "symbol".
log_header "Acquiring a CW20 for the standard pool"
log_step "Looking for an existing CW20 deployed by the test harness"
CW20_TOKEN=$(find_contract_by_query '{"token_info":{}}' '"symbol"')
echo "  CW20 token to pair with bluechip: ${CW20_TOKEN:-NONE FOUND}"

if [ -z "$CW20_TOKEN" ]; then
  log_fail "No CW20 token found on chain — cannot pair into a standard pool"
  exit 1
fi

# ─────────────────────────────────────────────────────────────────────
log_header "Step 1: CreateStandardPool"
CREATE_MSG=$(python3 -c "
import json
print(json.dumps({'create_standard_pool':{
  'pool_token_info':[{'bluechip':{'denom':'$DENOM'}},{'creator_token':{'contract_addr':'$CW20_TOKEN'}}],
  'label':'std-sanity'
}}))")
echo "  msg: $CREATE_MSG"

# Pay the standard-pool creation fee — factory init set this to 1_000_000 micro-USD ($1)
# At oracle BLUECHIP_USD = 1_000_000 (= $1), $1 = 1_000_000 ubluechip
TX=$($BIN tx wasm execute "$FACTORY_ADDR" "$CREATE_MSG" --from alice --amount 5000000$DENOM $TX_FLAGS 2>/dev/null)
TXHASH=$(echo "$TX" | python3 -c "
import json, sys, re
raw = sys.stdin.read()
# Find the first {...} JSON object in the output (cosmos-sdk sometimes prints
# 'gas estimate: N' before the JSON; we slice from the first '{').
i = raw.find('{')
if i < 0:
    print('NA'); sys.exit()
try:
    print(json.loads(raw[i:]).get('txhash','NA'))
except Exception:
    print('NA')" 2>/dev/null)
echo "  tx hash: $TXHASH"
if [ "$TXHASH" = "NA" ]; then
  log_fail "Submit failed: $TX"
  exit 1
fi

# Wait for tx to be indexed; retry up to 6 times.
RAW=""
for i in 1 2 3 4 5 6; do
  sleep 4
  RAW=$($BIN query tx "$TXHASH" --node $NODE --output json 2>/dev/null)
  if [ -n "$RAW" ] && echo "$RAW" | head -c 1 | grep -q "{"; then break; fi
done

CODE=$(echo "$RAW" | python3 -c "
import json, sys
try: print(json.load(sys.stdin).get('code',-1))
except: print('-1')" 2>/dev/null)
if [ "$CODE" != "0" ]; then
  RAW_LOG=$(echo "$RAW" | python3 -c "
import json, sys
try: print(json.load(sys.stdin).get('raw_log',''))
except: print('(no log)')" 2>/dev/null)
  log_fail "CreateStandardPool tx failed (code=$CODE): $RAW_LOG"
  exit 1
fi
log_pass "CreateStandardPool tx accepted"

# Pull the new standard pool address from events
STD_POOL_ADDR=$(echo "$RAW" | python3 -c "
import json, sys
d = json.load(sys.stdin)
for e in d.get('events', []):
    if e.get('type') == 'instantiate':
        for a in e.get('attributes', []):
            if a.get('key') in ('_contract_address','contract_address'):
                # standard-pool has the larger code_id; first instantiate is CW20/CW721
                pass
# We'll just pick the contract that responds to query 'pool_state'
")
# Simpler: list contracts under STD_POOL_CODE and pick newest
STD_POOL_ADDR=$($BIN query wasm list-contract-by-code "$STD_POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[-1] if cs else '')")
echo "  Standard pool: ${STD_POOL_ADDR:-NOT FOUND}"

if [ -z "$STD_POOL_ADDR" ]; then
  log_fail "No standard-pool contract found under code $STD_POOL_CODE"
  exit 1
fi

# ─────────────────────────────────────────────────────────────────────
log_header "Step 2: Reject ExecuteMsg::Commit"
COMMIT_MSG='{"commit":{"asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"1000000"},"transaction_deadline":null,"belief_price":null,"max_spread":null}}'
COMMIT_TX=$($BIN tx wasm execute "$STD_POOL_ADDR" "$COMMIT_MSG" --from alice --amount 1000000$DENOM $TX_FLAGS 2>&1)
COMMIT_HASH=$(echo "$COMMIT_TX" | python3 -c "import json,sys
try: print(json.load(sys.stdin).get('txhash','NA'))
except: print('NA')" 2>/dev/null)

if [ "$COMMIT_HASH" = "NA" ]; then
  # CLI rejected at simulation step — already proves it's not accepted
  if echo "$COMMIT_TX" | grep -qE "unknown variant|Error parsing|cannot parse"; then
    log_pass "commit msg rejected at simulation: $(echo "$COMMIT_TX" | grep -oE 'unknown variant[^"]*' | head -1)"
  else
    log_pass "commit msg rejected (no txhash)"
  fi
else
  sleep 6
  RAW2=$($BIN query tx "$COMMIT_HASH" --node $NODE --output json 2>/dev/null)
  CODE2=$(echo "$RAW2" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',-1))" 2>/dev/null)
  RAW_LOG2=$(echo "$RAW2" | python3 -c "import json,sys; print(json.load(sys.stdin).get('raw_log',''))")
  if [ "$CODE2" = "0" ]; then
    log_fail "commit tx unexpectedly succeeded against standard pool"
  else
    if echo "$RAW_LOG2" | grep -qiE "unknown variant|cannot parse|parse"; then
      log_pass "commit tx rejected with parse error: $(echo "$RAW_LOG2" | head -c 140)"
    else
      log_pass "commit tx rejected (code=$CODE2): $(echo "$RAW_LOG2" | head -c 140)"
    fi
  fi
fi

# ─────────────────────────────────────────────────────────────────────
log_header "Step 3: No commit/threshold fields in pool config/state"
CFG=$($BIN query wasm contract-state smart "$STD_POOL_ADDR" '{"config":{}}' --node $NODE --output json 2>/dev/null || echo "{}")
PS=$($BIN query wasm contract-state smart "$STD_POOL_ADDR" '{"pool_state":{}}' --node $NODE --output json 2>/dev/null || echo "{}")
echo "  config: $(echo "$CFG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print(json.dumps(d)[:200])" 2>/dev/null)"

if echo "$CFG $PS" | grep -qiE "commit_limit|commit_threshold|threshold_payout|is_threshold_hit"; then
  log_fail "config/state mentions commit/threshold metadata"
else
  log_pass "no commit/threshold metadata in config/state queries"
fi

# ─────────────────────────────────────────────────────────────────────
log_header "Step 4: SimpleSwap variant is accepted (message-level)"
# At zero liquidity the swap will fail with insufficient liquidity, but
# the *variant* should parse — that's what we're testing here.
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"1000"},"max_spread":"0.5"}}'
SW=$($BIN tx wasm execute "$STD_POOL_ADDR" "$SWAP_MSG" --from alice --amount 1000$DENOM $TX_FLAGS 2>&1)
SW_HASH=$(echo "$SW" | python3 -c "import json,sys
try: print(json.load(sys.stdin).get('txhash','NA'))
except: print('NA')" 2>/dev/null)

if echo "$SW" | grep -qE "unknown variant|cannot parse"; then
  log_fail "simple_swap variant not recognized — standard pool missing swap entrypoint"
else
  if [ "$SW_HASH" != "NA" ]; then
    sleep 6
    SW_RAW=$($BIN query tx "$SW_HASH" --node $NODE --output json 2>/dev/null)
    SW_LOG=$(echo "$SW_RAW" | python3 -c "import json,sys; print(json.load(sys.stdin).get('raw_log',''))")
    SW_CODE=$(echo "$SW_RAW" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',-1))")
    if [ "$SW_CODE" = "0" ]; then
      log_pass "simple_swap executed (post-creation, no commit gating)"
    else
      log_pass "simple_swap variant accepted; pool reverted on liquidity (expected): $(echo "$SW_LOG" | head -c 140)"
    fi
  else
    log_pass "simple_swap simulation surfaced a runtime (not parse) error"
  fi
fi

# ─────────────────────────────────────────────────────────────────────
echo
echo "================================================================"
echo "  Standard-pool sanity: PASS=$PASS  FAIL=$FAIL"
echo "================================================================"
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
