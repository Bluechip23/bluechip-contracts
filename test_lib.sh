# test_lib.sh — shared harness for run_*.sh integration scripts.
#
# Source from a script:
#     source "$(dirname "$0")/test_lib.sh"
#
# Override any default by exporting the var BEFORE sourcing, e.g.
#     CHAIN_ID=foo bash run_my_test.sh
#
# Centralizes the things that drift independently of the test scenario:
#   * chain config (BIN, CHAIN_ID, CHAIN_HOME, NODE, DENOM, KR)
#   * USD precision constants (must match contract MIN_COMMIT_USD_*)
#   * keyring address resolution (no more hardcoded `bluechip1cyy...`)
#   * code-id discovery from the running chain (no more `POOL_CODE=3`)
#   * tx submit / query / balance helpers that handle the
#     `WARNING:(ast) sonic only supports go1.17~1.23` stderr noise
#     and survive multi-line pretty-printed JSON.
# ─────────────────────────────────────────────────────────────────────

# ─── Defaults (override-able) ───────────────────────────────────────
: "${BIN:=/tmp/bluechipChaind_new}"
: "${CHAIN_ID:=bluechip-test}"
: "${CHAIN_HOME:=$HOME/.bluechipTest}"
: "${NODE:=tcp://localhost:26657}"
: "${KR:=test}"
: "${DENOM:=ubluechip}"
: "${GAS_FLAGS:=--gas auto --gas-adjustment 1.5 --fees 50000ubluechip}"

# 6-decimal raw USD. Keep these in lockstep with creator-pool/src/commit.rs:
#   MIN_COMMIT_USD_PRE_THRESHOLD  =  5_000_000  ($5)
#   MIN_COMMIT_USD_POST_THRESHOLD =  1_000_000  ($1)
# Factory normalizes Pyth prices to 6 decimals (expo -8 => /100), so for
# 1 ubluechip = $0.01 the raw Pyth price is 1e12.
: "${ORACLE_PRICE:=1000000000000}"
: "${THRESHOLD_USD_RAW:=25000000000}"   # $25,000

TX_FLAGS="--chain-id $CHAIN_ID --node $NODE --keyring-backend $KR --home $CHAIN_HOME $GAS_FLAGS -y --output json"

# ─── Logging ────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
PASS=0; FAIL=0
log_header() { echo; echo -e "${CYAN}================================================================${NC}"; echo -e "${CYAN}  $1${NC}"; echo -e "${CYAN}================================================================${NC}"; }
log_step()   { echo; echo -e "  ${YELLOW}--- $1 ---${NC}"; }
log_info()   { echo "      $1"; }
log_pass()   { echo -e "  ${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); }
log_fail()   { echo -e "  ${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); }

# Print a summary at exit. Sourcing scripts should not need to re-implement.
_summary() {
  local rc=$?
  echo
  echo "================================================================"
  if [ "$FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}Result: PASS=$PASS  FAIL=$FAIL${NC}"
  else
    echo -e "  ${RED}Result: PASS=$PASS  FAIL=$FAIL${NC}"
  fi
  echo "================================================================"
  exit $rc
}
trap _summary EXIT

# ─── JSON helpers (resilient to stderr warnings + multi-line output) ─

# Read stdin, find first '{', try to json.loads from there, print field.
# Usage: echo "$RAW" | json_get txhash
json_get() {
  local field="$1"
  python3 -c "
import json, sys
raw = sys.stdin.read()
i = raw.find('{')
if i < 0:
    sys.exit(0)
try:
    print(json.loads(raw[i:]).get('$field',''))
except Exception:
    pass
" 2>/dev/null
}

# Same as json_get but expects nested data.<field> (typical query response).
json_data_get() {
  local field="$1"
  python3 -c "
import json, sys
raw = sys.stdin.read()
i = raw.find('{')
if i < 0:
    sys.exit(0)
try:
    d = json.loads(raw[i:]).get('data', {})
    if isinstance(d, dict):
        # Allow shallow dotted lookup: 'foo.bar'
        cur = d
        for part in '$field'.split('.'):
            if isinstance(cur, dict):
                cur = cur.get(part, '')
            else:
                cur = ''; break
        print(cur)
except Exception:
    pass
" 2>/dev/null
}

# ─── Key + address resolution ───────────────────────────────────────
addr_for() {
  $BIN keys show "$1" -a --keyring-backend $KR --home "$CHAIN_HOME" 2>/dev/null
}

# Ensure a key exists in the local keyring; create with a new mnemonic if not.
ensure_key() {
  local name="$1"
  $BIN keys show "$name" --keyring-backend $KR --home "$CHAIN_HOME" >/dev/null 2>&1 \
    || $BIN keys add "$name" --keyring-backend $KR --home "$CHAIN_HOME" >/dev/null 2>&1 || true
}

# ─── Tx + query primitives ──────────────────────────────────────────

# Submit a wasm-execute tx, return txhash on stdout (or empty on submit fail).
# Args: from_key contract msg [funds]
exe_as() {
  local key="$1" contract="$2" msg="$3" funds="${4:-}"
  local out
  if [ -n "$funds" ]; then
    out=$($BIN tx wasm execute "$contract" "$msg" --amount "$funds" --from "$key" $TX_FLAGS 2>/dev/null)
  else
    out=$($BIN tx wasm execute "$contract" "$msg" --from "$key" $TX_FLAGS 2>/dev/null)
  fi
  echo "$out" | json_get txhash
}
exe()     { exe_as alice "$@"; }
exe_bob() { exe_as bob   "$@"; }

# Smart query — print full JSON response (so caller can json_data_get).
qry() {
  $BIN query wasm contract-state smart "$1" "$2" --node $NODE --output json 2>/dev/null
}

# Bank balance of (addr, denom). Returns "0" if missing.
get_bal() {
  $BIN query bank balances "$1" --node $NODE --output json 2>/dev/null \
    | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: print('0'); sys.exit()
try:
    d = json.loads(raw[i:])
    for c in d.get('balances', []):
        if c['denom'] == '$2':
            print(c['amount']); sys.exit()
    print('0')
except Exception:
    print('0')
" 2>/dev/null || echo "0"
}

# Wait for a tx to be indexed; print the full query JSON to stdout
# (or empty + return 1 on timeout).
# Args: txhash [retries=8 sleep=4]
wait_for_tx() {
  local hash="$1" retries="${2:-8}" delay="${3:-4}"
  local out
  for _ in $(seq 1 "$retries"); do
    sleep "$delay"
    out=$($BIN query tx "$hash" --node $NODE --output json 2>/dev/null)
    if [ -n "$out" ] && echo "$out" | head -c 1 | grep -q "{"; then
      echo "$out"; return 0
    fi
  done
  return 1
}

# Convenience: assert a tx hash succeeded on chain (code = 0). Logs PASS/FAIL.
# Args: label txhash
assert_tx_ok() {
  local label="$1" hash="$2"
  if [ -z "$hash" ] || [ "$hash" = "SUBMIT_FAILED" ]; then
    log_fail "$label — submit failed (no txhash)"
    return 1
  fi
  local raw code
  raw=$(wait_for_tx "$hash" 8 3)
  code=$(echo "$raw" | json_get code)
  if [ "$code" = "0" ]; then
    log_pass "$label"
  else
    local raw_log
    raw_log=$(echo "$raw" | json_get raw_log)
    log_fail "$label (code=$code): $(echo "$raw_log" | head -c 160)"
  fi
}

# Inverse of assert_tx_ok: PASS when the tx is rejected (submit-time or on-chain).
# Useful for asserting access-control, validation-error tests.
assert_tx_fail() {
  local label="$1" hash="$2"
  if [ -z "$hash" ] || [ "$hash" = "SUBMIT_FAILED" ]; then
    log_pass "$label (rejected at submission)"
    return 0
  fi
  local raw code
  raw=$(wait_for_tx "$hash" 8 3)
  code=$(echo "$raw" | json_get code)
  if [ "$code" != "0" ]; then
    log_pass "$label (rejected code=$code)"
  else
    log_fail "$label — expected failure but tx succeeded"
  fi
}

# Upload a wasm; return the assigned code_id (or empty on error).
# Args: path/to/file.wasm  [from_key=alice]
store_wasm() {
  local file="$1" from="${2:-alice}"
  local out hash
  out=$($BIN tx wasm store "$file" --from "$from" $TX_FLAGS 2>/dev/null)
  hash=$(echo "$out" | json_get txhash)
  [ -z "$hash" ] && return 1
  local raw
  raw=$(wait_for_tx "$hash" 8 3) || return 1
  echo "$raw" | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    d = json.loads(raw[i:])
    for e in d.get('events', []):
        for a in e.get('attributes', []):
            if a.get('key') == 'code_id':
                print(a['value']); sys.exit()
except Exception:
    pass
" 2>/dev/null
}

# Instantiate a code_id with a JSON msg; return the new contract address.
# Args: code_id  msg  label  [funds]  [from_key=alice]  [admin_flag=--no-admin]
inst() {
  local code_id="$1" msg="$2" label="$3" funds="${4:-}" from="${5:-alice}" admin="${6:---no-admin}"
  local out hash
  if [ -n "$funds" ]; then
    out=$($BIN tx wasm instantiate "$code_id" "$msg" --label "$label" --amount "$funds" --from "$from" $admin $TX_FLAGS 2>/dev/null)
  else
    out=$($BIN tx wasm instantiate "$code_id" "$msg" --label "$label" --from "$from" $admin $TX_FLAGS 2>/dev/null)
  fi
  hash=$(echo "$out" | json_get txhash)
  [ -z "$hash" ] && return 1
  local raw
  raw=$(wait_for_tx "$hash" 8 3) || return 1
  echo "$raw" | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    d = json.loads(raw[i:])
    for e in d.get('events', []):
        for a in e.get('attributes', []):
            if a.get('key') in ('_contract_address','contract_address'):
                print(a['value']); sys.exit()
except Exception:
    pass
" 2>/dev/null
}

# ─── Code-id discovery ─────────────────────────────────────────────
#
# Probes every code_id on the chain by listing its contracts and
# querying with $probe_msg; returns the first code_id whose response
# contains $needle. Empty on miss.
#
# Use this instead of hardcoding `POOL_CODE=3`.
find_code_id_by_query() {
  local probe="$1" needle="$2"
  local codes
  codes=$($BIN query wasm list-code --node $NODE --output json --limit 200 2>/dev/null \
    | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    print(' '.join(c['code_id'] for c in json.loads(raw[i:]).get('code_infos',[])))
except Exception:
    pass
")
  for c in $codes; do
    local addrs
    addrs=$($BIN query wasm list-contract-by-code "$c" --node $NODE --output json 2>/dev/null \
      | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    print(' '.join(json.loads(raw[i:]).get('contracts',[])))
except Exception:
    pass
")
    for a in $addrs; do
      local resp
      resp=$($BIN query wasm contract-state smart "$a" "$probe" --node $NODE --output json 2>/dev/null)
      if echo "$resp" | grep -q "$needle"; then
        echo "$c"; return 0
      fi
    done
  done
  return 1
}

# Same idea but returns the FIRST contract address (not the code-id).
find_contract_by_query() {
  local probe="$1" needle="$2"
  local cid
  cid=$(find_code_id_by_query "$probe" "$needle") || return 1
  $BIN query wasm list-contract-by-code "$cid" --node $NODE --output json 2>/dev/null \
    | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    cs = json.loads(raw[i:]).get('contracts',[])
    if cs: print(cs[0])
except Exception:
    pass
"
}

# Convenience probes for the bluechip stack:
discover_factory_addr()  { find_contract_by_query '{"factory":{}}'        'standard_pool_wasm_contract_id'; }
discover_factory_code()  { find_code_id_by_query  '{"factory":{}}'        'standard_pool_wasm_contract_id'; }
discover_cw20_code()     { find_code_id_by_query  '{"token_info":{}}'     '"symbol"'; }
discover_creator_pool_code() {
  # creator-pool exposes is_fully_commited; standard-pool does not.
  find_code_id_by_query '{"is_fully_commited":{}}' 'data'
}
discover_standard_pool_code() {
  # Read from the factory's config — single source of truth.
  local f
  f=$(discover_factory_addr) || return 1
  qry "$f" '{"factory":{}}' | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: sys.exit()
try:
    print(json.loads(raw[i:])['data']['factory']['standard_pool_wasm_contract_id'])
except Exception:
    pass
"
}

# ─── Misc ──────────────────────────────────────────────────────────
chain_height() {
  $BIN status --node $NODE 2>/dev/null \
    | python3 -c "
import json, sys
raw = sys.stdin.read(); i = raw.find('{')
if i < 0: print('0'); sys.exit()
try:
    si = json.loads(raw[i:]).get('sync_info',{})
    print(si.get('latest_block_height','0'))
except Exception:
    print('0')
" 2>/dev/null || echo 0
}
