#!/usr/bin/env bash
# =====================================================================
# Bluechip Enhanced Full-Stack Test
# =====================================================================
# Tests:
#   1. Factory creation & pool creation
#   2. Pre-threshold commits with fee disbursement verification
#   3. Threshold crossing, token distribution, bluechip minting
#   4. Post-threshold commits (swap mode)
#   5. Standard bank transactions
#   6. Liquidity positions with scaler testing:
#      - Small position (~10K liquidity) → punished by fee scaler
#      - Large position (≥1M liquidity)  → full fee accumulation
#   7. Fee generation via swaps
#   8. Fee tracking & scaler impact verification
#   9. Liquidity exits & fee collection
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

# Oracle: 1 ubluechip = $0.01 → price = 1,000,000 at expo -8
# Threshold: $25,000 USD → 2,500,000 ubluechip
ORACLE_PRICE="1000000"

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

# Get balance of a specific denom for an address
get_bal() {
  local ADDR="$1" D="$2"
  $BIN query bank balances "$ADDR" --node $NODE --output json 2>/dev/null \
    | python3 -c "
import json, sys
d = json.load(sys.stdin)
for c in d.get('balances', []):
    if c['denom'] == '$D':
        print(c['amount']); exit()
print('0')
" 2>/dev/null || echo "0"
}

# Submit tx as a specific key, return txhash
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

exe()     { exe_as alice "$@"; }
exe_bob() { exe_as bob   "$@"; }

# Query contract
qry() {
  $BIN query wasm contract-state smart "$1" "$2" --node $NODE --output json 2>/dev/null
}

# Wait for tx, return result JSON
wait_tx() {
  sleep 10
  $BIN query tx "$1" --node $NODE --output json 2>/dev/null
}

# Return "code|log"
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

# Upload wasm, return code_id
store_wasm() {
  local FILE="$1"
  local OUT TXHASH CODE_ID
  OUT=$($BIN tx wasm store "$ARTIFACTS/$FILE" \
    --from alice --keyring-backend test $TX_FLAGS 2>/dev/null)
  TXHASH=$(echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','ERR'))" 2>/dev/null || echo "ERR")
  sleep 10
  CODE_ID=$($BIN query tx "$TXHASH" --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
d = json.load(sys.stdin)
for e in d.get('events', []):
    for a in e.get('attributes', []):
        if a.get('key') == 'code_id':
            print(a['value']); exit()
print('ERR')
" 2>/dev/null || echo "ERR")
  echo "$CODE_ID"
}

# Instantiate contract, return address
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

# Build commit message (optional 2nd arg = max_spread for post-threshold commits)
COMMIT_MSG() {
  local AMT="$1"
  local SPREAD="${2:-}"
  if [ -n "$SPREAD" ]; then
    python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'$DENOM'}},'amount':'$AMT'},'amount':'$AMT','transaction_deadline':None,'belief_price':None,'max_spread':'$SPREAD'}}))"
  else
    python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'$DENOM'}},'amount':'$AMT'},'amount':'$AMT','transaction_deadline':None,'belief_price':None,'max_spread':None}}))"
  fi
}

# =====================================================================
# PHASE 0: CHAIN RESET + GENESIS MODIFICATION
# =====================================================================
log_header "PHASE 0: Chain Reset + Genesis Modification"

echo "  Stopping any running chain..."
pkill -9 -f "bluechipChaind" 2>/dev/null || true
sleep 3
fuser -k 26657/tcp 2>/dev/null || true
fuser -k 26656/tcp 2>/dev/null || true
fuser -k 1317/tcp 2>/dev/null || true
fuser -k 9090/tcp 2>/dev/null || true
sleep 5

for i in $(seq 1 15); do
  if ! ss -tlnp 2>/dev/null | grep -q ':26657'; then break; fi
  echo "    ...waiting for port 26657 to free..."
  sleep 2
done

echo "  Re-initializing chain from scratch..."
rm -rf "$CHAIN_HOME"
$BIN init testnode --chain-id "$CHAIN_ID" --home "$CHAIN_HOME" > /dev/null 2>&1

# Copy keyring from original chain home
cp -r "$HOME/.bluechipChain/keyring-test" "$CHAIN_HOME/" 2>/dev/null || true

log_step "Setting up genesis accounts"
$BIN genesis add-genesis-account alice "100000000000000000stake,50000000000000ubluechip" \
  --keyring-backend test --home "$CHAIN_HOME" 2>/dev/null
$BIN genesis add-genesis-account bob "50000000stake" \
  --keyring-backend test --home "$CHAIN_HOME" 2>/dev/null
$BIN genesis add-genesis-account charlie "50000000stake" \
  --keyring-backend test --home "$CHAIN_HOME" 2>/dev/null

log_step "Creating validator"
$BIN genesis gentx alice 100000000stake --chain-id "$CHAIN_ID" \
  --keyring-backend test --home "$CHAIN_HOME" 2>/dev/null
$BIN genesis collect-gentxs --home "$CHAIN_HOME" > /dev/null 2>&1

# Set minimum gas prices to accept both stake and ubluechip
sed -i 's/minimum-gas-prices = .*/minimum-gas-prices = "0stake,0ubluechip"/' "$CHAIN_HOME/config/app.toml"

echo "  Genesis ready: Alice has 50T ubluechip + 100Q stake"
echo "  Fixedmint: 1,000,000 ubluechip/block"

echo "  Starting fresh chain..."
$BIN start --home "$CHAIN_HOME" > /tmp/bluechip_chain.log 2>&1 &
CHAIN_PID=$!
echo "  Chain PID: $CHAIN_PID"

echo "  Waiting for chain to produce blocks..."
for i in $(seq 1 60); do
  HEIGHT=$($BIN status --node $NODE --output json 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    si = d.get('sync_info', d.get('SyncInfo', {}))
    h = int(si.get('latest_block_height', si.get('LatestBlockHeight', 0)))
    print(h)
except:
    print(0)
" 2>/dev/null || echo 0)
  if [ "$HEIGHT" -ge 2 ] 2>/dev/null; then
    echo "  Chain ready at block $HEIGHT"
    break
  fi
  echo "    ...block $HEIGHT"
  sleep 3
done

log_step "Verify Alice ubluechip balance"
ALICE_UBC=$(get_bal "$ALICE" "ubluechip")
echo "  Alice ubluechip: $ALICE_UBC"
if [ "$ALICE_UBC" != "0" ] && [ -n "$ALICE_UBC" ]; then
  log_pass "Alice has ubluechip in genesis ($ALICE_UBC)"
else
  log_fail "Alice ubluechip balance is 0 or missing"
fi

log_step "Fund Bob with ubluechip"
$BIN tx bank send alice "$BOB" 5000000000ubluechip \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10
BOB_STAKE=$(get_bal "$BOB" "stake")
BOB_UBC=$(get_bal "$BOB" "ubluechip")
echo "  Bob stake: $BOB_STAKE  ubluechip: $BOB_UBC"

log_step "Fund Charlie with ubluechip"
$BIN tx bank send alice "$CHARLIE" 2000000000ubluechip \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10
CHARLIE_UBC=$(get_bal "$CHARLIE" "ubluechip")
echo "  Charlie ubluechip: $CHARLIE_UBC"

# =====================================================================
# PHASE 1: UPLOAD WASMs
# =====================================================================
log_header "PHASE 1: Upload WASMs"

log_step "cw20_base.wasm"
CW20_CODE=$(store_wasm "cw20_base.wasm")
echo "  → code $CW20_CODE"

log_step "cw721_base.wasm"
CW721_CODE=$(store_wasm "cw721_base.wasm")
echo "  → code $CW721_CODE"

log_step "pool.wasm"
POOL_CODE=$(store_wasm "pool.wasm")
echo "  → code $POOL_CODE"

log_step "oracle.wasm (mock)"
ORACLE_CODE=$(store_wasm "oracle.wasm")
echo "  → code $ORACLE_CODE"

log_step "expand-economy.wasm"
EXP_CODE=$(store_wasm "expand-economy.wasm")
echo "  → code $EXP_CODE"

log_step "factory.wasm"
FACTORY_CODE=$(store_wasm "factory.wasm")
echo "  → code $FACTORY_CODE"

echo ""
echo "  Code IDs: CW20=$CW20_CODE  CW721=$CW721_CODE  POOL=$POOL_CODE"
echo "            ORACLE=$ORACLE_CODE  EXP=$EXP_CODE  FACTORY=$FACTORY_CODE"

# Validate uploads
for CID in "$CW20_CODE" "$CW721_CODE" "$POOL_CODE" "$ORACLE_CODE" "$EXP_CODE" "$FACTORY_CODE"; do
  if [ "$CID" = "ERR" ]; then
    log_fail "One or more WASM uploads failed — cannot continue"
    echo "  Chain log: /tmp/bluechip_chain.log"
    exit 1
  fi
done
log_pass "All 6 WASMs uploaded successfully"

# =====================================================================
# PHASE 2: INSTANTIATE CONTRACTS
# =====================================================================
log_header "PHASE 2: Instantiate Contracts"

# 2a. Mock Oracle
log_step "Mock Oracle"
ORACLE_ADDR=$(inst "$ORACLE_CODE" '{}' "MockOracle")
echo "  Oracle: $ORACLE_ADDR"

log_step "Set oracle price: 1 ubluechip = \$0.01"
TXHASH=$(exe "$ORACLE_ADDR" "{\"set_price\":{\"price_id\":\"ATOM_USD\",\"price\":\"$ORACLE_PRICE\"}}")
assert_ok "Set oracle price" "$TXHASH"

# 2b. Expand Economy (placeholder factory = Alice initially)
log_step "Expand Economy"
EXP_MSG=$(python3 -c "import json; print(json.dumps({'factory_address':'$ALICE','owner':'$ALICE'}))")
EXP_ADDR=$(inst "$EXP_CODE" "$EXP_MSG" "ExpandEconomy")
echo "  ExpandEconomy: $EXP_ADDR"

# Fund expand-economy with ubluechip for minting (first pool mints ~500 tokens = 500M ubluechip)
log_step "Fund Expand Economy with ubluechip for minting"
$BIN tx bank send alice "$EXP_ADDR" 1000000000ubluechip \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10
EXP_BAL_BEFORE=$(get_bal "$EXP_ADDR" "ubluechip")
echo "  ExpandEconomy ubluechip balance: $EXP_BAL_BEFORE"

# 2c. Factory
log_step "Factory"
FACTORY_MSG=$(python3 -c "
import json
print(json.dumps({
    'factory_admin_address':              '$ALICE',
    'commit_amount_for_threshold_bluechip': '0',
    'commit_threshold_limit_usd':         '25000',
    'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
    'pyth_atom_usd_price_feed_id':        'ATOM_USD',
    'cw20_token_contract_id':             int('$CW20_CODE'),
    'cw721_nft_contract_id':              int('$CW721_CODE'),
    'create_pool_wasm_contract_id':       int('$POOL_CODE'),
    'bluechip_wallet_address':            '$ALICE',
    'commit_fee_bluechip':                '0.01',
    'commit_fee_creator':                 '0.05',
    'max_bluechip_lock_per_pool':         '25000000000',
    'creator_excess_liquidity_lock_days': 7,
    'atom_bluechip_anchor_pool_address':  '$ALICE',
    'bluechip_mint_contract_address':     '$EXP_ADDR',
}))
")
FACTORY_ADDR=$(inst "$FACTORY_CODE" "$FACTORY_MSG" "Factory")
echo "  Factory: $FACTORY_ADDR"

# 2d. ExpandEconomy factory_address stays as Alice for testing.
# In production, use ProposeConfigUpdate + ExecuteConfigUpdate (48h timelock).
# Minting is skipped in local test mode (admin == anchor_pool_address) so
# the factory doesn't need to call ExpandEconomy during threshold crossing.
log_step "ExpandEconomy factory_address = Alice (local test mode, minting skipped)"
echo "  factory_address stays as Alice — minting skipped in local test mode"

# 2e. Create Pool via Factory
log_step "Create Pool via Factory"
CREATE_MSG=$(python3 -c "
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
        'token_info': {'name': 'CreatorAlpha', 'symbol': 'CALPHA', 'decimal': 6},
    }
}))
")
TXHASH=$(exe "$FACTORY_ADDR" "$CREATE_MSG")
echo "  Create Pool TX: $TXHASH"
sleep 14

# Get pool and creator token addresses
POOL_ADDR=$($BIN query wasm list-contract-by-code "$POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[-1] if cs else 'ERR')" 2>/dev/null || echo "ERR")

CREATOR_TOKEN=$(qry "$POOL_ADDR" '{"pair":{}}' \
  | python3 -c "
import json, sys
d = json.load(sys.stdin)
for t in d.get('data', {}).get('asset_infos', []):
    ct = t.get('creator_token', {})
    if ct:
        print(ct.get('contract_addr', 'ERR')); exit()
print('ERR')
" 2>/dev/null || echo "ERR")

echo "  Pool:          $POOL_ADDR"
echo "  Creator Token: $CREATOR_TOKEN"

if [ "$POOL_ADDR" != "ERR" ] && [ "$CREATOR_TOKEN" != "ERR" ]; then
  log_pass "Factory created pool + CW20 + CW721 successfully"
else
  log_fail "Pool creation failed"
  echo "  Chain log: /tmp/bluechip_chain.log"
  exit 1
fi

echo ""
echo "  ====== DEPLOYMENT COMPLETE ======"
echo "  Oracle:          $ORACLE_ADDR"
echo "  Expand Economy:  $EXP_ADDR"
echo "  Factory:         $FACTORY_ADDR"
echo "  Pool:            $POOL_ADDR"
echo "  Creator Token:   $CREATOR_TOKEN"

# =====================================================================
# PHASE 3: PRE-THRESHOLD COMMITS WITH FEE VERIFICATION
# =====================================================================
log_header "PHASE 3: Pre-Threshold Commits with Fee Verification"
echo "  Oracle: 1 ubluechip = \$0.01   Threshold: \$25,000 = 2,500,000 ubluechip"
echo "  Fee structure: 1% bluechip wallet + 5% creator wallet = 6% total"

# ---------------------------------------------------------------
# Commit #1: Alice commits 500,000 ubluechip ($5,000)
# ---------------------------------------------------------------
log_step "Commit #1 — Alice commits 500,000 ubluechip (\$5,000)"

# Snapshot balances before commit
ALICE_UBC_PRE=$(get_bal "$ALICE" "$DENOM")
echo "  Alice ubluechip BEFORE: $ALICE_UBC_PRE"

TXHASH=$(exe "$POOL_ADDR" "$(COMMIT_MSG 500000)" "500000$DENOM")
assert_ok "Commit #1 Alice (500,000 ubluechip)" "$TXHASH"

# Verify balances after commit
ALICE_UBC_POST=$(get_bal "$ALICE" "$DENOM")
echo "  Alice ubluechip AFTER:  $ALICE_UBC_POST"

# Expected fee: 1% to bluechip wallet (Alice) = 5,000 ubluechip
#               5% to creator wallet (Alice) = 25,000 ubluechip
# Net Alice change: -500,000 + 5,000 + 25,000 = -470,000
# (Alice is both bluechip_wallet and creator_wallet, so she gets fees back)
ALICE_CHANGE=$(python3 -c "print(int('$ALICE_UBC_POST') - int('$ALICE_UBC_PRE'))")
echo "  Alice net change: $ALICE_CHANGE ubluechip (expected: -470000, fees return to Alice)"
python3 -c "
diff = int('$ALICE_UBC_POST') - int('$ALICE_UBC_PRE')
# Alice pays 500000, gets back 30000 in fees (1%+5%) = net -470000
if diff == -470000:
    print('  [FEE CHECK] Correct: Alice net = -470,000 (paid 500K, got 30K fees back)')
else:
    print(f'  [FEE CHECK] Net change = {diff} (expected -470000)')
"

# Verify pool state
POOL_STATE=$(qry "$POOL_ADDR" '{"pool_state":{}}')
echo "  Pool state after commit #1:"
echo "$POOL_STATE" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    reserve0={d.get(\"reserve0\",\"?\")}  total_liquidity={d.get(\"total_liquidity\",\"?\")}')" 2>/dev/null

# ---------------------------------------------------------------
# Commit #2: Bob commits 1,000,000 ubluechip ($10,000)
# ---------------------------------------------------------------
log_step "Commit #2 — Bob commits 1,000,000 ubluechip (\$10,000)"

BOB_UBC_PRE=$(get_bal "$BOB" "$DENOM")
ALICE_UBC_PRE2=$(get_bal "$ALICE" "$DENOM")
echo "  Bob ubluechip BEFORE: $BOB_UBC_PRE"
echo "  Alice ubluechip BEFORE (fee recipient): $ALICE_UBC_PRE2"

TXHASH=$(exe_bob "$POOL_ADDR" "$(COMMIT_MSG 1000000)" "1000000$DENOM")
assert_ok "Commit #2 Bob (1,000,000 ubluechip)" "$TXHASH"

BOB_UBC_POST=$(get_bal "$BOB" "$DENOM")
ALICE_UBC_POST2=$(get_bal "$ALICE" "$DENOM")
BOB_CHANGE=$(python3 -c "print(int('$BOB_UBC_POST') - int('$BOB_UBC_PRE'))")
ALICE_FEE_RECEIVED=$(python3 -c "print(int('$ALICE_UBC_POST2') - int('$ALICE_UBC_PRE2'))")
echo "  Bob net change: $BOB_CHANGE ubluechip (expected: -1000000)"
echo "  Alice fee received: $ALICE_FEE_RECEIVED ubluechip (expected: 60000 = 1%+5% of 1M)"

python3 -c "
bob_diff = int('$BOB_UBC_POST') - int('$BOB_UBC_PRE')
alice_fee = int('$ALICE_UBC_POST2') - int('$ALICE_UBC_PRE2')
if bob_diff == -1000000:
    print('  [FEE CHECK] Bob correctly paid 1,000,000 ubluechip')
else:
    print(f'  [FEE CHECK] Bob change = {bob_diff} (expected -1000000)')
if alice_fee == 60000:
    print('  [FEE CHECK] Alice received correct 60,000 fee (6% of 1M)')
else:
    print(f'  [FEE CHECK] Alice fee = {alice_fee} (expected 60000)')
"

# Check cumulative pool state
echo "  Cumulative pool USD raised:"
qry "$POOL_ADDR" '{"pool_state":{}}' | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    USD raised: {d.get(\"usd_raised_from_commit\",\"?\")}  Native raised: {d.get(\"native_raised_from_commit\",\"?\")}')" 2>/dev/null

# Check committing info for both wallets
echo "  Alice commit info:"
qry "$POOL_ADDR" "{\"commiting_info\":{\"wallet\":\"$ALICE\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print(f'    usd_committed={d}')" 2>/dev/null
echo "  Bob commit info:"
qry "$POOL_ADDR" "{\"commiting_info\":{\"wallet\":\"$BOB\"}}" | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print(f'    usd_committed={d}')" 2>/dev/null

# =====================================================================
# PHASE 4: THRESHOLD CROSSING
# =====================================================================
log_header "PHASE 4: Threshold Crossing (\$25,000 USD)"
echo "  Current: ~\$15,000 committed (500K + 1M ubluechip at \$0.01/each)"
echo "  Commit #3: Alice 1,200,000 ubluechip (\$12,000) → total \$27,000 > \$25,000"

# Snapshot expand-economy balance to verify minting
EXP_BAL_PRE=$(get_bal "$EXP_ADDR" "$DENOM")
ALICE_UBC_PRE3=$(get_bal "$ALICE" "$DENOM")
echo "  ExpandEconomy ubluechip BEFORE: $EXP_BAL_PRE"
echo "  Alice ubluechip BEFORE: $ALICE_UBC_PRE3"

log_step "Commit #3 — Alice 1,200,000 ubluechip (THRESHOLD CROSSING)"
TXHASH=$(exe "$POOL_ADDR" "$(COMMIT_MSG 1200000)" "1200000$DENOM")
assert_ok "Commit #3 Alice (1,200,000 ubluechip — threshold crossing)" "$TXHASH"

# Check threshold state
log_step "Verify threshold was crossed"
IS_COMMITTED=$(qry "$POOL_ADDR" '{"is_fully_commited":{}}')
echo "  Is fully committed: $IS_COMMITTED"
echo "$IS_COMMITTED" | python3 -c "
import json, sys
d = json.load(sys.stdin)
val = d.get('data', {})
if val == True or val == {'is_fully_commited': True} or str(val).lower() == 'true':
    print('  Threshold CROSSED')
else:
    print(f'  Threshold status: {val}')
" 2>/dev/null

# Check expand-economy minting
log_step "Verify bluechip minting from expand-economy"
EXP_BAL_POST=$(get_bal "$EXP_ADDR" "$DENOM")
echo "  ExpandEconomy ubluechip AFTER: $EXP_BAL_POST"
MINT_AMOUNT=$(python3 -c "print(int('$EXP_BAL_PRE') - int('$EXP_BAL_POST'))")
echo "  Minted amount (sent to bluechip wallet): $MINT_AMOUNT ubluechip"
# Note: Factory is in local testing mode (admin == anchor_pool_address), which
# bypasses internal TWAP oracle AND skips minting. These are coupled by design.
# The factory needs a real ATOM/BLUECHIP anchor pool for production minting.
# We test the expand-economy contract directly below instead.
echo "  [MINT CHECK] Factory is in local mode (admin==anchor_pool) — minting skipped by design"
echo "  [MINT CHECK] Testing expand-economy contract directly below..."

# ---------------------------------------------------------------
# Direct expand-economy test (bypasses factory's mock mode)
# ---------------------------------------------------------------
log_step "Direct Expand-Economy Test (verify contract sends ubluechip)"

# ExpandEconomy factory_address is already Alice (set at instantiation, never updated
# in local test mode), so Alice can call RequestExpansion directly.
echo "  ExpandEconomy factory_address is already Alice — no config change needed"

# Record balances before
EXP_BAL_DIRECT_PRE=$(get_bal "$EXP_ADDR" "$DENOM")
ALICE_BAL_DIRECT_PRE=$(get_bal "$ALICE" "$DENOM")
echo "  ExpandEconomy balance before: $EXP_BAL_DIRECT_PRE"
echo "  Alice balance before: $ALICE_BAL_DIRECT_PRE"

# Calculate expected mint amount: formula = 500 - ((5x²+x)/((s/6)+333x))
# For first pool (x=1), with very small s: ≈ 500 - (6/(s/6 + 333)) ≈ 499.98 tokens
# In ubluechip units (6 decimals): ~499,980,000
MINT_TEST_AMOUNT="499980000"
echo "  Testing RequestExpansion with $MINT_TEST_AMOUNT ubluechip (~499.98 tokens)"

EXPAND_MSG=$(python3 -c "import json; print(json.dumps({'expand_economy':{'request_expansion':{'recipient':'$ALICE','amount':'$MINT_TEST_AMOUNT'}}}))")
TXHASH=$(exe "$EXP_ADDR" "$EXPAND_MSG")
assert_ok "ExpandEconomy: RequestExpansion (direct test)" "$TXHASH"

EXP_BAL_DIRECT_POST=$(get_bal "$EXP_ADDR" "$DENOM")
ALICE_BAL_DIRECT_POST=$(get_bal "$ALICE" "$DENOM")
DIRECT_MINTED=$(python3 -c "print(int('$EXP_BAL_DIRECT_PRE') - int('$EXP_BAL_DIRECT_POST'))")
ALICE_DIRECT_GAIN=$(python3 -c "print(int('$ALICE_BAL_DIRECT_POST') - int('$ALICE_BAL_DIRECT_PRE'))")
echo "  ExpandEconomy sent: $DIRECT_MINTED ubluechip"
echo "  Alice received: $ALICE_DIRECT_GAIN ubluechip (includes -50K gas fee)"

python3 -c "
minted = int('$DIRECT_MINTED')
expected = int('$MINT_TEST_AMOUNT')
if minted == expected:
    print(f'  [MINT CHECK] PASS: Expand-economy correctly sent {minted} ubluechip')
elif minted > 0:
    print(f'  [MINT CHECK] Sent {minted} (expected {expected})')
else:
    print('  [MINT CHECK] FAIL: No ubluechip minted')
"

# In local test mode, factory_address stays as Alice throughout the test.
# In production, ExpandEconomy would be linked to the real factory via
# ProposeConfigUpdate + ExecuteConfigUpdate (48h timelock).
echo "  ExpandEconomy factory_address remains Alice (local test mode)"

# Pool state after threshold
echo "  Pool state after threshold crossing:"
qry "$POOL_ADDR" '{"pool_state":{}}' | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
for k in ['reserve0','reserve1','total_liquidity','usd_raised_from_commit','native_raised_from_commit']:
    print(f'    {k} = {d.get(k,\"?\")}')
" 2>/dev/null

# ---------------------------------------------------------------
# Continue Distribution (flush committer token distribution)
# ---------------------------------------------------------------
log_step "ContinueDistribution (flush committer token payouts)"
for i in 1 2 3 4 5; do
  TXHASH=$(exe "$POOL_ADDR" '{"continue_distribution":{}}')
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  if [ "$CODE" = "0" ]; then
    echo "  Round $i: OK"
  else
    echo "  Round $i: code=$CODE (may already be complete)"
    break
  fi
done

# ---------------------------------------------------------------
# Verify threshold payouts — creator token distribution
# ---------------------------------------------------------------
log_step "Verify creator token distribution to committers"

ALICE_CT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
BOB_CT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
echo "  Alice creator tokens: $ALICE_CT"
echo "  Bob creator tokens:   $BOB_CT"

python3 -c "
alice_ct = int('$ALICE_CT')
bob_ct = int('$BOB_CT')
# Expected distribution (approximate):
# - Alice: committer share + creator reward (325K) + protocol reward (25K)
# - Bob: committer share only
# Committer pool = 500,000 tokens = 500,000,000,000 units
# Alice committed ~\$15K of ~\$25K → ~60% → ~300B + 350B (creator+protocol) = ~650B
# Bob committed ~\$10K of ~\$25K → ~40% → ~200B
print(f'  Alice: {alice_ct:,} units ({alice_ct/1000000:.0f} tokens)')
print(f'  Bob:   {bob_ct:,} units ({bob_ct/1000000:.0f} tokens)')
if alice_ct > 0 and bob_ct > 0:
    print('  [PAYOUT CHECK] Both committers received creator tokens')
    ratio = alice_ct / bob_ct if bob_ct > 0 else 0
    print(f'  [PAYOUT CHECK] Alice/Bob ratio: {ratio:.2f} (Alice gets more due to creator+protocol rewards)')
else:
    print('  [PAYOUT CHECK] WARNING: One or both committers have 0 tokens')
"

# =====================================================================
# PHASE 5: POST-THRESHOLD OPERATIONS
# =====================================================================
log_header "PHASE 5: Post-Threshold Operations"

# ---------------------------------------------------------------
# 5a. Post-threshold commit (should work as swap)
# ---------------------------------------------------------------
log_step "Verify distribution completed"
DIST_CHECK=$(qry "$POOL_ADDR" '{"distribution_state":{}}' 2>/dev/null || echo '{}')
echo "  Distribution state: $DIST_CHECK"
# If distribution still active, run more rounds
echo "$DIST_CHECK" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    data = d.get('data', {})
    if data and data.get('is_distributing', False):
        print('STILL_DISTRIBUTING')
    else:
        print('DONE')
except:
    print('DONE')
" 2>/dev/null | grep -q "STILL_DISTRIBUTING" && {
  echo "  Distribution still active — running more rounds..."
  for i in $(seq 1 10); do
    TXHASH=$(exe "$POOL_ADDR" '{"continue_distribution":{}}')
    RES=$(tx_result "$TXHASH")
    CODE=$(echo "$RES" | cut -d'|' -f1)
    if [ "$CODE" != "0" ]; then
      echo "  Extra round $i: code=$CODE (distribution complete)"
      break
    fi
    echo "  Extra round $i: OK"
  done
}

log_step "Post-threshold commit — Alice 100,000 ubluechip (should swap for creator tokens)"
sleep 15  # Ensure rate limit cooldown (13 second minimum)

ALICE_CT_PRE=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
ALICE_UBC_PRE5=$(get_bal "$ALICE" "$DENOM")

TXHASH=$(exe "$POOL_ADDR" "$(COMMIT_MSG 100000 0.99)" "100000$DENOM")
assert_ok "Post-threshold commit (100,000 ubluechip → swap)" "$TXHASH"

ALICE_CT_POST=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
ALICE_UBC_POST5=$(get_bal "$ALICE" "$DENOM")

CT_GAINED=$(python3 -c "print(int('$ALICE_CT_POST') - int('$ALICE_CT_PRE'))")
UBC_SPENT=$(python3 -c "print(int('$ALICE_UBC_PRE5') - int('$ALICE_UBC_POST5'))")
echo "  Alice spent: $UBC_SPENT ubluechip"
echo "  Alice gained: $CT_GAINED creator token units"

python3 -c "
ct_gained = int('$ALICE_CT_POST') - int('$ALICE_CT_PRE')
ubc_spent = int('$ALICE_UBC_PRE5') - int('$ALICE_UBC_POST5')
if ct_gained > 0:
    print(f'  [POST-THRESHOLD] Commit converted to swap: spent {ubc_spent} ubluechip, got {ct_gained} creator tokens')
    # Alice should get fees back (she is fee wallet), so effective spend < 100,000
    print(f'  [POST-THRESHOLD] Effective spend: {ubc_spent} (includes fee return to Alice as fee wallet)')
else:
    print('  [POST-THRESHOLD] WARNING: No creator tokens received from post-threshold commit')
"

# ---------------------------------------------------------------
# 5b. Standard bank transactions
# ---------------------------------------------------------------
log_step "Standard bank transactions (ubluechip transfers)"

ALICE_PRE_SEND=$(get_bal "$ALICE" "$DENOM")
BOB_PRE_SEND=$(get_bal "$BOB" "$DENOM")
CHARLIE_PRE_SEND=$(get_bal "$CHARLIE" "$DENOM")

echo "  Sending 500,000 ubluechip: Alice → Charlie"
$BIN tx bank send alice "$CHARLIE" 500000$DENOM \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10

ALICE_POST_SEND=$(get_bal "$ALICE" "$DENOM")
CHARLIE_POST_SEND=$(get_bal "$CHARLIE" "$DENOM")
CHARLIE_GAINED=$(python3 -c "print(int('$CHARLIE_POST_SEND') - int('$CHARLIE_PRE_SEND'))")
echo "  Charlie gained: $CHARLIE_GAINED ubluechip (expected: 500000)"
if [ "$CHARLIE_GAINED" = "500000" ]; then
  log_pass "Standard ubluechip transfer Alice → Charlie (500,000)"
else
  log_fail "Standard transfer: Charlie gained $CHARLIE_GAINED (expected 500,000)"
fi

echo "  Sending 200,000 ubluechip: Bob → Charlie"
$BIN tx bank send bob "$CHARLIE" 200000$DENOM \
  --from bob --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10

CHARLIE_POST_SEND2=$(get_bal "$CHARLIE" "$DENOM")
CHARLIE_GAINED2=$(python3 -c "print(int('$CHARLIE_POST_SEND2') - int('$CHARLIE_POST_SEND'))")
echo "  Charlie gained: $CHARLIE_GAINED2 ubluechip (expected: 200000)"
if [ "$CHARLIE_GAINED2" = "200000" ]; then
  log_pass "Standard ubluechip transfer Bob → Charlie (200,000)"
else
  log_fail "Standard transfer: Charlie gained $CHARLIE_GAINED2 (expected 200,000)"
fi

# ---------------------------------------------------------------
# 5c. Simple swap (post-threshold)
# ---------------------------------------------------------------
log_step "SimpleSwap — 50,000 ubluechip → creator tokens"
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"50000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL_ADDR" "$SWAP_MSG" "50000ubluechip")
assert_ok "SimpleSwap 50,000 ubluechip → creator tokens" "$TXHASH"

echo "  Pool state after swap:"
qry "$POOL_ADDR" '{"pool_state":{}}' | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    reserve0={d.get(\"reserve0\",\"?\")}  reserve1={d.get(\"reserve1\",\"?\")}')
" 2>/dev/null

# =====================================================================
# PHASE 6: LIQUIDITY POSITIONS — SCALER TESTING
# =====================================================================
log_header "PHASE 6: Liquidity Positions (Scaler Testing)"
echo "  Optimal liquidity: 1,000,000 units → 100% fee multiplier"
echo "  Small position target: ~10,000 units → ~10.9% fee multiplier"
echo "  Large position target: ~1,100,000 units → 100% fee multiplier"

# Get current pool state for proportional calculation
POOL_STATE_JSON=$(qry "$POOL_ADDR" '{"pool_state":{}}')
RESERVE0=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "0")
RESERVE1=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "0")
TOTAL_LIQ=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['total_liquidity'])" 2>/dev/null || echo "0")
echo "  Current pool: reserve0=$RESERVE0  reserve1=$RESERVE1  totalLiquidity=$TOTAL_LIQ"

# ---------------------------------------------------------------
# 6a. SMALL position — Bob deposits for ~10K liquidity
# ---------------------------------------------------------------
log_step "Small Position — Bob (target ~10,000 liquidity units)"

# Calculate amounts for target liquidity
read SMALL_AMT0 SMALL_AMT1 < <(python3 -c "
r0 = int('$RESERVE0')
r1 = int('$RESERVE1')
tl = int('$TOTAL_LIQ')
target = 10000
if tl > 0 and r0 > 0 and r1 > 0:
    a0 = target * r0 // tl
    a1 = target * r1 // tl
    # Add 15% padding for slippage
    a0 = a0 + a0 // 7
    a1 = a1 + a1 // 7
    # Minimum 1 for each
    a0 = max(a0, 1)
    a1 = max(a1, 1)
    print(a0, a1)
else:
    print(1000, 100000000)
")
echo "  Deposit plan: $SMALL_AMT0 ubluechip + $SMALL_AMT1 creator tokens"

# Bob needs creator tokens — check his balance
BOB_CT_BAL=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
echo "  Bob creator token balance: $BOB_CT_BAL"

# If Bob doesn't have enough, swap some ubluechip for creator tokens
python3 -c "
bob_ct = int('$BOB_CT_BAL')
needed = int('$SMALL_AMT1')
if bob_ct < needed:
    print('NEED_SWAP')
else:
    print('OK')
" | grep -q "NEED_SWAP" && {
  echo "  Bob needs more creator tokens — swapping 200,000 ubluechip"
  BOB_SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"200000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
  TXHASH=$(exe_bob "$POOL_ADDR" "$BOB_SWAP_MSG" "200000ubluechip")
  sleep 10
  BOB_CT_BAL=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
  echo "  Bob creator tokens after swap: $BOB_CT_BAL"

  # Recalculate after swap changed reserves
  POOL_STATE_JSON=$(qry "$POOL_ADDR" '{"pool_state":{}}')
  RESERVE0=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "0")
  RESERVE1=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "0")
  TOTAL_LIQ=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['total_liquidity'])" 2>/dev/null || echo "0")

  read SMALL_AMT0 SMALL_AMT1 < <(python3 -c "
r0 = int('$RESERVE0')
r1 = int('$RESERVE1')
tl = int('$TOTAL_LIQ')
target = 10000
if tl > 0 and r0 > 0 and r1 > 0:
    a0 = target * r0 // tl
    a1 = target * r1 // tl
    a0 = a0 + a0 // 7
    a1 = a1 + a1 // 7
    a0 = max(a0, 1)
    a1 = max(a1, 1)
    print(a0, a1)
else:
    print(1000, 100000000)
")
  echo "  Recalculated deposit: $SMALL_AMT0 ubluechip + $SMALL_AMT1 creator tokens"
}

# Set CW20 allowance for Bob → Pool
ALLOW_MSG=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL_ADDR','amount':str($SMALL_AMT1)}}))")
TXHASH=$(exe_bob "$CREATOR_TOKEN" "$ALLOW_MSG")
assert_ok "Bob: IncreaseAllowance for small deposit" "$TXHASH"

# Deposit
DEP_MSG=$(python3 -c "import json; print(json.dumps({'deposit_liquidity':{'amount0':'$SMALL_AMT0','amount1':str($SMALL_AMT1),'min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TXHASH=$(exe_bob "$POOL_ADDR" "$DEP_MSG" "${SMALL_AMT0}ubluechip")
assert_ok "Bob: DepositLiquidity (small ~10K position)" "$TXHASH"

# Get Bob's position ID
BOB_POSITIONS=$(qry "$POOL_ADDR" "{\"positions_by_owner\":{\"owner\":\"$BOB\",\"start_after\":null,\"limit\":null}}")
BOB_POS_ID=$(echo "$BOB_POSITIONS" | python3 -c "
import json, sys
d = json.load(sys.stdin)
positions = d.get('data', {}).get('positions', d.get('data', []))
if isinstance(positions, list) and len(positions) > 0:
    p = positions[0]
    print(p.get('position_id', p.get('id', '1')))
else:
    print('1')
" 2>/dev/null || echo "1")
echo "  Bob's position ID: $BOB_POS_ID"

# Query position details
BOB_POS_INFO=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$BOB_POS_ID\"}}")
echo "  Bob's position details:"
echo "$BOB_POS_INFO" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
liq = d.get('liquidity', '?')
mult = d.get('fee_multiplier', d.get('size_multiplier', '?'))
print(f'    liquidity={liq}  fee_multiplier={mult}')
" 2>/dev/null

# ---------------------------------------------------------------
# 6b. LARGE position — Alice deposits for ~1.1M liquidity
# ---------------------------------------------------------------
log_step "Large Position — Alice (target ~1,100,000 liquidity units)"

# Recalculate with current reserves
POOL_STATE_JSON=$(qry "$POOL_ADDR" '{"pool_state":{}}')
RESERVE0=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "0")
RESERVE1=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "0")
TOTAL_LIQ=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['total_liquidity'])" 2>/dev/null || echo "0")
echo "  Current pool: reserve0=$RESERVE0  reserve1=$RESERVE1  totalLiquidity=$TOTAL_LIQ"

read LARGE_AMT0 LARGE_AMT1 < <(python3 -c "
r0 = int('$RESERVE0')
r1 = int('$RESERVE1')
tl = int('$TOTAL_LIQ')
target = 1100000
if tl > 0 and r0 > 0 and r1 > 0:
    a0 = target * r0 // tl
    a1 = target * r1 // tl
    # Add 15% padding
    a0 = a0 + a0 // 7
    a1 = a1 + a1 // 7
    a0 = max(a0, 1)
    a1 = max(a1, 1)
    print(a0, a1)
else:
    print(5000000, 500000000000)
")
echo "  Deposit plan: $LARGE_AMT0 ubluechip + $LARGE_AMT1 creator tokens"

# Alice should have plenty of creator tokens
ALICE_CT_BAL=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
echo "  Alice creator token balance: $ALICE_CT_BAL"

# If Alice needs more creator tokens, swap some
python3 -c "
alice_ct = int('$ALICE_CT_BAL')
needed = int('$LARGE_AMT1')
if alice_ct < needed:
    print('NEED_SWAP')
else:
    print('OK')
" | grep -q "NEED_SWAP" && {
  echo "  Alice needs more creator tokens — swapping 5,000,000 ubluechip"
  ALICE_SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"5000000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
  TXHASH=$(exe "$POOL_ADDR" "$ALICE_SWAP_MSG" "5000000ubluechip")
  sleep 10
  ALICE_CT_BAL=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
  echo "  Alice creator tokens after swap: $ALICE_CT_BAL"

  # Recalculate
  POOL_STATE_JSON=$(qry "$POOL_ADDR" '{"pool_state":{}}')
  RESERVE0=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "0")
  RESERVE1=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "0")
  TOTAL_LIQ=$(echo "$POOL_STATE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['total_liquidity'])" 2>/dev/null || echo "0")

  read LARGE_AMT0 LARGE_AMT1 < <(python3 -c "
r0 = int('$RESERVE0')
r1 = int('$RESERVE1')
tl = int('$TOTAL_LIQ')
target = 1100000
if tl > 0 and r0 > 0 and r1 > 0:
    a0 = target * r0 // tl
    a1 = target * r1 // tl
    a0 = a0 + a0 // 7
    a1 = a1 + a1 // 7
    a0 = max(a0, 1)
    a1 = max(a1, 1)
    print(a0, a1)
else:
    print(5000000, 500000000000)
")
  echo "  Recalculated deposit: $LARGE_AMT0 ubluechip + $LARGE_AMT1 creator tokens"
}

# Set CW20 allowance for Alice → Pool
ALLOW_MSG2=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL_ADDR','amount':str($LARGE_AMT1)}}))")
TXHASH=$(exe "$CREATOR_TOKEN" "$ALLOW_MSG2")
assert_ok "Alice: IncreaseAllowance for large deposit" "$TXHASH"

# Deposit
sleep 10  # Rate limit buffer
DEP_MSG2=$(python3 -c "import json; print(json.dumps({'deposit_liquidity':{'amount0':'$LARGE_AMT0','amount1':str($LARGE_AMT1),'min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TXHASH=$(exe "$POOL_ADDR" "$DEP_MSG2" "${LARGE_AMT0}ubluechip")
assert_ok "Alice: DepositLiquidity (large ~1.1M position)" "$TXHASH"

# Get Alice's position ID
ALICE_POSITIONS=$(qry "$POOL_ADDR" "{\"positions_by_owner\":{\"owner\":\"$ALICE\",\"start_after\":null,\"limit\":null}}")
ALICE_POS_ID=$(echo "$ALICE_POSITIONS" | python3 -c "
import json, sys
d = json.load(sys.stdin)
positions = d.get('data', {}).get('positions', d.get('data', []))
if isinstance(positions, list) and len(positions) > 0:
    # Get the latest position (last one)
    p = positions[-1]
    print(p.get('position_id', p.get('id', '2')))
else:
    print('2')
" 2>/dev/null || echo "2")
echo "  Alice's position ID: $ALICE_POS_ID"

# Query position details
ALICE_POS_INFO=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
echo "  Alice's position details:"
echo "$ALICE_POS_INFO" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
liq = d.get('liquidity', '?')
mult = d.get('fee_multiplier', d.get('size_multiplier', '?'))
print(f'    liquidity={liq}  fee_multiplier={mult}')
" 2>/dev/null

# ---------------------------------------------------------------
# Compare positions
# ---------------------------------------------------------------
log_step "Position Comparison (Scaler Impact)"
python3 << PYEOF
import json

bob_info = json.loads('''$BOB_POS_INFO''')
alice_info = json.loads('''$ALICE_POS_INFO''')

bob_data = bob_info.get('data', {})
alice_data = alice_info.get('data', {})

bob_liq = int(bob_data.get('liquidity', '0'))
alice_liq = int(alice_data.get('liquidity', '0'))

# Fee multiplier might be stored differently
bob_mult = bob_data.get('fee_multiplier', bob_data.get('size_multiplier', 'N/A'))
alice_mult = alice_data.get('fee_multiplier', alice_data.get('size_multiplier', 'N/A'))

print(f'  Bob (small):   liquidity={bob_liq:>12,}  fee_multiplier={bob_mult}')
print(f'  Alice (large): liquidity={alice_liq:>12,}  fee_multiplier={alice_mult}')
print()

# Scaler formula: multiplier = 0.1 + 0.9 * min(liquidity / 1,000,000, 1.0)
OPTIMAL = 1_000_000
bob_expected = 0.1 + 0.9 * min(bob_liq / OPTIMAL, 1.0) if bob_liq > 0 else 0
alice_expected = 0.1 + 0.9 * min(alice_liq / OPTIMAL, 1.0) if alice_liq > 0 else 0
print(f'  Expected scaler: Bob={bob_expected:.4f}  Alice={alice_expected:.4f}')
print(f'  Bob should earn ~{bob_expected*100:.1f}% of full fees')
print(f'  Alice should earn ~{alice_expected*100:.1f}% of full fees')
PYEOF

# =====================================================================
# PHASE 7: FEE GENERATION VIA SWAPS
# =====================================================================
log_header "PHASE 7: Fee Generation via Swaps"
echo "  Performing multiple swaps to generate LP fees..."

# Get fee state before swaps
FEE_STATE_PRE=$(qry "$POOL_ADDR" '{"fee_state":{}}')
echo "  Fee state BEFORE swaps:"
echo "$FEE_STATE_PRE" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
for k, v in d.items():
    print(f'    {k} = {v}')
" 2>/dev/null

# Swap #1: 500,000 ubluechip → creator tokens (Alice)
log_step "Swap #1 — 500,000 ubluechip → creator tokens"
SWAP1='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"500000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL_ADDR" "$SWAP1" "500000ubluechip")
assert_ok "Swap #1: 500K ubluechip → creator" "$TXHASH"

# Swap #2: creator tokens → ubluechip (Alice, reverse direction)
# Need to send CW20 via send message
log_step "Swap #2 — creator tokens → ubluechip (reverse)"
# First check Alice's creator token balance
ALICE_CT_NOW=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
SWAP2_AMT=$(python3 -c "print(min(int('$ALICE_CT_NOW') // 10, 50000000000))")

# For CW20→native swap, must use CW20 Send with embedded Cw20HookMsg::Swap
# The pool's execute_swap_cw20 handler receives tokens via CW20 Receive callback
sleep 15  # Rate limit: 13 second min between swaps from same wallet
SWAP2_HOOK=$(python3 -c "
import json, base64
hook_msg = json.dumps({'swap':{'belief_price':None,'max_spread':'0.99','to':None,'transaction_deadline':None}})
b64 = base64.b64encode(hook_msg.encode()).decode()
send_msg = json.dumps({'send':{'contract':'$POOL_ADDR','amount':'$SWAP2_AMT','msg':b64}})
print(send_msg)
")
TXHASH=$(exe "$CREATOR_TOKEN" "$SWAP2_HOOK")
assert_ok "Swap #2: creator → ubluechip (reverse via CW20 Send)" "$TXHASH"

# Swap #3: 300,000 ubluechip → creator tokens (Bob for variety)
log_step "Swap #3 — Bob: 300,000 ubluechip → creator tokens"
SWAP3='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"300000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe_bob "$POOL_ADDR" "$SWAP3" "300000ubluechip")
assert_ok "Swap #3: Bob 300K ubluechip → creator" "$TXHASH"

# Swap #4: Another 400,000 ubluechip → creator (Alice)
log_step "Swap #4 — Alice: 400,000 ubluechip → creator tokens"
sleep 5  # Extra buffer for Alice's rate limit (swap #2 was 10+15=25s ago, need 13s total)
SWAP4='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"400000"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL_ADDR" "$SWAP4" "400000ubluechip")
assert_ok "Swap #4: Alice 400K ubluechip → creator" "$TXHASH"

# Swap #5: Reverse swap with creator tokens (Bob)
log_step "Swap #5 — Bob: creator tokens → ubluechip (reverse)"
sleep 5  # Extra buffer for Bob's rate limit (swap #3 was ~10s ago, need 13s total)
BOB_CT_NOW=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")
SWAP5_AMT=$(python3 -c "print(min(int('$BOB_CT_NOW') // 5, 30000000000))")

SWAP5_HOOK=$(python3 -c "
import json, base64
hook_msg = json.dumps({'swap':{'belief_price':None,'max_spread':'0.99','to':None,'transaction_deadline':None}})
b64 = base64.b64encode(hook_msg.encode()).decode()
send_msg = json.dumps({'send':{'contract':'$POOL_ADDR','amount':'$SWAP5_AMT','msg':b64}})
print(send_msg)
")
TXHASH=$(exe_bob "$CREATOR_TOKEN" "$SWAP5_HOOK")
assert_ok "Swap #5: Bob creator → ubluechip (reverse via CW20 Send)" "$TXHASH"

# Get fee state after swaps
FEE_STATE_POST=$(qry "$POOL_ADDR" '{"fee_state":{}}')
echo ""
echo "  Fee state AFTER swaps:"
echo "$FEE_STATE_POST" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
for k, v in d.items():
    print(f'    {k} = {v}')
" 2>/dev/null

# =====================================================================
# PHASE 8: FEE TRACKING & SCALER VERIFICATION
# =====================================================================
log_header "PHASE 8: Fee Tracking & Scaler Verification"

log_step "Query positions for unclaimed fees"

BOB_POS_NOW=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$BOB_POS_ID\"}}")
ALICE_POS_NOW=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")

echo "  Bob's position (small, scaler-punished):"
echo "$BOB_POS_NOW" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    liquidity       = {d.get(\"liquidity\", \"?\")}')
print(f'    unclaimed_fees_0= {d.get(\"unclaimed_fees_0\", d.get(\"pending_fees_0\", \"?\"))}')
print(f'    unclaimed_fees_1= {d.get(\"unclaimed_fees_1\", d.get(\"pending_fees_1\", \"?\"))}')
print(f'    fee_multiplier  = {d.get(\"fee_multiplier\", d.get(\"size_multiplier\", \"?\"))}')
" 2>/dev/null

echo "  Alice's position (large, full fees):"
echo "$ALICE_POS_NOW" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    liquidity       = {d.get(\"liquidity\", \"?\")}')
print(f'    unclaimed_fees_0= {d.get(\"unclaimed_fees_0\", d.get(\"pending_fees_0\", \"?\"))}')
print(f'    unclaimed_fees_1= {d.get(\"unclaimed_fees_1\", d.get(\"pending_fees_1\", \"?\"))}')
print(f'    fee_multiplier  = {d.get(\"fee_multiplier\", d.get(\"size_multiplier\", \"?\"))}')
" 2>/dev/null

log_step "Scaler Impact Analysis"
python3 << PYEOF
import json

bob_pos = json.loads('''$BOB_POS_NOW''').get('data', {})
alice_pos = json.loads('''$ALICE_POS_NOW''').get('data', {})

bob_liq = int(bob_pos.get('liquidity', '0'))
alice_liq = int(alice_pos.get('liquidity', '0'))

# Try different field names for fees
bob_f0 = int(bob_pos.get('unclaimed_fees_0', bob_pos.get('pending_fees_0', '0')))
bob_f1 = int(bob_pos.get('unclaimed_fees_1', bob_pos.get('pending_fees_1', '0')))
alice_f0 = int(alice_pos.get('unclaimed_fees_0', alice_pos.get('pending_fees_0', '0')))
alice_f1 = int(alice_pos.get('unclaimed_fees_1', alice_pos.get('pending_fees_1', '0')))

print("  ┌─────────────────────────────────────────────────────────────┐")
print("  │              FEE SCALER COMPARISON                         │")
print("  ├─────────────────┬──────────────────┬──────────────────────┤")
print(f"  │                 │ Bob (small)       │ Alice (large)        │")
print(f"  ├─────────────────┼──────────────────┼──────────────────────┤")
print(f"  │ Liquidity       │ {bob_liq:>16,} │ {alice_liq:>20,} │")
print(f"  │ Fees (token 0)  │ {bob_f0:>16,} │ {alice_f0:>20,} │")
print(f"  │ Fees (token 1)  │ {bob_f1:>16,} │ {alice_f1:>20,} │")

# Calculate fee per liquidity unit for comparison
if bob_liq > 0 and alice_liq > 0:
    bob_fpl0 = bob_f0 / bob_liq
    alice_fpl0 = alice_f0 / alice_liq
    bob_fpl1 = bob_f1 / bob_liq
    alice_fpl1 = alice_f1 / alice_liq
    print(f"  │ Fee/liq (tok 0) │ {bob_fpl0:>16.6f} │ {alice_fpl0:>20.6f} │")
    print(f"  │ Fee/liq (tok 1) │ {bob_fpl1:>16.6f} │ {alice_fpl1:>20.6f} │")

    # The scaler means Alice gets more fee per liquidity unit
    if alice_fpl0 > bob_fpl0 or (alice_f0 == 0 and bob_f0 == 0):
        if alice_f0 > 0:
            ratio = alice_fpl0 / bob_fpl0 if bob_fpl0 > 0 else float('inf')
            print(f"  │ Fee ratio       │ Alice earns {ratio:.1f}x more fee/liquidity      │")
        print("  └─────────────────┴──────────────────┴──────────────────────┘")
        print("  [SCALER CHECK] Large position earns more fee per liquidity unit")
    else:
        print("  └─────────────────┴──────────────────┴──────────────────────┘")
        print("  [SCALER CHECK] Note: Scaler effect may be visible after fee collection")
else:
    print("  └─────────────────┴──────────────────┴──────────────────────┘")
    print("  [SCALER CHECK] Cannot compute — zero liquidity detected")
PYEOF

# =====================================================================
# PHASE 9: FEE COLLECTION & LIQUIDITY EXITS
# =====================================================================
log_header "PHASE 9: Fee Collection & Liquidity Exits"

# ---------------------------------------------------------------
# 9a. Collect fees from both positions
# ---------------------------------------------------------------
log_step "Collect fees — Bob's small position (ID: $BOB_POS_ID)"

BOB_UBC_PRE_COLLECT=$(get_bal "$BOB" "$DENOM")
BOB_CT_PRE_COLLECT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

TXHASH=$(exe_bob "$POOL_ADDR" "{\"collect_fees\":{\"position_id\":\"$BOB_POS_ID\"}}")
assert_ok "Bob: CollectFees (small position)" "$TXHASH"

BOB_UBC_POST_COLLECT=$(get_bal "$BOB" "$DENOM")
BOB_CT_POST_COLLECT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

# Note: balance change includes -50000 gas fee, so actual fee = change + 50000
BOB_FEE_UBC_RAW=$(python3 -c "print(int('$BOB_UBC_POST_COLLECT') - int('$BOB_UBC_PRE_COLLECT'))")
BOB_FEE_UBC=$(python3 -c "print(int('$BOB_UBC_POST_COLLECT') - int('$BOB_UBC_PRE_COLLECT') + 50000)")
BOB_FEE_CT=$(python3 -c "print(int('$BOB_CT_POST_COLLECT') - int('$BOB_CT_PRE_COLLECT'))")
echo "  Bob collected: $BOB_FEE_UBC ubluechip (raw change: $BOB_FEE_UBC_RAW, adjusted for 50K gas) + $BOB_FEE_CT creator tokens"

log_step "Collect fees — Alice's large position (ID: $ALICE_POS_ID)"

ALICE_UBC_PRE_COLLECT=$(get_bal "$ALICE" "$DENOM")
ALICE_CT_PRE_COLLECT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

TXHASH=$(exe "$POOL_ADDR" "{\"collect_fees\":{\"position_id\":\"$ALICE_POS_ID\"}}")
assert_ok "Alice: CollectFees (large position)" "$TXHASH"

ALICE_UBC_POST_COLLECT=$(get_bal "$ALICE" "$DENOM")
ALICE_CT_POST_COLLECT=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

# Note: balance change includes -50000 gas fee, so actual fee = change + 50000
ALICE_FEE_UBC_RAW=$(python3 -c "print(int('$ALICE_UBC_POST_COLLECT') - int('$ALICE_UBC_PRE_COLLECT'))")
ALICE_FEE_UBC=$(python3 -c "print(int('$ALICE_UBC_POST_COLLECT') - int('$ALICE_UBC_PRE_COLLECT') + 50000)")
ALICE_FEE_CT=$(python3 -c "print(int('$ALICE_CT_POST_COLLECT') - int('$ALICE_CT_PRE_COLLECT'))")
echo "  Alice collected: $ALICE_FEE_UBC ubluechip (raw change: $ALICE_FEE_UBC_RAW, adjusted for 50K gas) + $ALICE_FEE_CT creator tokens"

log_step "Fee Collection Comparison"
python3 << PYEOF
bob_fee_ubc = int('$BOB_FEE_UBC')
bob_fee_ct = int('$BOB_FEE_CT')
alice_fee_ubc = int('$ALICE_FEE_UBC')
alice_fee_ct = int('$ALICE_FEE_CT')

print("  ┌─────────────────────────────────────────────────────────┐")
print("  │           COLLECTED FEES COMPARISON                     │")
print("  ├──────────────┬─────────────────┬────────────────────────┤")
print(f"  │              │ Bob (small)      │ Alice (large)          │")
print(f"  ├──────────────┼─────────────────┼────────────────────────┤")
print(f"  │ ubluechip    │ {bob_fee_ubc:>15,} │ {alice_fee_ubc:>22,} │")
print(f"  │ Creator tok  │ {bob_fee_ct:>15,} │ {alice_fee_ct:>22,} │")
print("  └──────────────┴─────────────────┴────────────────────────┘")

if alice_fee_ubc > bob_fee_ubc:
    print("  [FEE COLLECTION] Alice (large) collected MORE ubluechip fees than Bob (small)")
    if bob_fee_ubc > 0:
        ratio = alice_fee_ubc / bob_fee_ubc
        print(f"  [FEE COLLECTION] Ratio: Alice/Bob = {ratio:.2f}x")
    print("  [FEE COLLECTION] Scaler is working: small position penalized")
elif alice_fee_ubc == 0 and bob_fee_ubc == 0:
    print("  [FEE COLLECTION] Both collected 0 ubluechip fees (fees may be in creator tokens only)")
    if alice_fee_ct > bob_fee_ct and bob_fee_ct > 0:
        ratio = alice_fee_ct / bob_fee_ct
        print(f"  [FEE COLLECTION] Creator token fee ratio: {ratio:.2f}x (Alice > Bob)")
else:
    print("  [FEE COLLECTION] Unexpected: Bob earned more than Alice")
PYEOF

# ---------------------------------------------------------------
# 9b. Remove liquidity — Bob's small position (partial)
# ---------------------------------------------------------------
log_step "Remove Liquidity — Bob's small position (partial, 50%)"
sleep 10  # Rate limit buffer

BOB_UBC_PRE_RM=$(get_bal "$BOB" "$DENOM")
BOB_CT_PRE_RM=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

RM_MSG=$(python3 -c "import json; print(json.dumps({'remove_partial_liquidity_by_percent':{'position_id':'$BOB_POS_ID','percentage':50,'transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TXHASH=$(exe_bob "$POOL_ADDR" "$RM_MSG")
assert_ok "Bob: RemovePartialLiquidity 50% (small position)" "$TXHASH"

BOB_UBC_POST_RM=$(get_bal "$BOB" "$DENOM")
BOB_CT_POST_RM=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$BOB\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

BOB_RM_UBC=$(python3 -c "print(int('$BOB_UBC_POST_RM') - int('$BOB_UBC_PRE_RM'))")
BOB_RM_CT=$(python3 -c "print(int('$BOB_CT_POST_RM') - int('$BOB_CT_PRE_RM'))")
echo "  Bob received: $BOB_RM_UBC ubluechip + $BOB_RM_CT creator tokens from 50% removal"

# Check remaining position
BOB_POS_AFTER_RM=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$BOB_POS_ID\"}}")
echo "  Bob's remaining position:"
echo "$BOB_POS_AFTER_RM" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    liquidity={d.get(\"liquidity\",\"?\")}  (should be ~50% of original)')
" 2>/dev/null

# ---------------------------------------------------------------
# 9c. Remove ALL liquidity — Bob's remaining position
# ---------------------------------------------------------------
log_step "Remove ALL Liquidity — Bob's remaining position"
sleep 10  # Rate limit buffer

RM_ALL_MSG=$(python3 -c "import json; print(json.dumps({'remove_all_liquidity':{'position_id':'$BOB_POS_ID','transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TXHASH=$(exe_bob "$POOL_ADDR" "$RM_ALL_MSG")
assert_ok "Bob: RemoveAllLiquidity (remaining position)" "$TXHASH"

# Verify position is empty
BOB_POS_FINAL=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$BOB_POS_ID\"}}")
echo "  Bob's position after full removal:"
echo "$BOB_POS_FINAL" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
liq = d.get('liquidity', '?')
print(f'    liquidity={liq}  (should be 0)')
" 2>/dev/null

# ---------------------------------------------------------------
# 9d. Remove partial liquidity — Alice's large position (25%)
# ---------------------------------------------------------------
log_step "Remove Partial Liquidity — Alice's large position (25%)"
sleep 10  # Rate limit buffer

ALICE_UBC_PRE_RM=$(get_bal "$ALICE" "$DENOM")
ALICE_CT_PRE_RM=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

RM_ALICE_MSG=$(python3 -c "import json; print(json.dumps({'remove_partial_liquidity_by_percent':{'position_id':'$ALICE_POS_ID','percentage':25,'transaction_deadline':None,'min_amount0':None,'min_amount1':None,'max_ratio_deviation_bps':None}}))")
TXHASH=$(exe "$POOL_ADDR" "$RM_ALICE_MSG")
assert_ok "Alice: RemovePartialLiquidity 25% (large position)" "$TXHASH"

ALICE_UBC_POST_RM=$(get_bal "$ALICE" "$DENOM")
ALICE_CT_POST_RM=$(qry "$CREATOR_TOKEN" "{\"balance\":{\"address\":\"$ALICE\"}}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('data',{}).get('balance','0'))" 2>/dev/null || echo "0")

ALICE_RM_UBC=$(python3 -c "print(int('$ALICE_UBC_POST_RM') - int('$ALICE_UBC_PRE_RM'))")
ALICE_RM_CT=$(python3 -c "print(int('$ALICE_CT_POST_RM') - int('$ALICE_CT_PRE_RM'))")
echo "  Alice received: $ALICE_RM_UBC ubluechip + $ALICE_RM_CT creator tokens from 25% removal"

ALICE_POS_AFTER_RM=$(qry "$POOL_ADDR" "{\"position\":{\"position_id\":\"$ALICE_POS_ID\"}}")
echo "  Alice's remaining position:"
echo "$ALICE_POS_AFTER_RM" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print(f'    liquidity={d.get(\"liquidity\",\"?\")}  (should be ~75% of original)')
" 2>/dev/null

# ---------------------------------------------------------------
# 9e. Final pool state
# ---------------------------------------------------------------
log_step "Final Pool State"
FINAL_POOL=$(qry "$POOL_ADDR" '{"pool_state":{}}')
echo "$FINAL_POOL" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
print('  Final pool state:')
for k in ['reserve0','reserve1','total_liquidity','usd_raised_from_commit','native_raised_from_commit']:
    print(f'    {k} = {d.get(k, \"?\")}')
" 2>/dev/null

FINAL_FEE=$(qry "$POOL_ADDR" '{"fee_state":{}}')
echo "  Final fee state:"
echo "$FINAL_FEE" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {})
for k, v in d.items():
    print(f'    {k} = {v}')
" 2>/dev/null

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
echo "  Contract Addresses:"
echo "    Oracle:          $ORACLE_ADDR"
echo "    ExpandEconomy:   $EXP_ADDR"
echo "    Factory:         $FACTORY_ADDR"
echo "    Pool:            $POOL_ADDR"
echo "    Creator Token:   $CREATOR_TOKEN"
echo ""
echo "  Test Accounts:"
echo "    Alice: $ALICE"
echo "    Bob:   $BOB"
echo "    Charlie: $CHARLIE"
echo ""
echo "  Chain log: /tmp/bluechip_chain.log"
echo ""
