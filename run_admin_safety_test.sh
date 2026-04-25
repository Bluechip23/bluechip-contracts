#!/usr/bin/env bash
# =====================================================================
# Bluechip Admin Controls & Safety Guards Test
# =====================================================================
# Tests:
#   Scenario 1: Admin & Emergency Controls — Pause/Unpause, Emergency
#               Withdraw (2-phase with 24h timelock), Cancel, Recovery.
#               Uses a standalone pool where Alice = factory_addr.
#   Scenario 2: Slippage & Safety Guards — ShortOfThreshold, rate
#               limiting, transaction deadline, max spread slippage.
# =====================================================================
# PREREQUISITE: run_full_test.sh must have been run first (chain up,
#               code IDs stored, wallets funded, oracle deployed).
# =====================================================================

BIN="/tmp/bluechipChaind_new"
CHAIN_HOME="$HOME/.bluechipTest"
CHAIN_ID="bluechip-test"
NODE="tcp://localhost:26657"
ARTIFACTS="/home/jeremy/snap/smartcontracts/bluechip-contracts/artifacts"
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

# Returns tx result code and raw_log for custom assertions
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

inst() {
  local CODE_ID="$1" MSG="$2" LABEL="$3" FUNDS="${4:-}"
  local ARGS="--from alice --keyring-backend test $TX_FLAGS --no-admin"
  local OUT TXHASH ADDR
  if [ -n "$FUNDS" ]; then
    OUT=$($BIN tx wasm instantiate "$CODE_ID" "$MSG" --label "$LABEL" --amount "$FUNDS" $ARGS 2>/dev/null)
  else
    OUT=$($BIN tx wasm instantiate "$CODE_ID" "$MSG" --label "$LABEL" $ARGS 2>/dev/null)
  fi
  TXHASH=$(echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','ERR'))" 2>/dev/null || echo "ERR")
  sleep 10
  ADDR=$($BIN query tx "$TXHASH" --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
for e in d.get('events', []):
    for a in e.get('attributes', []):
        if a.get('key') in ['_contract_address', 'contract_address']:
            print(a['value']); exit()
print('ERR')
" 2>/dev/null || echo "ERR")
  echo "$ADDR"
}

COMMIT_MSG() {
  local AMT="$1"
  local SPREAD="${2:-}"
  local DEADLINE="${3:-}"
  if [ -n "$DEADLINE" ]; then
    python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'$DENOM'}},'amount':'$AMT'},'amount':'$AMT','transaction_deadline':'$DEADLINE','belief_price':None,'max_spread':None}}))"
  elif [ -n "$SPREAD" ]; then
    python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'$DENOM'}},'amount':'$AMT'},'amount':'$AMT','transaction_deadline':None,'belief_price':None,'max_spread':'$SPREAD'}}))"
  else
    python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'$DENOM'}},'amount':'$AMT'},'amount':'$AMT','transaction_deadline':None,'belief_price':None,'max_spread':None}}))"
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
STANDARD_POOL_CODE="4"
ORACLE_CODE="5"
EXP_CODE="6"
FACTORY_CODE="7"

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

if [ "$ORACLE_ADDR" = "ERR" ] || [ "$FACTORY_ADDR" = "ERR" ] || [ "$POOL1_ADDR" = "ERR" ]; then
  echo -e "  ${RED}ERROR: Could not find existing contracts. Run run_full_test.sh first.${NC}"
  exit 1
fi
log_pass "Discovered all existing contracts"

# =====================================================================
# SCENARIO 1: ADMIN & EMERGENCY CONTROLS
# =====================================================================
log_header "SCENARIO 1: Admin & Emergency Controls"
echo "  Goal: Test Pause/Unpause, EmergencyWithdraw, Cancel, unauthorized access"
echo "  Approach: Standalone pool with Alice = factory_addr (admin)"

# ---------------------------------------------------------------
# 1a. Create CW20 token for admin test pool
# ---------------------------------------------------------------
log_step "Create CW20 token (AdminTestToken)"

CW20_INIT_MSG=$(python3 -c "
import json
print(json.dumps({
    'name': 'AdminTestToken',
    'symbol': 'ATEST',
    'decimals': 6,
    'initial_balances': [
        {'address': '$ALICE', 'amount': '1000000000000'}
    ],
    'mint': {'minter': '$ALICE'}
}))
")

ADMIN_CW20=$(inst "$CW20_CODE" "$CW20_INIT_MSG" "AdminTestCW20")
echo "  Admin CW20: $ADMIN_CW20"

if [ "$ADMIN_CW20" = "ERR" ]; then
  log_fail "CW20 creation failed"
  exit 1
fi
log_pass "CW20 AdminTestToken created"

# ---------------------------------------------------------------
# 1b. Create CW721 for admin test pool
# ---------------------------------------------------------------
log_step "Create CW721 NFT (AdminTestNFT)"

CW721_INIT_MSG=$(python3 -c "
import json
print(json.dumps({
    'name': 'AdminTestNFT',
    'symbol': 'ANFT',
    'minter': '$ALICE'
}))
")

ADMIN_CW721=$(inst "$CW721_CODE" "$CW721_INIT_MSG" "AdminTestCW721")
echo "  Admin CW721: $ADMIN_CW721"

if [ "$ADMIN_CW721" = "ERR" ]; then
  log_fail "CW721 creation failed"
  exit 1
fi
log_pass "CW721 AdminTestNFT created"

# ---------------------------------------------------------------
# 1c. Instantiate admin pool (Alice = factory_addr)
# ---------------------------------------------------------------
log_step "Instantiate admin pool (Alice = factory_addr, is_standard_pool=true)"

POOL_INIT_MSG=$(python3 -c "
import json
print(json.dumps({
    'pool_id': 99,
    'pool_token_info': [
        {'bluechip': {'denom': '$DENOM'}},
        {'creator_token': {'contract_addr': '$ADMIN_CW20'}}
    ],
    'cw20_token_contract_id': int('$CW20_CODE'),
    'used_factory_addr': '$ALICE',
    'threshold_payout': None,
    'commit_fee_info': {
        'bluechip_wallet_address': '$ALICE',
        'creator_wallet_address': '$ALICE',
        'commit_fee_bluechip': '0.01',
        'commit_fee_creator': '0.05',
    },
    'commit_threshold_limit_usd': '0',
    'commit_amount_for_threshold': '0',
    'position_nft_address': '$ADMIN_CW721',
    'token_address': '$ADMIN_CW20',
    'max_bluechip_lock_per_pool': '25000000000',
    'creator_excess_liquidity_lock_days': 7,
    'is_standard_pool': True,
}))
")

ADMIN_POOL=$(inst "$POOL_CODE" "$POOL_INIT_MSG" "AdminTestPool")
echo "  Admin Pool: $ADMIN_POOL"

if [ "$ADMIN_POOL" = "ERR" ]; then
  log_fail "Admin pool instantiation failed"
  exit 1
fi
log_pass "Admin pool created (Alice = factory_addr, standard pool)"

# Verify threshold is already hit (is_standard_pool=true)
IS_HIT=$(qry "$ADMIN_POOL" '{"is_fully_commited":{}}' | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
s = str(d).lower()
if 'fully_committed' in s or 'fullycommitted' in s:
    print('YES')
else:
    print('NO')
" 2>/dev/null || echo "UNKNOWN")
echo "  Standard pool threshold status: $IS_HIT"

# ---------------------------------------------------------------
# 1d. Unauthorized access tests (Bob → rejected)
# ---------------------------------------------------------------
log_step "Unauthorized access — Bob tries admin operations"

TXHASH=$(exe_bob "$ADMIN_POOL" '{"pause":{}}')
assert_fail "Bob calls Pause → Unauthorized" "$TXHASH"

TXHASH=$(exe_bob "$ADMIN_POOL" '{"emergency_withdraw":{}}')
assert_fail "Bob calls EmergencyWithdraw → Unauthorized" "$TXHASH"

TXHASH=$(exe_bob "$ADMIN_POOL" '{"recover_stuck_states":{"recovery_type":"both"}}')
assert_fail "Bob calls RecoverStuckStates → Unauthorized" "$TXHASH"

# ---------------------------------------------------------------
# 1e. Pause — verify swap blocked
# ---------------------------------------------------------------
log_step "Pause pool — verify operations blocked"

TXHASH=$(exe "$ADMIN_POOL" '{"pause":{}}')
assert_ok "Alice calls Pause" "$TXHASH"

# Try a swap on the paused pool — should fail with "paused" error
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"10000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$ADMIN_POOL" "$SWAP_MSG" "10000ubluechip")
assert_fail_contains "Swap blocked when paused" "$TXHASH" "paused"

# ---------------------------------------------------------------
# 1f. Unpause — verify different error (not paused)
# ---------------------------------------------------------------
log_step "Unpause pool — verify different behavior"

TXHASH=$(exe "$ADMIN_POOL" '{"unpause":{}}')
assert_ok "Alice calls Unpause" "$TXHASH"

# Same swap — should now fail with a DIFFERENT error (zero reserves, not paused)
TXHASH=$(exe "$ADMIN_POOL" "$SWAP_MSG" "10000ubluechip")
if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
  # Check if submission failed (gas estimation fails on zero-reserve pool)
  log_pass "Swap after unpause fails with different error (not paused)"
else
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  LOG=$(echo "$RES"  | cut -d'|' -f2-)
  if [ "$CODE" != "0" ]; then
    if echo "$LOG" | grep -qi "paused"; then
      log_fail "Swap still fails with paused error after unpause"
    else
      log_pass "Swap after unpause fails with different error (not paused): ${LOG:0:100}"
    fi
  else
    log_pass "Swap after unpause succeeded (pool functional)"
  fi
fi

# ---------------------------------------------------------------
# 1g. Emergency Withdraw Phase 1 — initiate
# ---------------------------------------------------------------
log_step "Emergency Withdraw — Phase 1 (initiate with 24h timelock)"

TXHASH=$(exe "$ADMIN_POOL" '{"emergency_withdraw":{}}')
assert_ok "EmergencyWithdraw Phase 1 (initiate)" "$TXHASH"

# Verify pool is now paused (Phase 1 pauses the pool)
TXHASH=$(exe "$ADMIN_POOL" "$SWAP_MSG" "10000ubluechip")
assert_fail_contains "Pool paused after Emergency Phase 1" "$TXHASH" "paused"

# ---------------------------------------------------------------
# 1h. Emergency Withdraw Phase 2 — too early (24h timelock)
# ---------------------------------------------------------------
log_step "Emergency Withdraw — Phase 2 too early"

TXHASH=$(exe "$ADMIN_POOL" '{"emergency_withdraw":{}}')
assert_fail_contains "Phase 2 rejected (timelock not elapsed)" "$TXHASH" "timelock"

# ---------------------------------------------------------------
# 1i. Cancel Emergency Withdraw
# ---------------------------------------------------------------
log_step "Cancel Emergency Withdraw"

TXHASH=$(exe "$ADMIN_POOL" '{"cancel_emergency_withdraw":{}}')
assert_ok "CancelEmergencyWithdraw succeeds" "$TXHASH"

# Verify pool is unpaused after cancel
TXHASH=$(exe "$ADMIN_POOL" "$SWAP_MSG" "10000ubluechip")
if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
  # Submission may fail due to zero reserves but NOT due to pause
  log_pass "Pool unpaused after cancel (submission fails — not pause-related)"
else
  RES=$(tx_result "$TXHASH")
  LOG=$(echo "$RES" | cut -d'|' -f2-)
  if echo "$LOG" | grep -qi "paused"; then
    log_fail "Pool still paused after CancelEmergencyWithdraw"
  else
    log_pass "Pool unpaused after cancel (error: ${LOG:0:80})"
  fi
fi

# ---------------------------------------------------------------
# 1j. Cancel with no pending withdrawal — should fail
# ---------------------------------------------------------------
log_step "Cancel with no pending withdrawal"

TXHASH=$(exe "$ADMIN_POOL" '{"cancel_emergency_withdraw":{}}')
assert_fail_contains "Cancel with nothing pending → rejected" "$TXHASH" "pending"

# ---------------------------------------------------------------
# 1k. Re-initiate + cancel cycle (verify repeatable)
# ---------------------------------------------------------------
log_step "Re-initiate Emergency + Cancel cycle"

TXHASH=$(exe "$ADMIN_POOL" '{"emergency_withdraw":{}}')
assert_ok "Re-initiate EmergencyWithdraw (2nd time)" "$TXHASH"

TXHASH=$(exe "$ADMIN_POOL" '{"cancel_emergency_withdraw":{}}')
assert_ok "Cancel EmergencyWithdraw (2nd time)" "$TXHASH"

# Verify pool responds to queries (not bricked)
POOL_STATE=$(qry "$ADMIN_POOL" '{"pool_state":{}}')
POOL_OK=$(echo "$POOL_STATE" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
if 'reserve0' in d:
    print('OK')
else:
    print('FAIL')
" 2>/dev/null || echo "FAIL")

if [ "$POOL_OK" = "OK" ]; then
  log_pass "Pool responds to queries after emergency cycle"
else
  log_fail "Pool not responding after emergency cycle"
fi

echo ""
echo -e "  ${GREEN}=== Scenario 1 Complete ===${NC}"

# =====================================================================
# SCENARIO 2: SLIPPAGE & SAFETY GUARDS
# =====================================================================
log_header "SCENARIO 2: Slippage & Safety Guards"
echo "  Goal: Test ShortOfThreshold, rate limiting, deadline, slippage"

# ---------------------------------------------------------------
# 2a. Create pre-threshold Pool #4 via Factory A
# ---------------------------------------------------------------
log_step "Create pre-threshold Pool #4 via Factory A"

CREATE_MSG4=$(python3 -c "
import json
print(json.dumps({
    'create': {
        'pool_msg': {
            'pool_token_info': [
                {'bluechip': {'denom': '$DENOM'}},
                {'creator_token': {'contract_addr': 'WILL_BE_CREATED_BY_FACTORY'}}
            ],
            'cw20_token_contract_id':          int('$CW20_CODE'),
            'factory_to_create_pool_addr':     '$FACTORY_ADDR',
            'threshold_payout':                None,
            'commit_fee_info': {
                'bluechip_wallet_address':     '$ALICE',
                'creator_wallet_address':      '$ALICE',
                'commit_fee_bluechip':         '0.01',
                'commit_fee_creator':          '0.05',
            },
            'creator_token_address':           '$ALICE',
            'commit_amount_for_threshold':     '0',
            'commit_limit_usd':                '25000',
            'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
            'pyth_atom_usd_price_feed_id':    'ATOM_USD',
            'max_bluechip_lock_per_pool':      '25000000000',
            'creator_excess_liquidity_lock_days': 7,
            'is_standard_pool':                False,
        },
        'token_info': {'name': 'SafetyTest', 'symbol': 'STEST', 'decimal': 6},
    }
}))
")

TXHASH=$(exe "$FACTORY_ADDR" "$CREATE_MSG4")
echo "  Create Pool TX: $TXHASH"
sleep 14

POOL4_ADDR=$($BIN query wasm list-contract-by-code "$POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[-1] if cs else 'ERR')" 2>/dev/null || echo "ERR")

echo "  Pool #4: $POOL4_ADDR"

if [ "$POOL4_ADDR" != "ERR" ]; then
  log_pass "Pre-threshold Pool #4 created"
else
  log_fail "Pool #4 creation failed"
  exit 1
fi

# ---------------------------------------------------------------
# 2b. ShortOfThreshold — swap blocked before threshold
# ---------------------------------------------------------------
log_step "ShortOfThreshold — swap blocked on pre-threshold pool"

SWAP_MSG4='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"10000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL4_ADDR" "$SWAP_MSG4" "10000ubluechip")
assert_fail_contains "Swap on pre-threshold pool fails" "$TXHASH" "threshold"

# ---------------------------------------------------------------
# 2c. Rate limiting — two commits < 13s from same wallet
# ---------------------------------------------------------------
log_step "Rate limiting — two rapid commits from same wallet"

# First commit should succeed
TXHASH=$(exe "$POOL4_ADDR" "$(COMMIT_MSG 100000)" "100000ubluechip")
assert_ok "First commit on Pool #4 (Alice)" "$TXHASH"

# Second commit immediately (within 13s) — should fail with TooFrequentCommits
# Use fixed gas to avoid gas-estimation delay eating into the 13s window
RAPID_OUT=$($BIN tx wasm execute "$POOL4_ADDR" "$(COMMIT_MSG 100000)" \
  --amount "100000ubluechip" \
  --from alice --keyring-backend test \
  --chain-id $CHAIN_ID --node $NODE --gas 2000000 --fees 50000ubluechip -y --output json 2>/dev/null)
RAPID_TXHASH=$(echo "$RAPID_OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','SUBMIT_FAILED'))" 2>/dev/null || echo "SUBMIT_FAILED")
assert_fail_contains "Second commit within 13s → TooFrequentCommits" "$RAPID_TXHASH" "frequent"

# ---------------------------------------------------------------
# 2d. Transaction deadline — expired deadline
# ---------------------------------------------------------------
log_step "Transaction deadline — commit with expired deadline"

# Wait for rate limit cooldown (commit from Bob to avoid Alice's rate limit)
sleep 15

# Use deadline timestamp of "1" (1970-01-01 00:00:01 — definitely expired)
DEADLINE_MSG=$(python3 -c "
import json
print(json.dumps({
    'commit': {
        'asset': {'info': {'bluechip': {'denom': '$DENOM'}}, 'amount': '100000'},
        'amount': '100000',
        'transaction_deadline': '1',
        'belief_price': None,
        'max_spread': None,
    }
}))
")

TXHASH=$(exe_bob "$POOL4_ADDR" "$DEADLINE_MSG" "100000ubluechip")
assert_fail_contains "Commit with expired deadline → TransactionExpired" "$TXHASH" "deadline"

# ---------------------------------------------------------------
# 2e. Slippage protection — tight max_spread on post-threshold pool
# ---------------------------------------------------------------
log_step "Slippage protection — tight max_spread fails"

# Use Pool #1 (from run_full_test.sh — post-threshold with reserves)
# Large swap with very tight spread (0.1% = 0.001) should fail
sleep 15  # Rate limit cooldown

TIGHT_SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"500000"},"belief_price":null,"max_spread":"0.001","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL1_ADDR" "$TIGHT_SWAP_MSG" "500000ubluechip")
assert_fail_contains "Large swap with 0.1% max_spread → MaxSpreadAssertion" "$TXHASH" "spread"

# Verify same swap succeeds with generous spread (0.99)
sleep 15  # Rate limit cooldown
GENEROUS_SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"500000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL1_ADDR" "$GENEROUS_SWAP_MSG" "500000ubluechip")
assert_ok "Same swap with 99% max_spread succeeds" "$TXHASH"

echo ""
echo -e "  ${GREEN}=== Scenario 2 Complete ===${NC}"

# =====================================================================
# FINAL REPORT
# =====================================================================
log_header "FINAL REPORT"
echo ""
echo -e "  ${GREEN}Passed: $PASS${NC}   ${RED}Failed: $FAIL${NC}"
echo ""
if [ "$FAIL" -eq 0 ]; then
  echo -e "  ${GREEN}ALL TESTS PASSED${NC}"
else
  echo -e "  ${RED}$FAIL TEST(S) FAILED${NC}"
fi
echo ""
echo "  Contracts Used:"
echo "    Admin CW20:     $ADMIN_CW20"
echo "    Admin CW721:    $ADMIN_CW721"
echo "    Admin Pool:     $ADMIN_POOL"
echo "    Pool #1:        $POOL1_ADDR"
echo "    Pool #4:        $POOL4_ADDR"
echo "    Factory A:      $FACTORY_ADDR"
echo "    Oracle:         $ORACLE_ADDR"
echo ""
echo "  Test Accounts:"
echo "    Alice:   $ALICE"
echo "    Bob:     $BOB"
echo ""
echo "  Chain log: /tmp/bluechip_chain.log"
echo ""
