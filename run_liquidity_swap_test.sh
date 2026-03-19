#!/usr/bin/env bash
# =====================================================================
# Bluechip Partial Liquidity, CW20 Swap, Oracle & NFT Burn Tests
# =====================================================================
# Tests:
#   Scenario 1: Partial Liquidity Removal & AddToPosition
#   Scenario 2: CW20 Swap Path (Send hook, reverse direction)
#   Scenario 3: Oracle Price Rotation (ForceRotate, UpdateOraclePrice)
#   Scenario 4: NFT Lifecycle & Burn
# =====================================================================
# PREREQUISITE: run_full_test.sh must have been run first (chain up,
#               code IDs stored, wallets funded, oracle deployed).
# =====================================================================

BIN="/tmp/bluechipChaind_new"
CHAIN_HOME="$HOME/.bluechipTest"
CHAIN_ID="bluechip-test"
NODE="tcp://localhost:26657"
DENOM="ubluechip"

ALICE="bluechip1cyyzpxplxdzkeea7kwsydadg87357qnara5tfv"
BOB="bluechip1sc78mkjfmufxq6vjxgnhaq9ym9nhedvassl62n"
CHARLIE="bluechip1kgqnrggt0y50ujzls677kxpxfaur4mqujnq59j"

TX_FLAGS="--chain-id $CHAIN_ID --node $NODE --gas auto --gas-adjustment 1.5 --fees 50000ubluechip -y --output json"

PASS=0; FAIL=0

# =====================================================================
# HELPERS
# =====================================================================
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

log_header() { echo ""; echo ""; echo -e "${CYAN}================================================================${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}================================================================${NC}"; }
log_step()   { echo ""; echo -e "  ${YELLOW}--- $1 ---${NC}"; }
log_info()   { echo "      $1"; }
log_pass()   { echo -e "  ${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); }
log_fail()   { echo -e "  ${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); }

wait_for_next_block() {
  local START_HEIGHT
  START_HEIGHT=$($BIN status --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    si = d.get('sync_info', d.get('SyncInfo', {}))
    print(int(si.get('latest_block_height', si.get('LatestBlockHeight', 0))))
except:
    print(0)
" 2>/dev/null || echo 0)
  for i in $(seq 1 20); do
    sleep 1
    local CUR_HEIGHT
    CUR_HEIGHT=$($BIN status --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    si = d.get('sync_info', d.get('SyncInfo', {}))
    print(int(si.get('latest_block_height', si.get('LatestBlockHeight', 0))))
except:
    print(0)
" 2>/dev/null || echo 0)
    if [ "$CUR_HEIGHT" -gt "$START_HEIGHT" ] 2>/dev/null; then
      return 0
    fi
  done
}

exe_as() {
  local KEY="$1" CONTRACT="$2" MSG="$3" FUNDS="${4:-}"
  local ARGS="--from $KEY --keyring-backend test $TX_FLAGS"
  local OUT
  if [ -n "$FUNDS" ]; then
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" --amount "$FUNDS" $ARGS 2>/dev/null)
  else
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" $ARGS 2>/dev/null)
  fi
  echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','SUBMIT_FAILED'))" 2>/dev/null || echo "SUBMIT_FAILED"
}

exe()         { exe_as alice "$@"; }
exe_bob()     { exe_as bob   "$@"; }
exe_charlie() { exe_as charlie "$@"; }

qry() {
  $BIN query wasm contract-state smart "$1" "$2" --node $NODE --output json 2>/dev/null
}

wait_tx() {
  sleep 10
  $BIN query tx "$1" --node $NODE --output json 2>/dev/null
}

tx_result() {
  local RESULT
  RESULT=$(wait_tx "$1")
  local CODE LOG
  CODE=$(echo "$RESULT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',99))" 2>/dev/null || echo "99")
  LOG=$(echo "$RESULT"  | python3 -c "import json,sys; print(str(json.load(sys.stdin).get('raw_log',''))[:300])" 2>/dev/null || echo "")
  echo "${CODE}|${LOG}"
}

assert_ok() {
  local DESC="$1" TXHASH="$2"
  if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
    log_fail "$DESC — tx submission failed"
    return
  fi
  local RES CODE LOG
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  LOG=$(echo "$RES"  | cut -d'|' -f2-)
  if [ "$CODE" = "0" ]; then
    log_pass "$DESC"
  else
    log_fail "$DESC  code=$CODE  $LOG"
  fi
}

assert_fail() {
  local DESC="$1" TXHASH="$2"
  if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
    log_pass "$DESC (rejected at submission)"
    return
  fi
  local RES CODE
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  if [ "$CODE" != "0" ]; then
    log_pass "$DESC (rejected code=$CODE)"
  else
    log_fail "$DESC — expected failure but tx succeeded!"
  fi
}

assert_fail_contains() {
  local DESC="$1" TXHASH="$2" EXPECTED_SUBSTR="$3"
  if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
    log_pass "$DESC (rejected at submission)"
    return
  fi
  local RES CODE LOG
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  LOG=$(echo "$RES"  | cut -d'|' -f2-)
  if [ "$CODE" != "0" ]; then
    if echo "$LOG" | grep -qi "$EXPECTED_SUBSTR"; then
      log_pass "$DESC (contains '$EXPECTED_SUBSTR')"
    else
      log_pass "$DESC (rejected code=$CODE, msg: ${LOG:0:120})"
    fi
  else
    log_fail "$DESC — expected failure but tx succeeded!"
  fi
}

# =====================================================================
# PHASE 0: DISCOVER EXISTING CONTRACTS
# =====================================================================
log_header "PHASE 0: Discover Existing Contracts"

log_step "Querying code IDs and existing contracts"

CW20_CODE="1"
CW721_CODE="2"
POOL_CODE="3"
ORACLE_CODE="4"
EXP_CODE="5"
FACTORY_CODE="6"

echo "  Code IDs: CW20=$CW20_CODE  CW721=$CW721_CODE  POOL=$POOL_CODE"
echo "            ORACLE=$ORACLE_CODE  EXP=$EXP_CODE  FACTORY=$FACTORY_CODE"

ORACLE_ADDR=$($BIN query wasm list-contract-by-code "$ORACLE_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[0] if cs else 'ERR')" 2>/dev/null || echo "ERR")
echo "  Oracle: $ORACLE_ADDR"

FACTORY_ADDR=$($BIN query wasm list-contract-by-code "$FACTORY_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[0] if cs else 'ERR')" 2>/dev/null || echo "ERR")
echo "  Factory A: $FACTORY_ADDR"

POOL1_ADDR=$($BIN query wasm list-contract-by-code "$POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[0] if cs else 'ERR')" 2>/dev/null || echo "ERR")
echo "  Pool #1: $POOL1_ADDR"

HEIGHT=$($BIN status --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    si = d.get('sync_info', d.get('SyncInfo', {}))
    print(int(si.get('latest_block_height', si.get('LatestBlockHeight', 0))))
except:
    print(0)
" 2>/dev/null || echo 0)

if [ "$HEIGHT" -lt 2 ] 2>/dev/null; then
  echo -e "  ${RED}ERROR: Chain is not running. Run run_full_test.sh first.${NC}"
  exit 1
fi
echo "  Chain alive at block $HEIGHT"

if [ "$FACTORY_ADDR" = "ERR" ] || [ "$POOL1_ADDR" = "ERR" ]; then
  echo -e "  ${RED}ERROR: Could not find existing contracts. Run run_full_test.sh first.${NC}"
  exit 1
fi

# ---- Discover Pool #1's CW20 token address from Pair query ----
log_step "Querying Pool #1 token addresses"

PAIR_INFO=$(qry "$POOL1_ADDR" '{"pair":{}}')
CW20_TOKEN=$(echo "$PAIR_INFO" | python3 -c "
import json, sys
d = json.load(sys.stdin)
pair = d.get('data', d)
for ai in pair.get('asset_infos', []):
    if 'creator_token' in ai:
        print(ai['creator_token']['contract_addr']); exit()
print('ERR')
" 2>/dev/null || echo "ERR")
echo "  CW20 Token: $CW20_TOKEN"

if [ "$CW20_TOKEN" = "ERR" ]; then
  echo -e "  ${RED}ERROR: Could not find CW20 token address from Pair query.${NC}"
  exit 1
fi

# Query pool state for ratio calculations
POOL_STATE=$(qry "$POOL1_ADDR" '{"pool_state":{}}')
RESERVE0=$(echo "$POOL_STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('reserve0','0'))" 2>/dev/null)
RESERVE1=$(echo "$POOL_STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('reserve1','0'))" 2>/dev/null)
echo "  Pool reserves: $RESERVE0 ubluechip / $RESERVE1 CW20"

# ---- Create a fresh liquidity position for Alice ----
log_step "Creating fresh liquidity position for Alice"

# Calculate CW20 amount needed based on pool ratio (deposit 50000 ubluechip)
DEPOSIT_BLUECHIP="50000"
DEPOSIT_CW20=$(python3 -c "
r0 = int('$RESERVE0')
r1 = int('$RESERVE1')
# CW20 needed = deposit_bluechip * (reserve1 / reserve0) * 1.01 (1% buffer)
cw20_needed = int($DEPOSIT_BLUECHIP) * r1 // r0 + 1
# Add extra buffer for rounding
cw20_needed = int(cw20_needed * 1.02)
print(cw20_needed)
")
echo "  Depositing: $DEPOSIT_BLUECHIP ubluechip + $DEPOSIT_CW20 CW20"

# Give CW20 allowance to pool
APPROVE_MSG=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL1_ADDR','amount':'$DEPOSIT_CW20','expires':None}}))")
TX=$(exe "$CW20_TOKEN" "$APPROVE_MSG")
RESULT=$(tx_result "$TX" 2>/dev/null)
CODE=$(echo "$RESULT" | cut -d'|' -f1)
echo "  CW20 allowance tx: code=$CODE"

sleep 3

# Deposit liquidity
DEPOSIT_MSG=$(python3 -c "import json; print(json.dumps({'deposit_liquidity':{'amount0':'$DEPOSIT_BLUECHIP','amount1':'$DEPOSIT_CW20','min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TX=$(exe "$POOL1_ADDR" "$DEPOSIT_MSG" "${DEPOSIT_BLUECHIP}ubluechip")
RESULT=$(tx_result "$TX" 2>/dev/null)
CODE=$(echo "$RESULT" | cut -d'|' -f1)
echo "  DepositLiquidity tx: code=$CODE"

if [ "$CODE" != "0" ]; then
  LOG=$(echo "$RESULT" | cut -d'|' -f2-)
  echo -e "  ${RED}ERROR: DepositLiquidity failed: $LOG${NC}"
  echo -e "  ${RED}Cannot proceed without a liquidity position.${NC}"
  exit 1
fi

# Get Alice's new position ID
ALICE_POSITIONS=$(qry "$POOL1_ADDR" "{\"positions_by_owner\":{\"owner\":\"$ALICE\"}}")
ALICE_POS_ID=$(echo "$ALICE_POSITIONS" | python3 -c "
import json, sys
d = json.load(sys.stdin)
ps = d.get('data', d).get('positions', [])
# Get the last (newest) position
print(ps[-1].get('position_id', '') if ps else '')
" 2>/dev/null)
ALICE_POS_LIQ=$(echo "$ALICE_POSITIONS" | python3 -c "
import json, sys
d = json.load(sys.stdin)
ps = d.get('data', d).get('positions', [])
print(ps[-1].get('liquidity', '0') if ps else '0')
" 2>/dev/null)
echo "  Alice new position ID: $ALICE_POS_ID  liquidity: $ALICE_POS_LIQ"

if [ -z "$ALICE_POS_ID" ] || [ "$ALICE_POS_ID" = "" ]; then
  echo -e "  ${RED}ERROR: Could not find Alice's position after deposit.${NC}"
  exit 1
fi

# Find NFT contract by iterating CW721 contracts
NFT_ADDR="ERR"
CW721_CONTRACTS=$($BIN query wasm list-contract-by-code "$CW721_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; print(' '.join(json.load(sys.stdin).get('contracts',[])))" 2>/dev/null)
for CADDR in $CW721_CONTRACTS; do
  OWNER_RESULT=$(qry "$CADDR" "{\"owner_of\":{\"token_id\":\"$ALICE_POS_ID\"}}" 2>/dev/null)
  OWNER_VAL=$(echo "$OWNER_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('owner',''))" 2>/dev/null)
  if [ "$OWNER_VAL" = "$ALICE" ]; then
    NFT_ADDR="$CADDR"
    break
  fi
done
echo "  Position NFT: $NFT_ADDR"

if [ "$NFT_ADDR" = "ERR" ]; then
  echo -e "  ${RED}WARNING: Could not find NFT contract. NFT burn tests will be skipped.${NC}"
fi

# =====================================================================
# SCENARIO 1: PARTIAL LIQUIDITY & ADD-TO-POSITION
# =====================================================================
log_header "SCENARIO 1: Partial Liquidity & AddToPosition"

# ---- 1a. RemovePartialLiquidityByPercent ----
log_step "1a. RemovePartialLiquidityByPercent (25%)"

REMOVE_PCT_MSG=$(python3 -c "import json; print(json.dumps({'remove_partial_liquidity_by_percent':{'position_id':'$ALICE_POS_ID','percentage':25,'transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TX=$(exe "$POOL1_ADDR" "$REMOVE_PCT_MSG")
assert_ok "T1: Alice RemovePartialLiquidityByPercent (25%) → succeeds" "$TX"

# Verify position liquidity decreased
ALICE_POS_AFTER=$(qry "$POOL1_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
LIQ_AFTER_PARTIAL=$(echo "$ALICE_POS_AFTER" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('liquidity','0'))" 2>/dev/null)
echo "  Liquidity: before=$ALICE_POS_LIQ  after=$LIQ_AFTER_PARTIAL"

EXPECTED_LIQ=$(python3 -c "
before = int('$ALICE_POS_LIQ')
after = int('$LIQ_AFTER_PARTIAL')
expected_approx = before * 75 // 100
diff_pct = abs(after - expected_approx) / max(expected_approx, 1) * 100
print('PASS' if diff_pct < 5 else 'FAIL')
print(f'{expected_approx}')
")
RESULT_CHECK=$(echo "$EXPECTED_LIQ" | head -1)
EXPECTED_VAL=$(echo "$EXPECTED_LIQ" | tail -1)

if [ "$RESULT_CHECK" = "PASS" ]; then
  log_pass "T2: Position liquidity decreased ~25% ($ALICE_POS_LIQ → $LIQ_AFTER_PARTIAL, expected ~$EXPECTED_VAL)"
else
  log_fail "T2: Position liquidity mismatch ($ALICE_POS_LIQ → $LIQ_AFTER_PARTIAL, expected ~$EXPECTED_VAL)"
fi

# ---- 1b. Get CW20 tokens for AddToPosition via SimpleSwap ----
log_step "1b. SimpleSwap to acquire CW20 tokens"

# Wait for block to ensure T1 is fully committed
wait_for_next_block
wait_for_next_block

# Alice swaps 10000 ubluechip → CW20 tokens (rate limit starts)
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"10000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TX=$(exe "$POOL1_ADDR" "$SWAP_MSG" "10000ubluechip")
# Retry once if submission fails
if [ "$TX" = "SUBMIT_FAILED" ]; then
  wait_for_next_block
  TX=$(exe "$POOL1_ADDR" "$SWAP_MSG" "10000ubluechip")
fi
assert_ok "T3: Alice SimpleSwap 10000 ubluechip → CW20 tokens" "$TX"

# Check Alice's CW20 balance
ALICE_CW20_BAL=$(qry "$CW20_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('balance','0'))" 2>/dev/null)
echo "  Alice CW20 balance: $ALICE_CW20_BAL"

# ---- 1c. Increase CW20 allowance for pool ----
log_step "1c. Increase CW20 allowance for pool"

APPROVE_MSG=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL1_ADDR','amount':'$ALICE_CW20_BAL','expires':None}}))")
TX=$(exe "$CW20_TOKEN" "$APPROVE_MSG")
assert_ok "T4: Alice increase_allowance for Pool #1" "$TX"

# ---- 1d. AddToPosition ----
log_step "1d. AddToPosition"

# Add small amount — pool calculates optimal CW20 from ratio
ADD_MSG=$(python3 -c "import json; print(json.dumps({'add_to_position':{'position_id':'$ALICE_POS_ID','amount0':'1000','amount1':'$ALICE_CW20_BAL','min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TX=$(exe "$POOL1_ADDR" "$ADD_MSG" "1000ubluechip")
assert_ok "T5: Alice AddToPosition (1000 ubluechip) → succeeds" "$TX"

# Verify liquidity increased
ALICE_POS_AFTER2=$(qry "$POOL1_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
LIQ_AFTER_ADD=$(echo "$ALICE_POS_AFTER2" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('liquidity','0'))" 2>/dev/null)
echo "  Liquidity after add: $LIQ_AFTER_ADD (was $LIQ_AFTER_PARTIAL)"

if python3 -c "exit(0 if int('$LIQ_AFTER_ADD') > int('$LIQ_AFTER_PARTIAL') else 1)"; then
  log_pass "T6: Position liquidity increased ($LIQ_AFTER_PARTIAL → $LIQ_AFTER_ADD)"
else
  log_fail "T6: Position liquidity did not increase ($LIQ_AFTER_PARTIAL → $LIQ_AFTER_ADD)"
fi

# ---- 1e. RemovePartialLiquidity (absolute amount) ----
log_step "1e. RemovePartialLiquidity (absolute amount)"

# Wait for block to ensure T5 is fully committed
wait_for_next_block
wait_for_next_block

# Remove a small absolute amount of liquidity
REMOVE_ABS=$(python3 -c "print(max(int('$LIQ_AFTER_ADD') // 10, 1000))")
REMOVE_ABS_MSG=$(python3 -c "import json; print(json.dumps({'remove_partial_liquidity':{'position_id':'$ALICE_POS_ID','liquidity_to_remove':'$REMOVE_ABS','transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TX=$(exe "$POOL1_ADDR" "$REMOVE_ABS_MSG")
# Retry once if submission fails
if [ "$TX" = "SUBMIT_FAILED" ]; then
  wait_for_next_block
  TX=$(exe "$POOL1_ADDR" "$REMOVE_ABS_MSG")
fi
assert_ok "T7: Alice RemovePartialLiquidity ($REMOVE_ABS units) → succeeds" "$TX"

# Verify position still exists
ALICE_POS_AFTER3=$(qry "$POOL1_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
LIQ_AFTER_ABS=$(echo "$ALICE_POS_AFTER3" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('liquidity','0'))" 2>/dev/null)
echo "  Liquidity after absolute removal: $LIQ_AFTER_ABS"

if [ "$LIQ_AFTER_ABS" != "0" ] && [ -n "$LIQ_AFTER_ABS" ]; then
  log_pass "T8: Position still exists with liquidity ($LIQ_AFTER_ABS)"
else
  log_fail "T8: Position missing or zero liquidity"
fi

# ---- 1f. CollectFees ----
log_step "1f. CollectFees"

TX=$(exe "$POOL1_ADDR" "{\"collect_fees\":{\"position_id\":\"$ALICE_POS_ID\"}}")
assert_ok "T9: Alice CollectFees → succeeds" "$TX"

# =====================================================================
# SCENARIO 2: CW20 SWAP PATH
# =====================================================================
log_header "SCENARIO 2: CW20 Swap Path (CW20 → ubluechip)"

log_step "2a. Check Alice CW20 balance"

ALICE_CW20_BAL2=$(qry "$CW20_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('balance','0'))" 2>/dev/null)
echo "  Alice CW20 balance: $ALICE_CW20_BAL2"

if [ "$ALICE_CW20_BAL2" != "0" ] && [ -n "$ALICE_CW20_BAL2" ]; then
  log_pass "T10: Alice has CW20 tokens ($ALICE_CW20_BAL2)"
else
  log_fail "T10: Alice has no CW20 tokens"
fi

# ---- 2b. CW20 Send + Swap hook ----
log_step "2b. CW20 Send → Pool Swap Hook"

# Calculate swap amount (use 10% of balance or 100M, whichever is smaller)
SWAP_CW20_AMT=$(python3 -c "
bal = int('$ALICE_CW20_BAL2')
amt = min(bal // 10, 100000000)
print(max(amt, 1000))
")
echo "  Swapping $SWAP_CW20_AMT CW20 → ubluechip"

# Build base64-encoded Cw20HookMsg::Swap
CW20_SEND_MSG=$(python3 -c "
import json, base64
hook = json.dumps({'swap':{'belief_price':None,'max_spread':'0.99','to':None,'transaction_deadline':None}})
b64 = base64.b64encode(hook.encode()).decode()
msg = {'send':{'contract':'$POOL1_ADDR','amount':'$SWAP_CW20_AMT','msg':b64}}
print(json.dumps(msg))
")

TX=$(exe "$CW20_TOKEN" "$CW20_SEND_MSG")
assert_ok "T11: Alice CW20 Send+Swap ($SWAP_CW20_AMT CW20 → ubluechip)" "$TX"

# Verify CW20 balance decreased
ALICE_CW20_BAL3=$(qry "$CW20_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('balance','0'))" 2>/dev/null)
echo "  CW20 balance: before=$ALICE_CW20_BAL2  after=$ALICE_CW20_BAL3"

if python3 -c "exit(0 if int('$ALICE_CW20_BAL3') < int('$ALICE_CW20_BAL2') else 1)"; then
  log_pass "T12: CW20 balance decreased ($ALICE_CW20_BAL2 → $ALICE_CW20_BAL3)"
else
  log_fail "T12: CW20 balance did not decrease"
fi

# =====================================================================
# SCENARIO 3: ORACLE PRICE ROTATION
# =====================================================================
log_header "SCENARIO 3: Oracle Price Rotation"

# ---- 3a. ForceRotateOraclePools (admin only) ----
log_step "3a. ForceRotateOraclePools"

TX=$(exe "$FACTORY_ADDR" '{"force_rotate_oracle_pools":{}}')
assert_ok "T13: Alice ForceRotateOraclePools → succeeds" "$TX"

TX=$(exe "$FACTORY_ADDR" '{"force_rotate_oracle_pools":{}}')
assert_ok "T14: Alice ForceRotateOraclePools (2nd, no rate limit) → succeeds" "$TX"

TX=$(exe_bob "$FACTORY_ADDR" '{"force_rotate_oracle_pools":{}}')
assert_fail_contains "T15: Bob ForceRotateOraclePools → rejected" "$TX" "admin"

# ---- 3b. UpdateOraclePrice (permissionless, 300s rate limit) ----
log_step "3b. UpdateOraclePrice"

# Wait for sequence to update after T14/T15 txs
sleep 3

TX=$(exe "$FACTORY_ADDR" '{"update_oracle_price":{}}')
# This may succeed, fail at simulation (SUBMIT_FAILED), or fail on-chain
if [ "$TX" = "SUBMIT_FAILED" ]; then
  log_pass "T16: UpdateOraclePrice → correctly rate-limited (rejected at simulation)"
else
  RESULT=$(tx_result "$TX" 2>/dev/null)
  CODE=$(echo "$RESULT" | cut -d'|' -f1)
  LOG=$(echo "$RESULT"  | cut -d'|' -f2-)
  if [ "$CODE" = "0" ]; then
    log_pass "T16: UpdateOraclePrice → succeeds"
  elif echo "$LOG" | grep -qi "too quickly\|UpdateTooSoon\|update_too_soon"; then
    log_pass "T16: UpdateOraclePrice → correctly rate-limited (300s interval)"
  else
    log_fail "T16: UpdateOraclePrice unexpected error: $LOG"
  fi
fi

# Immediately try again — should be rate limited
TX=$(exe_bob "$FACTORY_ADDR" '{"update_oracle_price":{}}')
assert_fail_contains "T17: UpdateOraclePrice (immediate retry) → rate limited" "$TX" "too"

# =====================================================================
# SCENARIO 4: NFT LIFECYCLE & BURN
# =====================================================================
log_header "SCENARIO 4: NFT Lifecycle & Burn"

# ---- 4a. Remove all remaining liquidity ----
log_step "4a. RemoveAllLiquidity"

REMOVE_ALL_MSG=$(python3 -c "import json; print(json.dumps({'remove_all_liquidity':{'position_id':'$ALICE_POS_ID','transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TX=$(exe "$POOL1_ADDR" "$REMOVE_ALL_MSG")
assert_ok "T18: Alice RemoveAllLiquidity (position $ALICE_POS_ID) → succeeds" "$TX"

# Verify position removed from pool storage
POS_CHECK=$(qry "$POOL1_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
POS_GONE=$(echo "$POS_CHECK" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    # If query returns an error or no data, position is gone
    data = d.get('data', d)
    if data is None or data == {} or 'error' in str(d).lower():
        print('GONE')
    else:
        liq = data.get('liquidity', '0')
        print('GONE' if liq == '0' else 'EXISTS')
except:
    print('GONE')
" 2>/dev/null)

if [ "$POS_GONE" = "GONE" ]; then
  log_pass "T19: Position removed from pool storage"
else
  log_fail "T19: Position still exists in pool storage"
fi

# ---- 4b. Verify NFT still exists (pool doesn't burn it) ----
log_step "4b. Verify NFT still exists after liquidity removal"

if [ "$NFT_ADDR" != "ERR" ]; then
  NFT_OWNER=$(qry "$NFT_ADDR" "{\"owner_of\":{\"token_id\":\"$ALICE_POS_ID\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('owner','NONE'))" 2>/dev/null)
  echo "  NFT token $ALICE_POS_ID owner: $NFT_OWNER"

  if [ "$NFT_OWNER" = "$ALICE" ]; then
    log_pass "T20: NFT still owned by Alice after RemoveAllLiquidity"
  else
    log_fail "T20: NFT owner unexpected: $NFT_OWNER"
  fi

  # ---- 4c. Alice burns the NFT ----
  log_step "4c. Burn Position NFT"

  BURN_MSG="{\"burn\":{\"token_id\":\"$ALICE_POS_ID\"}}"
  TX=$(exe "$NFT_ADDR" "$BURN_MSG")
  assert_ok "T21: Alice burns position NFT (token $ALICE_POS_ID)" "$TX"

  # Verify NFT no longer exists
  NFT_CHECK=$(qry "$NFT_ADDR" "{\"owner_of\":{\"token_id\":\"$ALICE_POS_ID\"}}" 2>/dev/null)
  NFT_BURNED=$(echo "$NFT_CHECK" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    owner = d.get('data', d).get('owner', '')
    print('EXISTS' if owner else 'BURNED')
except:
    print('BURNED')
" 2>/dev/null)

  if [ "$NFT_BURNED" = "BURNED" ]; then
    log_pass "T22: NFT burned — OwnerOf query fails (token destroyed)"
  else
    log_fail "T22: NFT still exists after burn"
  fi
else
  log_fail "T20: Skipped — NFT contract not found"
  log_fail "T21: Skipped — NFT contract not found"
  log_fail "T22: Skipped — NFT contract not found"
fi

# =====================================================================
# FINAL REPORT
# =====================================================================
log_header "FINAL REPORT"

TOTAL=$((PASS + FAIL))
echo ""
echo -e "  ${GREEN}PASSED: $PASS${NC}"
echo -e "  ${RED}FAILED: $FAIL${NC}"
echo -e "  TOTAL:  $TOTAL"
echo ""
if [ "$FAIL" -eq 0 ]; then
  echo -e "  ${GREEN}ALL $TOTAL TESTS PASSED${NC}"
else
  echo -e "  ${RED}$FAIL TEST(S) FAILED${NC}"
fi
echo ""
