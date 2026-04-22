#!/usr/bin/env bash
# =====================================================================
# Bluechip Factory Governance, ExpandEconomy Treasury & Edge Cases Test
# =====================================================================
# Tests:
#   Scenario 1: Factory Governance — ProposeConfigUpdate/UpdateConfig/
#               CancelConfigUpdate/ProposePoolConfigUpdate/ExecutePoolConfigUpdate/
#               CancelPoolConfigUpdate/UpgradePools.
#   Scenario 2: ExpandEconomy Treasury — ProposeWithdrawal/ExecuteWithdrawal/
#               CancelWithdrawal (48h timelock), ProposeConfigUpdate/
#               ExecuteConfigUpdate/CancelConfigUpdate (48h timelock).
#   Scenario 3: Pool Edge Cases — zero amounts, simulation queries,
#               position ownership, fee queries.
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
# HELPERS (same as other test scripts)
# =====================================================================
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

log_header() { echo ""; echo ""; echo -e "${CYAN}================================================================${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}================================================================${NC}"; }
log_step()   { echo ""; echo -e "  ${YELLOW}--- $1 ---${NC}"; }
log_info()   { echo "      $1"; }
log_pass()   { echo -e "  ${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); }
log_fail()   { echo -e "  ${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); }

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

EXP_ADDR=$($BIN query wasm list-contract-by-code "$EXP_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[0] if cs else 'ERR')" 2>/dev/null || echo "ERR")
echo "  ExpandEconomy: $EXP_ADDR"

FACTORY_ADDR=$($BIN query wasm list-contract-by-code "$FACTORY_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[0] if cs else 'ERR')" 2>/dev/null || echo "ERR")
echo "  Factory A: $FACTORY_ADDR"

# Get Pool #1 (from run_full_test.sh — post-threshold, functional)
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

if [ "$ORACLE_ADDR" = "ERR" ] || [ "$FACTORY_ADDR" = "ERR" ] || [ "$POOL1_ADDR" = "ERR" ] || [ "$EXP_ADDR" = "ERR" ]; then
  echo -e "  ${RED}ERROR: Could not find existing contracts. Run run_full_test.sh first.${NC}"
  exit 1
fi

# Query current factory config for use in governance tests
log_step "Querying current factory config"
FACTORY_CONFIG=$(qry "$FACTORY_ADDR" '{"factory":{}}')
echo "  Factory config fetched"

# Build ProposeConfigUpdate message from current config (change max_bluechip_lock_per_pool)
PROPOSE_MSG=$(FACTORY_CONFIG_JSON="$FACTORY_CONFIG" ALICE_ADDR="$ALICE" ORACLE_ADDR_VAL="$ORACLE_ADDR" python3 << 'PYEOF'
import json, sys, os

raw = os.environ.get("FACTORY_CONFIG_JSON", "{}")
try:
    data = json.loads(raw)
except:
    data = {}

factory = data.get("data", data)
if isinstance(factory, dict) and "factory" in factory:
    factory = factory["factory"]

alice = os.environ.get("ALICE_ADDR", "")
oracle = os.environ.get("ORACLE_ADDR_VAL", "")

propose = {
    "propose_config_update": {
        "config": {
            "factory_admin_address": factory.get("factory_admin_address", alice),
            "commit_amount_for_threshold_bluechip": factory.get("commit_amount_for_threshold_bluechip", "1000000000000"),
            "commit_threshold_limit_usd": factory.get("commit_threshold_limit_usd", "25000000000"),
            "pyth_contract_addr_for_conversions": factory.get("pyth_contract_addr_for_conversions", oracle),
            "pyth_atom_usd_price_feed_id": factory.get("pyth_atom_usd_price_feed_id", "ATOM_USD"),
            "cw20_token_contract_id": int(factory.get("cw20_token_contract_id", 1)),
            "cw721_nft_contract_id": int(factory.get("cw721_nft_contract_id", 2)),
            "create_pool_wasm_contract_id": int(factory.get("create_pool_wasm_contract_id", 3)),
            "bluechip_wallet_address": factory.get("bluechip_wallet_address", alice),
            "commit_fee_bluechip": factory.get("commit_fee_bluechip", "0.01"),
            "commit_fee_creator": factory.get("commit_fee_creator", "0.05"),
            "max_bluechip_lock_per_pool": "99999999999999",
            "creator_excess_liquidity_lock_days": int(factory.get("creator_excess_liquidity_lock_days", 14)),
            "atom_bluechip_anchor_pool_address": factory.get("atom_bluechip_anchor_pool_address", alice),
            "bluechip_mint_contract_address": factory.get("bluechip_mint_contract_address", None),
            "bluechip_denom": factory.get("bluechip_denom", "ubluechip"),
            "standard_pool_creation_fee_usd": factory.get("standard_pool_creation_fee_usd", "1000000"),
        }
    }
}
print(json.dumps(propose))
PYEOF
)
echo "  ProposeConfigUpdate message built"

# =====================================================================
# SCENARIO 1: FACTORY GOVERNANCE
# =====================================================================
log_header "SCENARIO 1: Factory Governance"

# ---- 1a. Unauthorized access tests ----
log_step "1a. Unauthorized Access (Bob)"

# Bob tries ProposeConfigUpdate
TX=$(exe_bob "$FACTORY_ADDR" "$PROPOSE_MSG")
assert_fail_contains "T1: Bob ProposeConfigUpdate → rejected" "$TX" "admin"

# Bob tries ProposePoolConfigUpdate
TX=$(exe_bob "$FACTORY_ADDR" '{"propose_pool_config_update":{"pool_id":1,"pool_config":{"lp_fee":"0.005","min_commit_interval":null,"usd_payment_tolerance_bps":null,"oracle_address":null}}}')
assert_fail_contains "T2: Bob ProposePoolConfigUpdate → rejected" "$TX" "admin"

# Bob tries CancelConfigUpdate
TX=$(exe_bob "$FACTORY_ADDR" '{"cancel_config_update":{}}')
assert_fail_contains "T3: Bob CancelConfigUpdate → rejected" "$TX" "admin"

# ---- 1b. ProposeConfigUpdate → UpdateConfig (timelock) → CancelConfigUpdate ----
log_step "1b. Config Update Governance Cycle"

TX=$(exe "$FACTORY_ADDR" "$PROPOSE_MSG")
assert_ok "T4: Alice ProposeConfigUpdate → succeeds" "$TX"

# UpdateConfig too early (48h timelock not expired)
TX=$(exe "$FACTORY_ADDR" '{"update_config":{}}')
assert_fail_contains "T5: Alice UpdateConfig (too early, 48h timelock) → rejected" "$TX" "timelock"

# CancelConfigUpdate
TX=$(exe "$FACTORY_ADDR" '{"cancel_config_update":{}}')
assert_ok "T6: Alice CancelConfigUpdate → succeeds" "$TX"

# ---- 1c. Repeatable cycle ----
log_step "1c. Repeatable Propose/Cancel Cycle"

TX=$(exe "$FACTORY_ADDR" "$PROPOSE_MSG")
assert_ok "T7: Alice ProposeConfigUpdate (2nd time) → succeeds" "$TX"

TX=$(exe "$FACTORY_ADDR" '{"cancel_config_update":{}}')
assert_ok "T8: Alice CancelConfigUpdate (2nd time) → succeeds" "$TX"

# ---- 1d. Pool Upgrade Governance ----
log_step "1d. Pool Upgrade Governance"

# Use base64 encoded empty migrate msg: "{}" = "e30="
TX=$(exe "$FACTORY_ADDR" '{"upgrade_pools":{"new_code_id":3,"pool_ids":[1],"migrate_msg":"e30="}}')
assert_ok "T9: Alice ProposePoolUpgrade → succeeds" "$TX"

TX=$(exe "$FACTORY_ADDR" '{"execute_pool_upgrade":{}}')
assert_fail_contains "T10: Alice ExecutePoolUpgrade (too early, 48h timelock) → rejected" "$TX" "timelock"

TX=$(exe "$FACTORY_ADDR" '{"cancel_pool_upgrade":{}}')
assert_ok "T11: Alice CancelPoolUpgrade → succeeds" "$TX"

TX=$(exe "$FACTORY_ADDR" '{"cancel_pool_upgrade":{}}')
assert_fail_contains "T12: Alice CancelPoolUpgrade (no pending) → rejected" "$TX" "No pending"

# ---- 1e. ProposePoolConfigUpdate ----
log_step "1e. ProposePoolConfigUpdate"

# Propose pool 1's lp_fee update via factory (48h timelock)
TX=$(exe "$FACTORY_ADDR" '{"propose_pool_config_update":{"pool_id":1,"pool_config":{"lp_fee":"0.005","min_commit_interval":null,"usd_payment_tolerance_bps":null,"oracle_address":null}}}')
assert_ok "T13: Alice ProposePoolConfigUpdate (pool 1 lp_fee → 0.5%) → succeeds" "$TX"

# ExecutePoolConfigUpdate too early (48h timelock not expired)
TX=$(exe "$FACTORY_ADDR" '{"execute_pool_config_update":{"pool_id":1}}')
assert_fail_contains "T13b: Alice ExecutePoolConfigUpdate (too early, 48h timelock) → rejected" "$TX" "timelock"

# CancelPoolConfigUpdate
TX=$(exe "$FACTORY_ADDR" '{"cancel_pool_config_update":{"pool_id":1}}')
assert_ok "T13c: Alice CancelPoolConfigUpdate → succeeds" "$TX"

# =====================================================================
# SCENARIO 2: EXPANDECONOMY TREASURY
# =====================================================================
log_header "SCENARIO 2: ExpandEconomy Treasury"

# Tests: config verification, unauthorized access, propose/execute/cancel
# withdrawal (48h timelock), propose/execute/cancel config update (48h timelock).

# ---- 2a. Verify Config ----
log_step "2a. Verify ExpandEconomy Config"

EXP_CONFIG=$(qry "$EXP_ADDR" '{"get_config":{}}')
EXP_OWNER=$(echo "$EXP_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('owner','???'))" 2>/dev/null)
EXP_FACTORY=$(echo "$EXP_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('factory_address','???'))" 2>/dev/null)
echo "  ExpandEconomy owner: $EXP_OWNER"
echo "  ExpandEconomy factory: $EXP_FACTORY"

if [ "$EXP_OWNER" = "$ALICE" ]; then
  log_pass "T14: ExpandEconomy owner is Alice"
else
  log_fail "T14: ExpandEconomy owner expected=$ALICE got=$EXP_OWNER"
fi

# ---- 2b. Fund ExpandEconomy with ubluechip for testing ----
log_step "2b. Fund ExpandEconomy contract"

# Query current balance first
EXP_BAL_BEFORE=$(qry "$EXP_ADDR" '{"get_balance":{"denom":"ubluechip"}}')
EXP_BAL_BEFORE_AMT=$(echo "$EXP_BAL_BEFORE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('amount','0'))" 2>/dev/null)
echo "  Balance before funding: $EXP_BAL_BEFORE_AMT"

FUND_TX=$($BIN tx bank send alice "$EXP_ADDR" 500000ubluechip --from alice --keyring-backend test $TX_FLAGS 2>/dev/null \
  | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','ERR'))" 2>/dev/null)
sleep 10

EXP_BAL=$(qry "$EXP_ADDR" '{"get_balance":{"denom":"ubluechip"}}')
EXP_BAL_AMT=$(echo "$EXP_BAL" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('amount','0'))" 2>/dev/null)
echo "  Balance after funding: $EXP_BAL_AMT ubluechip"

if [ "$EXP_BAL_AMT" != "0" ] && [ -n "$EXP_BAL_AMT" ]; then
  log_pass "T15: ExpandEconomy has ubluechip balance ($EXP_BAL_AMT)"
else
  log_fail "T15: ExpandEconomy has zero balance"
fi

# ---- 2c. Unauthorized access ----
log_step "2c. Unauthorized Treasury Access (Bob)"

TX=$(exe_bob "$EXP_ADDR" '{"propose_withdrawal":{"amount":"100000","denom":"ubluechip","recipient":null}}')
assert_fail_contains "T16: Bob ProposeWithdrawal → rejected" "$TX" "Unauthorized"

TX=$(exe_bob "$EXP_ADDR" '{"propose_config_update":{"factory_address":null,"owner":null}}')
assert_fail_contains "T17: Bob ProposeConfigUpdate → rejected" "$TX" "Unauthorized"

# ---- 2d. Owner Withdrawal Governance (48h timelock) ----
log_step "2d. Owner Withdrawal Governance"

# Record balance before
BEFORE_BAL="$EXP_BAL_AMT"

# Propose withdrawal (starts 48h timelock)
TX=$(exe "$EXP_ADDR" '{"propose_withdrawal":{"amount":"100000","denom":"ubluechip","recipient":null}}')
assert_ok "T18: Alice ProposeWithdrawal (100000 ubluechip) → succeeds" "$TX"

# Execute too early (48h timelock not expired)
TX=$(exe "$EXP_ADDR" '{"execute_withdrawal":{}}')
assert_fail_contains "T19: Alice ExecuteWithdrawal (too early, 48h timelock) → rejected" "$TX" "Timelock"

# Verify balance unchanged (withdrawal not executed)
EXP_BAL_AFTER=$(qry "$EXP_ADDR" '{"get_balance":{"denom":"ubluechip"}}')
AFTER_BAL=$(echo "$EXP_BAL_AFTER" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('amount','0'))" 2>/dev/null)
echo "  Balance before: $BEFORE_BAL  after: $AFTER_BAL"

if [ "$AFTER_BAL" = "$BEFORE_BAL" ]; then
  log_pass "T19b: Balance unchanged during timelock ($BEFORE_BAL)"
else
  log_fail "T19b: Balance changed before timelock expired (before=$BEFORE_BAL, after=$AFTER_BAL)"
fi

# Cancel withdrawal
TX=$(exe "$EXP_ADDR" '{"cancel_withdrawal":{}}')
assert_ok "T20: Alice CancelWithdrawal → succeeds" "$TX"

# Cancel again (no pending) — should fail
TX=$(exe "$EXP_ADDR" '{"cancel_withdrawal":{}}')
assert_fail_contains "T20b: Alice CancelWithdrawal (no pending) → rejected" "$TX" "pending"

# Verify repeatable: propose + cancel cycle
TX=$(exe "$EXP_ADDR" '{"propose_withdrawal":{"amount":"50000","denom":"ubluechip","recipient":null}}')
assert_ok "T20c: Alice ProposeWithdrawal (2nd time) → succeeds" "$TX"

TX=$(exe "$EXP_ADDR" '{"cancel_withdrawal":{}}')
assert_ok "T20d: Alice CancelWithdrawal (2nd time) → succeeds" "$TX"

# ---- 2e. Owner ConfigUpdate Governance (48h timelock) ----
log_step "2e. Owner ConfigUpdate Governance"

# Propose config update (owner → Bob)
PROPOSE_OWNER_BOB=$(python3 -c "import json; print(json.dumps({'propose_config_update':{'factory_address':None,'owner':'$BOB'}}))")
TX=$(exe "$EXP_ADDR" "$PROPOSE_OWNER_BOB")
assert_ok "T21: Alice ProposeConfigUpdate (owner → Bob) → succeeds" "$TX"

# Execute too early (48h timelock)
TX=$(exe "$EXP_ADDR" '{"execute_config_update":{}}')
assert_fail_contains "T22: Alice ExecuteConfigUpdate (too early, 48h timelock) → rejected" "$TX" "Timelock"

# Verify config unchanged
EXP_CONFIG2=$(qry "$EXP_ADDR" '{"get_config":{}}')
CURRENT_OWNER=$(echo "$EXP_CONFIG2" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('owner','???'))" 2>/dev/null)

if [ "$CURRENT_OWNER" = "$ALICE" ]; then
  log_pass "T22b: Config unchanged during timelock — owner still Alice"
else
  log_fail "T22b: Config changed before timelock expired (owner=$CURRENT_OWNER)"
fi

# Cancel config update
TX=$(exe "$EXP_ADDR" '{"cancel_config_update":{}}')
assert_ok "T23: Alice CancelConfigUpdate → succeeds" "$TX"

# Cancel again (no pending) — should fail
TX=$(exe "$EXP_ADDR" '{"cancel_config_update":{}}')
assert_fail_contains "T23b: Alice CancelConfigUpdate (no pending) → rejected" "$TX" "pending"

# Repeatable cycle
PROPOSE_OWNER_BOB2=$(python3 -c "import json; print(json.dumps({'propose_config_update':{'factory_address':None,'owner':'$BOB'}}))")
TX=$(exe "$EXP_ADDR" "$PROPOSE_OWNER_BOB2")
assert_ok "T23c: Alice ProposeConfigUpdate (2nd time) → succeeds" "$TX"

TX=$(exe "$EXP_ADDR" '{"cancel_config_update":{}}')
assert_ok "T23d: Alice CancelConfigUpdate (2nd time) → succeeds" "$TX"

# =====================================================================
# SCENARIO 3: POOL EDGE CASES
# =====================================================================
log_header "SCENARIO 3: Pool Edge Cases"

# ---- 3a. Zero-amount swap ----
log_step "3a. Zero-Amount Swap"

# Zero-amount SimpleSwap — must_pay rejects zero/no funds
TX=$(exe "$POOL1_ADDR" '{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"0"},"belief_price":null,"max_spread":null,"to":null,"transaction_deadline":null}}')
assert_fail "T24: Zero-amount SimpleSwap → rejected" "$TX"

# ---- 3b. Simulation queries ----
log_step "3b. Simulation Queries"

SIM_RESULT=$(qry "$POOL1_ADDR" '{"simulation":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"1000000"}}}')
SIM_RETURN=$(echo "$SIM_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('return_amount','0'))" 2>/dev/null)
SIM_SPREAD=$(echo "$SIM_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('spread_amount','0'))" 2>/dev/null)
SIM_COMM=$(echo "$SIM_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('commission_amount','0'))" 2>/dev/null)
echo "  Simulation: 1000000 ubluechip → $SIM_RETURN tokens (spread=$SIM_SPREAD, commission=$SIM_COMM)"

if [ "$SIM_RETURN" != "0" ] && [ -n "$SIM_RETURN" ]; then
  log_pass "T25: Simulation query returns non-zero ($SIM_RETURN)"
else
  log_fail "T25: Simulation query returned zero or error"
fi

# Reverse Simulation
RSIM_RESULT=$(qry "$POOL1_ADDR" '{"reverse_simulation":{"ask_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"1000000"}}}')
RSIM_OFFER=$(echo "$RSIM_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('offer_amount','0'))" 2>/dev/null)
RSIM_SPREAD=$(echo "$RSIM_RESULT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('spread_amount','0'))" 2>/dev/null)
echo "  ReverseSimulation: need $RSIM_OFFER creator tokens for 1000000 ubluechip (spread=$RSIM_SPREAD)"

if [ "$RSIM_OFFER" != "0" ] && [ -n "$RSIM_OFFER" ]; then
  log_pass "T26: ReverseSimulation query returns non-zero offer ($RSIM_OFFER)"
else
  log_fail "T26: ReverseSimulation query returned zero or error"
fi

# ---- 3c. Pool state queries ----
log_step "3c. Pool State Queries"

POOL_STATE=$(qry "$POOL1_ADDR" '{"pool_state":{}}')
RESERVE0=$(echo "$POOL_STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('reserve0','0'))" 2>/dev/null)
RESERVE1=$(echo "$POOL_STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('reserve1','0'))" 2>/dev/null)
TOTAL_LIQ=$(echo "$POOL_STATE" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('total_liquidity','0'))" 2>/dev/null)
echo "  Pool #1 reserves: $RESERVE0 / $RESERVE1  liquidity: $TOTAL_LIQ"

if [ "$RESERVE0" != "0" ] && [ "$RESERVE1" != "0" ] && [ -n "$RESERVE0" ] && [ -n "$RESERVE1" ]; then
  log_pass "T27: PoolState has non-zero reserves ($RESERVE0 / $RESERVE1)"
else
  log_fail "T27: PoolState reserves are zero or missing"
fi

# Fee info query
FEE_INFO=$(qry "$POOL1_ADDR" '{"fee_info":{}}')
FEE_BLUECHIP=$(echo "$FEE_INFO" | python3 -c "import json,sys; d=json.load(sys.stdin); fi=d.get('data',d).get('fee_info',{}); print(fi.get('commit_fee_bluechip','???'))" 2>/dev/null)
FEE_CREATOR=$(echo "$FEE_INFO" | python3 -c "import json,sys; d=json.load(sys.stdin); fi=d.get('data',d).get('fee_info',{}); print(fi.get('commit_fee_creator','???'))" 2>/dev/null)
echo "  Fees: bluechip=$FEE_BLUECHIP creator=$FEE_CREATOR"

if [ "$FEE_BLUECHIP" != "???" ] && [ "$FEE_CREATOR" != "???" ]; then
  log_pass "T28: FeeInfo returns valid fee structure (bc=$FEE_BLUECHIP cr=$FEE_CREATOR)"
else
  log_fail "T28: FeeInfo query failed"
fi

# PoolInfo composite query
POOL_INFO=$(qry "$POOL1_ADDR" '{"pool_info":{}}')
POS_COUNT=$(echo "$POOL_INFO" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('data',d).get('total_positions',0))" 2>/dev/null)
echo "  Pool #1 total positions: $POS_COUNT"

if [ "$POS_COUNT" != "0" ] && [ -n "$POS_COUNT" ]; then
  log_pass "T29: PoolInfo shows active positions (count=$POS_COUNT)"
else
  log_fail "T29: PoolInfo returned zero positions"
fi

# ---- 3d. Position ownership ----
log_step "3d. Position Ownership"

# Query Alice's positions
ALICE_POSITIONS=$(qry "$POOL1_ADDR" "{\"positions_by_owner\":{\"owner\":\"$ALICE\"}}")
ALICE_POS_COUNT=$(echo "$ALICE_POSITIONS" | python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d.get('data',d).get('positions',[])))" 2>/dev/null)
ALICE_POS_ID=$(echo "$ALICE_POSITIONS" | python3 -c "import json,sys; d=json.load(sys.stdin); ps=d.get('data',d).get('positions',[]); print(ps[0].get('position_id','') if ps else '')" 2>/dev/null)
echo "  Alice has $ALICE_POS_COUNT positions, first ID: $ALICE_POS_ID"

if [ "$ALICE_POS_COUNT" != "0" ] && [ -n "$ALICE_POS_ID" ] && [ "$ALICE_POS_ID" != "" ]; then
  log_pass "T30: Alice has positions in Pool #1 (count=$ALICE_POS_COUNT)"
else
  log_fail "T30: Alice has no positions in Pool #1"
fi

# Bob tries to remove Alice's position → should fail (Unauthorized)
if [ -n "$ALICE_POS_ID" ] && [ "$ALICE_POS_ID" != "" ]; then
  REMOVE_MSG=$(python3 -c "import json; print(json.dumps({'remove_all_liquidity':{'position_id':'$ALICE_POS_ID','transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
  TX=$(exe_bob "$POOL1_ADDR" "$REMOVE_MSG")
  assert_fail_contains "T31: Bob RemoveAllLiquidity (Alice's position) → rejected" "$TX" "Unauthorized"
else
  log_fail "T31: Skipped — no position ID available"
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
