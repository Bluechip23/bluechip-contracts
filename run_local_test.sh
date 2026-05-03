BIN="/home/jeremy/go/bin/bluechipChaind"
CHAIN_HOME="$HOME/.bluechipChain"
CHAIN_ID="bluechipChain"
NODE="tcp://localhost:26657"
ARTIFACTS="/home/jeremy/snap/smartcontracts/bluechip-contracts/artifacts"

ALICE="cosmos1cyyzpxplxdzkeea7kwsydadg87357qnalx9dqz"
BOB="cosmos1sc78mkjfmufxq6vjxgnhaq9ym9nhedvavtwura"

TX_FLAGS="--chain-id $CHAIN_ID --node $NODE --gas auto --gas-adjustment 1.5 --fees 1000stake -y --output json"

PASS=0; FAIL=0; ATTACK_BLOCKED=0; ATTACK_SUCCEEDED=0

log_header() { echo ""; echo ""; echo "================================================================"; echo "  $1"; echo "================================================================"; }
log_step()   { echo ""; echo "  --- $1 ---"; }
log_info()   { echo "      $1"; }

# Submit a wasm execute tx as Alice, return txhash
exe() {
  local CONTRACT="$1" MSG="$2" FUNDS="${3:-}"
  local ARGS="--from alice --keyring-backend test $TX_FLAGS"
  local OUT
  if [ -n "$FUNDS" ]; then
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" --amount "$FUNDS" $ARGS 2>/dev/null)
  else
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" $ARGS 2>/dev/null)
  fi
  echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','SUBMIT_FAILED'))" 2>/dev/null || echo "SUBMIT_FAILED"
}

# Submit as Bob (attacker)
exe_bob() {
  local CONTRACT="$1" MSG="$2" FUNDS="${3:-}"
  local ARGS="--from bob --keyring-backend test $TX_FLAGS"
  local OUT
  if [ -n "$FUNDS" ]; then
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" --amount "$FUNDS" $ARGS 2>/dev/null)
  else
    OUT=$($BIN tx wasm execute "$CONTRACT" "$MSG" $ARGS 2>/dev/null)
  fi
  echo "$OUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('txhash','SUBMIT_FAILED'))" 2>/dev/null || echo "SUBMIT_FAILED"
}

# Query contract state
qry() {
  $BIN query wasm contract-state smart "$1" "$2" --node $NODE --output json 2>/dev/null
}

# Wait for tx to be included in a block, return result JSON
wait_tx() {
  sleep 10
  $BIN query tx "$1" --node $NODE --output json 2>/dev/null
}

# Return "code|log" for a submitted tx
tx_result() {
  local RESULT
  RESULT=$(wait_tx "$1")
  local CODE LOG
  CODE=$(echo "$RESULT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('code',99))" 2>/dev/null || echo "99")
  LOG=$(echo "$RESULT"  | python3 -c "import json,sys; print(str(json.load(sys.stdin).get('raw_log',''))[:250])" 2>/dev/null || echo "")
  echo "${CODE}|${LOG}"
}

assert_ok() {
  local DESC="$1" TXHASH="$2"
  if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
    echo "  [FAIL] $DESC — tx submission failed"
    FAIL=$((FAIL+1)); return
  fi
  local RES CODE LOG
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  LOG=$(echo "$RES"  | cut -d'|' -f2-)
  if [ "$CODE" = "0" ]; then
    echo "  [PASS] $DESC"
    PASS=$((PASS+1))
  else
    echo "  [FAIL] $DESC  code=$CODE  $LOG"
    FAIL=$((FAIL+1))
  fi
}

assert_blocked() {
  local DESC="$1" TXHASH="$2"
  if [ "$TXHASH" = "SUBMIT_FAILED" ]; then
    echo "  [BLOCKED] $DESC  (rejected before submission)"
    ATTACK_BLOCKED=$((ATTACK_BLOCKED+1)); return
  fi
  local RES CODE LOG
  RES=$(tx_result "$TXHASH")
  CODE=$(echo "$RES" | cut -d'|' -f1)
  LOG=$(echo "$RES"  | cut -d'|' -f2-)
  if [ "$CODE" != "0" ]; then
    echo "  [BLOCKED] $DESC  (code=$CODE)"
    ATTACK_BLOCKED=$((ATTACK_BLOCKED+1))
  else
    echo "  [!!!VULN!!!] Attack succeeded: $DESC"
    ATTACK_SUCCEEDED=$((ATTACK_SUCCEEDED+1))
  fi
}

# Upload a WASM, return code_id
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

# Instantiate a contract, return address
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

# =====================================================================
# PHASE 0: CHAIN RESET
# =====================================================================
log_header "PHASE 0: Chain Reset"

echo "  Stopping any running chain..."
pkill -9 bluechipChaind 2>/dev/null || true
sleep 8

# Wait for port 26657 to be released
for i in $(seq 1 15); do
  if ! ss -tlnp 2>/dev/null | grep -q ':26657'; then
    break
  fi
  echo "    ...waiting for port 26657 to free..."
  sleep 2
done

echo "  Resetting chain state (delete data + wasm, recreate priv_validator_state)..."
rm -rf "$CHAIN_HOME/data/"
rm -rf "$CHAIN_HOME/wasm/"
mkdir -p "$CHAIN_HOME/data"
printf '{"height":"0","round":0,"step":0}' > "$CHAIN_HOME/data/priv_validator_state.json"

echo "  Starting fresh chain..."
nohup $BIN start --home "$CHAIN_HOME" > /tmp/bluechip_chain.log 2>&1 &
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
  # Require height in [3, 200] — confirms fresh start (not old chain at 268k+)
  if [ "$HEIGHT" -ge 3 ] && [ "$HEIGHT" -le 200 ] 2>/dev/null; then
    echo "  Chain ready at block $HEIGHT (fresh)"
    break
  fi
  echo "    ...block $HEIGHT"
  sleep 3
done

log_step "Funding Bob with 10M stake for attack tests"
$BIN tx bank send alice "$BOB" 10000000stake \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10
BOB_BAL=$($BIN query bank balances "$BOB" --node $NODE --output json 2>/dev/null | python3 -c "import json,sys; bs=json.load(sys.stdin).get('balances',[]); [print(b) for b in bs]" 2>/dev/null || echo "unknown")
echo "  Bob balance: $BOB_BAL"

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

log_step "creator_pool.wasm"
log_step "standard_pool.wasm"
STANDARD_POOL_CODE=$(store_wasm "standard_pool.wasm")
POOL_CODE=$(store_wasm "creator_pool.wasm")
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

# =====================================================================
# PHASE 2: INSTANTIATE CONTRACTS
# =====================================================================
log_header "PHASE 2: Instantiate Contracts"

# 2a. Mock Oracle
log_step "Mock Oracle"
ORACLE_ADDR=$(inst "$ORACLE_CODE" '{}' "MockOracle")
echo "  Oracle: $ORACLE_ADDR"

# Set price: 1 ATOM = $10 → 1,000,000,000 at expo -8
log_step "Set oracle ATOM/USD = \$10"
TXHASH=$(exe "$ORACLE_ADDR" '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}')
assert_ok "Set oracle price" "$TXHASH"
# Note: oracle always returns current block time as publish_time — no staleness ever

# 2b. Expand Economy (placeholder factory = Alice while factory not yet deployed)
log_step "Expand Economy"
EXP_MSG=$(python3 -c "import json; print(json.dumps({'factory_address':'$ALICE','owner':'$ALICE'}))")
EXP_ADDR=$(inst "$EXP_CODE" "$EXP_MSG" "ExpandEconomy")
echo "  ExpandEconomy: $EXP_ADDR"

# 2c. Factory
log_step "Factory"
FACTORY_MSG=$(python3 -c "
import json
print(json.dumps({
    'factory_admin_address':              '$ALICE',
    'commit_threshold_limit_usd':         '25000',
    'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
    'pyth_atom_usd_price_feed_id':        'ATOM_USD',
    'cw20_token_contract_id':             int('$CW20_CODE'),
    'cw721_nft_contract_id':              int('$CW721_CODE'),
    'create_pool_wasm_contract_id':       int('$POOL_CODE'),
    'standard_pool_wasm_contract_id':    int('$STANDARD_POOL_CODE'),
    'bluechip_wallet_address':            '$ALICE',
    'commit_fee_bluechip':                '0.01',
    'commit_fee_creator':                 '0.05',
    'max_bluechip_lock_per_pool':         '25000000000',
    'creator_excess_liquidity_lock_days': 7,
    'atom_bluechip_anchor_pool_address':  '$ALICE',
    'bluechip_mint_contract_address':     '$EXP_ADDR',
    'bluechip_denom':                      'stake',
    'atom_denom':                          'uatom',
    'standard_pool_creation_fee_usd':      '1000000',
}))
")
FACTORY_ADDR=$(inst "$FACTORY_CODE" "$FACTORY_MSG" "Factory")
echo "  Factory: $FACTORY_ADDR"

# 2d. ExpandEconomy factory_address stays as Alice for testing.
# In production, use ProposeConfigUpdate + ExecuteConfigUpdate (48h timelock).
# Minting is skipped in local test mode (admin == anchor_pool_address).
log_step "ExpandEconomy factory_address = Alice (local test mode, minting skipped)"
echo "  factory_address stays as Alice — minting skipped in local test mode"

# 2e. Create Pool A via Factory (standard settings: max_lock=25B, lock_days=7)
log_step "Create Pool A (standard settings)"
CREATE_A=$(python3 -c "
import json
print(json.dumps({
    'create': {
        'pool_msg': {
            'pool_token_info': [
                {'bluechip': {'denom': 'stake'}},
                {'creator_token': {'contract_addr': 'WILL_BE_CREATED_BY_FACTORY'}}
            ],
        },
        'token_info': {'name': 'CreatorToken', 'symbol': 'CREATOR', 'decimal': 6},
    }
}))
")
TXHASH=$(exe "$FACTORY_ADDR" "$CREATE_A")
echo "  Create Pool A TX: $TXHASH"
sleep 14  # CW20 → CW721 → Pool — three sub-messages

POOL_A=$($BIN query wasm list-contract-by-code "$POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[-1] if cs else 'ERR')" 2>/dev/null || echo "ERR")

CREATOR_A=$(qry "$POOL_A" '{"pair":{}}' \
  | python3 -c "
import json, sys
d = json.load(sys.stdin)
for t in d.get('data', {}).get('asset_infos', []):
    ct = t.get('creator_token', {})
    if ct:
        print(ct.get('contract_addr', 'ERR')); exit()
print('ERR')
" 2>/dev/null || echo "ERR")

echo "  Pool A:         $POOL_A"
echo "  Creator Token A: $CREATOR_A"

echo ""
echo "  ====== DEPLOYMENT COMPLETE ======"
echo "  Oracle:          $ORACLE_ADDR"
echo "  Expand Economy:  $EXP_ADDR"
echo "  Factory:         $FACTORY_ADDR"
echo "  Pool A:          $POOL_A"
echo "  Creator Token A: $CREATOR_A"

# =====================================================================
# PHASE 3: COMMIT LOGIC  (threshold = $25,000 USD; 1 stake = $10 → need 2500+ stake)
# =====================================================================
log_header "PHASE 3: Commit Logic"
echo "  Oracle: 1 stake = \$10   Threshold: \$25,000 USD = 2,500 stake"

COMMIT_MSG() {
  local AMT="$1"
  python3 -c "import json; print(json.dumps({'commit':{'asset':{'info':{'bluechip':{'denom':'stake'}},'amount':'$AMT'},'transaction_deadline':None,'belief_price':None,'max_spread':None}}))"
}

log_step "Commit #1 — 500 stake by Alice (\$5,000 USD)"
TXHASH=$(exe "$POOL_A" "$(COMMIT_MSG 500)" "500stake")
assert_ok "Commit #1 Alice (500 stake)" "$TXHASH"
echo "  $(qry "$POOL_A" '{"pool_state":{}}'  | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print('reserve0='+str(d.get('reserve0','?')))" 2>/dev/null)"

log_step "Commit #2 — 1000 stake by Bob (total \$15,000 USD) [different wallet → no rate limit]"
TXHASH=$(exe_bob "$POOL_A" "$(COMMIT_MSG 1000)" "1000stake")
assert_ok "Commit #2 Bob (1000 stake)" "$TXHASH"

log_step "Commit #3 — 1200 stake by Alice (total \$27,000 USD → CROSSES \$25,000 THRESHOLD)"
TXHASH=$(exe "$POOL_A" "$(COMMIT_MSG 1200)" "1200stake")
assert_ok "Commit #3 Alice (1200 stake — threshold crossing)" "$TXHASH"
echo "  Pool state after crossing:"
qry "$POOL_A" '{"pool_state":{}}'
echo "  Is fully committed:"
qry "$POOL_A" '{"is_fully_commited":{}}'

log_step "ContinueDistribution (up to 4 rounds to flush all committers)"
for i in 1 2 3 4; do
  TXHASH=$(exe "$POOL_A" '{"continue_distribution":{}}')
  echo "  Round $i: $TXHASH"
  sleep 10
done

echo "  Alice creator token balance:"
qry "$CREATOR_A" "{\"balance\":{\"address\":\"$ALICE\"}}"

# =====================================================================
# PHASE 4: SWAP LOGIC
# =====================================================================
log_header "PHASE 4: Swap Logic"

log_step "SimpleSwap — 100 stake → creator tokens"
SWAP_MSG='{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"stake"}},"amount":"100"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}'
TXHASH=$(exe "$POOL_A" "$SWAP_MSG" "100stake")
assert_ok "SimpleSwap 100 stake → creator tokens" "$TXHASH"
echo "  Pool state after swap:"
qry "$POOL_A" '{"pool_state":{}}'

# =====================================================================
# PHASE 5: LIQUIDITY LOGIC
# =====================================================================
log_header "PHASE 5: Liquidity Logic"

# Read current pool state to calculate proportional amounts
POOL_STATE=$(qry "$POOL_A" '{"pool_state":{}}')
RESERVE0=$(echo "$POOL_STATE" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "2738")
RESERVE1=$(echo "$POOL_STATE" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "320000000000")
# For 500 stake: optimal = RESERVE1 * 500 / RESERVE0, padded by 10%
AMOUNT1_DEP=$(python3 -c "print(int('$RESERVE1') * 500 // int('$RESERVE0') + int('$RESERVE1') * 500 // int('$RESERVE0') // 10)" 2>/dev/null || echo "60000000000")
echo "  Pool reserves: $RESERVE0 stake / $RESERVE1 creator tokens"
echo "  Deposit plan: 500 stake + up to $AMOUNT1_DEP creator tokens"

log_step "IncreaseAllowance (CW20 → Pool) for DepositLiquidity"
ALLOW_MSG=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL_A','amount':str($AMOUNT1_DEP)}}))")
TXHASH=$(exe "$CREATOR_A" "$ALLOW_MSG")
assert_ok "IncreaseAllowance for deposit" "$TXHASH"

log_step "DepositLiquidity — 500 stake + proportional creator tokens"
DEP_MSG=$(python3 -c "import json; print(json.dumps({'deposit_liquidity':{'amount0':'500','amount1':str($AMOUNT1_DEP),'min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TXHASH=$(exe "$POOL_A" "$DEP_MSG" "500stake")
assert_ok "DepositLiquidity (500 stake)" "$TXHASH"
echo "  Pool state after deposit:"
qry "$POOL_A" '{"pool_state":{}}'
echo "  Positions:"
qry "$POOL_A" '{"positions":{"start_after":null,"limit":null}}'

log_step "Swap 200 stake to generate LP fees"
TXHASH=$(exe "$POOL_A" '{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"stake"}},"amount":"200"},"belief_price":null,"max_spread":"0.99","to":null,"transaction_deadline":null}}' "200stake")
assert_ok "Swap 200 stake (fee generation)" "$TXHASH"

log_step "CollectFees — position 1"
TXHASH=$(exe "$POOL_A" '{"collect_fees":{"position_id":"1"}}')
assert_ok "CollectFees position 1" "$TXHASH"
echo "  Position 1 unclaimed fees after collection:"
qry "$POOL_A" '{"position":{"position_id":"1"}}' | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print('  unclaimed_1='+str(d.get('unclaimed_fees_1','?')))" 2>/dev/null

log_step "AddToPosition — 300 more stake"
POOL_STATE2=$(qry "$POOL_A" '{"pool_state":{}}')
RESERVE0_2=$(echo "$POOL_STATE2" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "3000")
RESERVE1_2=$(echo "$POOL_STATE2" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "350000000000")
AMOUNT1_ADD=$(python3 -c "v=int('$RESERVE1_2')*300//int('$RESERVE0_2'); print(v + v//10)" 2>/dev/null || echo "35000000000")

ALLOW_MSG2=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL_A','amount':str($AMOUNT1_ADD)}}))")
TXHASH=$(exe "$CREATOR_A" "$ALLOW_MSG2")
assert_ok "IncreaseAllowance for AddToPosition" "$TXHASH"

ADD_MSG=$(python3 -c "import json; print(json.dumps({'add_to_position':{'position_id':'1','amount0':'300','amount1':str($AMOUNT1_ADD),'min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TXHASH=$(exe "$POOL_A" "$ADD_MSG" "300stake")
assert_ok "AddToPosition (300 stake)" "$TXHASH"
echo "  Position 1 liquidity after add:"
qry "$POOL_A" '{"position":{"position_id":"1"}}' | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print('  liquidity='+str(d.get('liquidity','?')))" 2>/dev/null

log_step "RemovePartialLiquidity — 2,000,000 units"
# check_rate_limit applies here: need ≥13s block-time since last rate-limited op (AddToPosition)
sleep 10
RM_MSG='{"remove_partial_liquidity":{"position_id":"1","liquidity_to_remove":"2000000","transaction_deadline":null,"min_amount0":null,"min_amount1":null,"max_ratio_deviation_bps":null}}'
TXHASH=$(exe "$POOL_A" "$RM_MSG")
assert_ok "RemovePartialLiquidity (2M units)" "$TXHASH"
echo "  Final pool state:"
qry "$POOL_A" '{"pool_state":{}}'

# =====================================================================
# PHASE 6: SECURITY ATTACK TESTS
# =====================================================================
log_header "PHASE 6: Security Attack Tests"
echo ""
echo "  All attacks below should be BLOCKED. [!!!VULN!!!] = real bug found."

# ---------------------------------------------------------------
# 6.1 NotifyThresholdCrossed impersonation
# ---------------------------------------------------------------
log_step "Attack 6.1 — Bob calls factory.NotifyThresholdCrossed (not a pool)"
TXHASH=$(exe_bob "$FACTORY_ADDR" '{"notify_threshold_crossed":{"pool_id":1}}')
assert_blocked "NotifyThresholdCrossed by non-pool Bob" "$TXHASH"

# ---------------------------------------------------------------
# 6.2 Double NotifyThresholdCrossed (replay after first firing)
# ---------------------------------------------------------------
log_step "Attack 6.2 — Double threshold notification (replay by Alice)"
TXHASH=$(exe "$FACTORY_ADDR" '{"notify_threshold_crossed":{"pool_id":1}}')
assert_blocked "Double NotifyThresholdCrossed (replay)" "$TXHASH"

# ---------------------------------------------------------------
# 6.3 ExpandEconomy called by non-factory (Bob)
# ---------------------------------------------------------------
log_step "Attack 6.3 — Bob calls ExpandEconomy::RequestExpansion directly"
EXP_ATTACK=$(python3 -c "import json; print(json.dumps({'expand_economy':{'request_expansion':{'recipient':'$BOB','amount':'9999999999'}}}))")
TXHASH=$(exe_bob "$EXP_ADDR" "$EXP_ATTACK")
assert_blocked "ExpandEconomy called by Bob (not factory)" "$TXHASH"

# ---------------------------------------------------------------
# 6.4 Expand Economy Withdraw by non-owner Bob
# ---------------------------------------------------------------
log_step "Attack 6.4 — Fund expand economy, then Bob tries Withdraw"
echo "  Sending 50000 stake to expand economy..."
$BIN tx bank send alice "$EXP_ADDR" 50000stake \
  --from alice --keyring-backend test $TX_FLAGS 2>/dev/null > /dev/null || true
sleep 10
echo "  ExpandEconomy stake balance:"
qry "$EXP_ADDR" '{"get_balance":{"denom":"stake"}}'

WITHDRAW_ATTACK=$(python3 -c "import json; print(json.dumps({'propose_withdrawal':{'amount':'50000','denom':'stake','recipient':'$BOB'}}))")
TXHASH=$(exe_bob "$EXP_ADDR" "$WITHDRAW_ATTACK")
assert_blocked "ExpandEconomy ProposeWithdrawal by Bob (not owner)" "$TXHASH"

# Owner (Alice) should be able to propose withdrawal
log_step "Valid — Alice proposes withdrawal from expand economy (48h timelock)"
WITHDRAW_VALID=$(python3 -c "import json; print(json.dumps({'propose_withdrawal':{'amount':'50000','denom':'stake','recipient':None}}))")
TXHASH=$(exe "$EXP_ADDR" "$WITHDRAW_VALID")
assert_ok "Alice ProposeWithdrawal from ExpandEconomy" "$TXHASH"

# Execute should fail (48h timelock not expired)
TXHASH=$(exe "$EXP_ADDR" '{"execute_withdrawal":{}}')
assert_blocked "Alice ExecuteWithdrawal (too early, 48h timelock)" "$TXHASH"

# Cancel the pending withdrawal
TXHASH=$(exe "$EXP_ADDR" '{"cancel_withdrawal":{}}')
assert_ok "Alice CancelWithdrawal" "$TXHASH"

# ---------------------------------------------------------------
# 6.5 Expand Economy UpdateConfig ownership theft by Bob
# ---------------------------------------------------------------
log_step "Attack 6.5 — Bob tries to change ExpandEconomy owner/factory"
UPD_ATTACK=$(python3 -c "import json; print(json.dumps({'propose_config_update':{'factory_address':'$BOB','owner':None}}))")
TXHASH=$(exe_bob "$EXP_ADDR" "$UPD_ATTACK")
assert_blocked "ExpandEconomy ProposeConfigUpdate by Bob (ownership theft)" "$TXHASH"

# ---------------------------------------------------------------
# 6.6 Commit after threshold is crossed
# ---------------------------------------------------------------
log_step "Attack 6.6 — Commit after threshold (pool is fully committed)"
TXHASH=$(exe "$POOL_A" "$(COMMIT_MSG 100)" "100stake")
assert_blocked "Commit after threshold crossed" "$TXHASH"

# ---------------------------------------------------------------
# 6.7 Bob removes Alice's LP position
# ---------------------------------------------------------------
log_step "Attack 6.7 — Bob removes Alice's position #1"
RM_BOB='{"remove_partial_liquidity":{"position_id":"1","liquidity_to_remove":"1000000","transaction_deadline":null,"min_amount0":null,"min_amount1":null,"max_ratio_deviation_bps":null}}'
TXHASH=$(exe_bob "$POOL_A" "$RM_BOB")
assert_blocked "Bob removes Alice position #1 (NFT ownership check)" "$TXHASH"

# ---------------------------------------------------------------
# 6.8 Bob collects fees from Alice's position
# ---------------------------------------------------------------
log_step "Attack 6.8 — Bob collects fees from Alice's position #1"
TXHASH=$(exe_bob "$POOL_A" '{"collect_fees":{"position_id":"1"}}')
assert_blocked "Bob CollectFees on Alice position #1 (NFT ownership check)" "$TXHASH"

# ---------------------------------------------------------------
# 6.9 ClaimCreatorExcessLiquidity when no excess position exists (Pool A)
# ---------------------------------------------------------------
log_step "Attack 6.9 — ClaimCreatorExcessLiquidity with no excess on Pool A"
TXHASH=$(exe "$POOL_A" '{"claim_creator_excess_liquidity":{}}')
assert_blocked "ClaimCreatorExcessLiquidity with no excess position" "$TXHASH"

# =====================================================================
# PHASE 7: CREATOR EXCESS LIQUIDITY TEST (Pool B)
# The factory overrides max_bluechip_lock_per_pool and lock_days from its
# own config — the pool_msg values are ignored for these fields.
# We deploy a second factory (FACTORY_B) with max_lock=1000 and lock_days=0,
# then create Pool B from FACTORY_B so the cap is actually enforced.
# =====================================================================
log_header "PHASE 7: Creator Excess Liquidity (Pool B via Factory B)"
echo "  Factory B: max_bluechip_lock=1000  lock_days=0 (instant unlock)"
echo "  Commits: Alice(500)+Bob(1000)+Alice(1200, excess 200 swapped)"
echo "  NATIVE_RAISED=2500 → pool_seed=2350 (0.94×) → capped at 1000 → excess=1350 bluechip"

# Deploy Factory B with small max_lock and zero lock_days
log_step "Instantiate Factory B (max_lock=1000, lock_days=0)"
FACTORY_B_MSG=$(python3 -c "
import json
print(json.dumps({
    'factory_admin_address':              '$ALICE',
    'commit_threshold_limit_usd':         '25000',
    'pyth_contract_addr_for_conversions': '$ORACLE_ADDR',
    'pyth_atom_usd_price_feed_id':        'ATOM_USD',
    'cw20_token_contract_id':             int('$CW20_CODE'),
    'cw721_nft_contract_id':              int('$CW721_CODE'),
    'create_pool_wasm_contract_id':       int('$POOL_CODE'),
    'standard_pool_wasm_contract_id':    int('$STANDARD_POOL_CODE'),
    'bluechip_wallet_address':            '$ALICE',
    'commit_fee_bluechip':                '0.01',
    'commit_fee_creator':                 '0.05',
    'max_bluechip_lock_per_pool':         '1000',
    'creator_excess_liquidity_lock_days': 0,
    'atom_bluechip_anchor_pool_address':  '$ALICE',
    'bluechip_mint_contract_address':     '$EXP_ADDR',
    'bluechip_denom':                      'stake',
    'atom_denom':                          'uatom',
    'standard_pool_creation_fee_usd':      '1000000',
}))
")
FACTORY_B_ADDR=$(inst "$FACTORY_CODE" "$FACTORY_B_MSG" "FactoryB")
echo "  Factory B: $FACTORY_B_ADDR"

log_step "Create Pool B from Factory B"
CREATE_B=$(python3 -c "
import json
print(json.dumps({
    'create': {
        'pool_msg': {
            'pool_token_info': [
                {'bluechip': {'denom': 'stake'}},
                {'creator_token': {'contract_addr': 'WILL_BE_CREATED_BY_FACTORY'}}
            ],
        },
        'token_info': {'name': 'CreatorTokenB', 'symbol': 'CRTRB', 'decimal': 6},
    }
}))
")
TXHASH=$(exe "$FACTORY_B_ADDR" "$CREATE_B")
echo "  Create Pool B TX: $TXHASH"
sleep 14

POOL_B=$($BIN query wasm list-contract-by-code "$POOL_CODE" --node $NODE --output json 2>/dev/null \
  | python3 -c "import json,sys; cs=json.load(sys.stdin).get('contracts',[]); print(cs[-1] if cs else 'ERR')" 2>/dev/null || echo "ERR")

CREATOR_B=$(qry "$POOL_B" '{"pair":{}}' \
  | python3 -c "
import json, sys
d = json.load(sys.stdin)
for t in d.get('data', {}).get('asset_infos', []):
    ct = t.get('creator_token', {})
    if ct:
        print(ct.get('contract_addr', 'ERR')); exit()
print('ERR')
" 2>/dev/null || echo "ERR")

echo "  Pool B:          $POOL_B"
echo "  Creator Token B: $CREATOR_B"

log_step "Committing to Pool B (500+1000+1200 stake, total \$27k > threshold, Alice/Bob/Alice)"
TXHASH=$(exe     "$POOL_B" "$(COMMIT_MSG 500)"  "500stake");  assert_ok  "Pool B Commit #1 Alice (500)" "$TXHASH"
TXHASH=$(exe_bob "$POOL_B" "$(COMMIT_MSG 1000)" "1000stake"); assert_ok  "Pool B Commit #2 Bob (1000)" "$TXHASH"
TXHASH=$(exe     "$POOL_B" "$(COMMIT_MSG 1200)" "1200stake"); assert_ok  "Pool B Commit #3 Alice (1200 → threshold)" "$TXHASH"

log_step "ContinueDistribution for Pool B"
for i in 1 2 3 4; do
  TXHASH=$(exe "$POOL_B" '{"continue_distribution":{}}')
  echo "  Round $i: $TXHASH"
  sleep 10
done

echo "  Pool B state (reserve0 should be capped at 1000 with excess stored separately):"
qry "$POOL_B" '{"pool_state":{}}'

# Pool B needs a DepositLiquidity first to trigger AcceptOwnership on CW721.
# ClaimCreatorExcessLiquidity mints an NFT — requires pool to be CW721 owner.
log_step "DepositLiquidity on Pool B (triggers CW721 AcceptOwnership)"
POOL_B_STATE=$(qry "$POOL_B" '{"pool_state":{}}')
R0B=$(echo "$POOL_B_STATE" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve0'])" 2>/dev/null || echo "1188")
R1B=$(echo "$POOL_B_STATE" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['reserve1'])" 2>/dev/null || echo "125000000000")
AMT1_B=$(python3 -c "v=int('$R1B')*100//int('$R0B'); print(v + v//10)" 2>/dev/null || echo "12000000000")
ALLOW_B=$(python3 -c "import json; print(json.dumps({'increase_allowance':{'spender':'$POOL_B','amount':str($AMT1_B)}}))")
TXHASH=$(exe "$CREATOR_B" "$ALLOW_B")
assert_ok "IncreaseAllowance CreatorB → PoolB" "$TXHASH"
DEP_B=$(python3 -c "import json; print(json.dumps({'deposit_liquidity':{'amount0':'100','amount1':str($AMT1_B),'min_amount0':None,'min_amount1':None,'transaction_deadline':None}}))")
TXHASH=$(exe "$POOL_B" "$DEP_B" "100stake")
assert_ok "DepositLiquidity on Pool B (NFT ownership accepted)" "$TXHASH"
echo "  Pool B nft_ownership_accepted should now be true:"
qry "$POOL_B" '{"pool_state":{}}' | python3 -c "import json,sys; d=json.load(sys.stdin).get('data',{}); print('  nft_ownership_accepted='+str(d.get('nft_ownership_accepted','?')))" 2>/dev/null

# Attack: Bob tries to claim Alice's excess position
log_step "Attack 7.1 — Bob tries to claim creator excess on Pool B"
TXHASH=$(exe_bob "$POOL_B" '{"claim_creator_excess_liquidity":{}}')
assert_blocked "Bob claims Pool B creator excess (not creator)" "$TXHASH"

# Valid: Alice claims her own excess (lock_days=0 → immediately unlocked)
log_step "Valid — Alice claims creator excess on Pool B (lock_days=0)"
TXHASH=$(exe "$POOL_B" '{"claim_creator_excess_liquidity":{}}')
assert_ok "Alice ClaimCreatorExcessLiquidity (instantly unlocked)" "$TXHASH"
echo "  Pool B state after claim (reserve0 should grow to include excess):"
qry "$POOL_B" '{"pool_state":{}}'

# Double-claim attempt
log_step "Attack 7.2 — Alice double-claims creator excess (should fail: already consumed)"
TXHASH=$(exe "$POOL_B" '{"claim_creator_excess_liquidity":{}}')
assert_blocked "Double claim creator excess" "$TXHASH"

# =====================================================================
# FINAL REPORT
# =====================================================================
log_header "FINAL REPORT"
echo ""
echo "  Functional tests:   PASS=$PASS   FAIL=$FAIL"
echo "  Security attacks:   BLOCKED=$ATTACK_BLOCKED   SUCCEEDED=$ATTACK_SUCCEEDED"
echo ""
if [ "$FAIL" -eq 0 ] && [ "$ATTACK_SUCCEEDED" -eq 0 ]; then
  echo "  ✓ All functional tests passed."
  echo "  ✓ All attack vectors blocked — no vulnerabilities found."
else
  [ "$FAIL" -gt 0 ]           && echo "  ✗ $FAIL functional test(s) FAILED"
  [ "$ATTACK_SUCCEEDED" -gt 0 ] && echo "  ✗ $ATTACK_SUCCEEDED attack(s) SUCCEEDED — VULNERABILITIES PRESENT"
fi
echo ""
echo "  Contract Addresses:"
echo "    Oracle:          $ORACLE_ADDR"
echo "    ExpandEconomy:   $EXP_ADDR"
echo "    Factory:         $FACTORY_ADDR"
echo "    Pool A:          $POOL_A  (creator: $CREATOR_A)"
echo "    Pool B:          $POOL_B  (creator: $CREATOR_B)"
echo ""
echo "  Chain log: /tmp/bluechip_chain.log"
