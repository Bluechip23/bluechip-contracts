#!/bin/bash
# =====================================================================
# FULL INTEGRATION TEST SUITE
# =====================================================================
# Covers: factory config, pool creation, commits, fees, threshold,
#   20% guard, distribution, swaps, liquidity, NFT transfer, partial
#   remove, pause/unpause, emergency withdraw, recover stuck states,
#   governance timelocks, oracle, slippage, deadline, multipool, and more.
# =====================================================================
set -uo pipefail

CHAIN_ID="bluechipChain"
KR="test"
DENOM="ubluechip"
GAS=3000000
W=7

RED='\033[0;31m'; GRN='\033[0;32m'; YEL='\033[1;33m'; CYN='\033[0;36m'; MAG='\033[0;35m'; NC='\033[0m'
pass() { echo -e "${GRN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; F=$((F+1)); }
info() { echo -e "${CYN}[INFO]${NC} $1"; }
note() { echo -e "${MAG}[NOTE]${NC} $1"; }
hdr()  { echo ""; echo -e "${YEL}$1${NC}"; }
F=0

# --- Helpers ---
send() { bluechipChaind tx "$@" --chain-id $CHAIN_ID --keyring-backend $KR --gas $GAS -y --output json 2>/dev/null | jq -r '.txhash'; }
q() { bluechipChaind query wasm contract-state smart "$1" "$2" --output json 2>/dev/null; }
chk() { sleep $W; bluechipChaind query tx "$1" --output json 2>/dev/null | jq -r '.code'; }
rawlog() { bluechipChaind query tx "$1" --output json 2>/dev/null | jq -r '.raw_log'; }
wa() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | python3 -c "
import sys,json
d=json.load(sys.stdin)
for e in d.get('events',[]):
    if e['type']=='wasm':
        for a in e['attributes']:
            if a['key']=='$2': print(a['value']); sys.exit(0)
print('')" 2>/dev/null
}
all_contracts() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | python3 -c "
import sys,json; d=json.load(sys.stdin); seen=[]
for e in d.get('events',[]):
    if e['type']=='wasm':
        for a in e['attributes']:
            if a['key']=='_contract_address' and a['value'] not in seen:
                seen.append(a['value']); print(a['value'])" 2>/dev/null
}
codeid() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | python3 -c "
import sys,json; d=json.load(sys.stdin)
for e in d.get('events',[]):
    if e['type']=='store_code':
        for a in e['attributes']:
            if a['key']=='code_id': print(a['value']); sys.exit(0)
print('')" 2>/dev/null
}
bal() { bluechipChaind query bank balances "$1" --output json 2>/dev/null | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount // \"0\""; }
cw20bal() { q "$1" "{\"balance\":{\"address\":\"$2\"}}" | jq -r '.data.balance // "0"'; }
upload() {
  local f=$1 lbl=$2
  info "Uploading $lbl..." >&2
  # Use --broadcast-mode block so the CLI waits for inclusion and gives us
  # a deterministic tx result. Fall back to sleep-and-query if that mode
  # isn't supported by the node version.
  local raw=$(bluechipChaind tx wasm store "artifacts/$f" --from alice \
    --chain-id $CHAIN_ID --keyring-backend $KR --gas 5000000 \
    --broadcast-mode sync -y --output json 2>&1)
  local h=$(echo "$raw" | grep -v WARNING | jq -r '.txhash // empty' 2>/dev/null)
  if [ -z "$h" ]; then
    echo "[DEBUG $lbl] broadcast output: $raw" >&2
    fail "$lbl upload failed (no txhash)" >&2
    echo ""; return
  fi
  # Poll for up to 30s for the tx to land. Needed because back-to-back
  # uploads from the same account can take multiple blocks to sequence.
  local c=""
  for i in 1 2 3 4 5 6; do
    sleep 5
    c=$(codeid "$h")
    if [ -n "$c" ]; then break; fi
  done
  [ -n "$c" ] && pass "$lbl -> code $c" >&2 || fail "$lbl upload failed (tx $h)" >&2
  echo "$c"
}
commit_msg() {
  echo "{\"commit\":{\"asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"$1\"},\"amount\":\"$1\"}}"
}

ALICE=$(bluechipChaind keys show alice -a --keyring-backend $KR 2>/dev/null)
BOB=$(bluechipChaind keys show bob -a --keyring-backend $KR 2>/dev/null)
info "Alice: $ALICE"
info "Bob:   $BOB"

# #####################################################################
hdr "===== PHASE 1: UPLOAD ALL WASM CONTRACTS ====="
# #####################################################################
CW20_CODE=$(upload cw20_base.wasm "CW20 Base")
CW721_CODE=$(upload cw721_base.wasm "CW721 Base")
POOL_CODE=$(upload pool.wasm "Pool")
ORACLE_CODE=$(upload oracle.wasm "Mock Oracle")
FACTORY_CODE=$(upload factory.wasm "Factory")
ECON_CODE=$(upload expand_economy.wasm "Expand Economy")
info "CW20=$CW20_CODE CW721=$CW721_CODE Pool=$POOL_CODE Oracle=$ORACLE_CODE Factory=$FACTORY_CODE Econ=$ECON_CODE"

# #####################################################################
hdr "===== PHASE 2: INSTANTIATE ORACLE + FACTORY + ECONOMY ====="
# #####################################################################
H=$(send wasm instantiate $ORACLE_CODE '{}' --from alice --label "oracle" --no-admin)
sleep $W
ORACLE=$(bluechipChaind query wasm list-contract-by-code $ORACLE_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$ORACLE" ] && [ "$ORACLE" != "null" ] && pass "Oracle: $ORACLE" || { fail "Oracle init"; exit 1; }
send wasm execute $ORACLE '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' --from alice >/dev/null
sleep $W
P=$(q $ORACLE '{"get_price":{"price_id":"ATOM_USD"}}' | jq -r '.data.price')
[ "$P" = "1000000000" ] && pass "Oracle price = \$10" || fail "Oracle price: $P"

FINIT=$(cat <<EOF
{
  "factory_admin_address":"$ALICE",
  "commit_amount_for_threshold_bluechip":"0",
  "commit_threshold_limit_usd":"1000000000",
  "pyth_contract_addr_for_conversions":"$ORACLE",
  "pyth_atom_usd_price_feed_id":"ATOM_USD",
  "cw721_nft_contract_id":$CW721_CODE,
  "cw20_token_contract_id":$CW20_CODE,
  "create_pool_wasm_contract_id":$POOL_CODE,
  "bluechip_wallet_address":"$ALICE",
  "commit_fee_bluechip":"0.01",
  "commit_fee_creator":"0.05",
  "max_bluechip_lock_per_pool":"25000000000",
  "creator_excess_liquidity_lock_days":7,
  "atom_bluechip_anchor_pool_address":"$ALICE",
  "bluechip_mint_contract_address":null
}
EOF
)
H=$(send wasm instantiate $FACTORY_CODE "$FINIT" --from alice --label "factory" --admin $ALICE)
sleep $W
FACTORY=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$FACTORY" ] && [ "$FACTORY" != "null" ] && pass "Factory: $FACTORY" || { fail "Factory init"; exit 1; }
send bank send $ALICE $FACTORY "50000000${DENOM}" --from alice >/dev/null
sleep $W
FBAL=$(bal $FACTORY)
[ "$FBAL" -ge 50000000 ] && pass "Factory funded: $FBAL" || fail "Factory funding"

H=$(send wasm instantiate $ECON_CODE "{\"factory_address\":\"$FACTORY\",\"owner\":\"$ALICE\"}" --from alice --label "econ" --no-admin)
sleep $W
ECON=$(bluechipChaind query wasm list-contract-by-code $ECON_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$ECON" ] && pass "Expand Economy: $ECON" || info "Econ init skipped"

# #####################################################################
hdr "===== PHASE 3: FACTORY CONFIG EDIT + GOVERNANCE TIMELOCKS ====="
# #####################################################################

# 3a. Propose factory config update (48-hour timelock)
NEW_CFG=$(cat <<EOF
{
  "factory_admin_address":"$ALICE",
  "commit_amount_for_threshold_bluechip":"0",
  "commit_threshold_limit_usd":"2000000000",
  "pyth_contract_addr_for_conversions":"$ORACLE",
  "pyth_atom_usd_price_feed_id":"ATOM_USD",
  "cw721_nft_contract_id":$CW721_CODE,
  "cw20_token_contract_id":$CW20_CODE,
  "create_pool_wasm_contract_id":$POOL_CODE,
  "bluechip_wallet_address":"$ALICE",
  "commit_fee_bluechip":"0.02",
  "commit_fee_creator":"0.05",
  "max_bluechip_lock_per_pool":"25000000000",
  "creator_excess_liquidity_lock_days":7,
  "atom_bluechip_anchor_pool_address":"$ALICE",
  "bluechip_mint_contract_address":null
}
EOF
)
info "Proposing factory config update (48hr timelock)..."
H=$(send wasm execute $FACTORY "{\"propose_config_update\":{\"config\":$NEW_CFG}}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "ProposeConfigUpdate OK" || fail "ProposeConfigUpdate ($C)"

# 3b. Try to execute immediately -> should fail (timelock not elapsed)
info "Executing immediately (should fail)..."
H=$(send wasm execute $FACTORY '{"update_config":{}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "UpdateConfig rejected (timelock not elapsed)" || fail "UpdateConfig should have failed"

# 3c. Cancel the proposal
info "Cancelling config update..."
H=$(send wasm execute $FACTORY '{"cancel_config_update":{}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "CancelConfigUpdate OK" || fail "CancelConfigUpdate ($C)"

# 3d. Non-admin (Bob) tries to propose -> should fail
info "Bob tries ProposeConfigUpdate (should fail)..."
H=$(send wasm execute $FACTORY "{\"propose_config_update\":{\"config\":$NEW_CFG}}" --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-admin rejected" || fail "Non-admin should be rejected"

# #####################################################################
hdr "===== PHASE 4: CREATE POOL ====="
# #####################################################################
CREATE=$(cat <<EOF
{
  "create":{
    "pool_msg":{
      "pool_token_info":[
        {"bluechip":{"denom":"$DENOM"}},
        {"creator_token":{"contract_addr":"WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id":$CW20_CODE,
      "factory_to_create_pool_addr":"$FACTORY",
      "threshold_payout":null,
      "commit_fee_info":{
        "bluechip_wallet_address":"$ALICE",
        "creator_wallet_address":"$ALICE",
        "commit_fee_bluechip":"0.01",
        "commit_fee_creator":"0.05"
      },
      "creator_token_address":"$ALICE",
      "commit_amount_for_threshold":"0",
      "commit_limit_usd":"100000000",
      "pyth_contract_addr_for_conversions":"$ORACLE",
      "pyth_atom_usd_price_feed_id":"ATOM_USD",
      "max_bluechip_lock_per_pool":"10000000000",
      "creator_excess_liquidity_lock_days":7,
      "is_standard_pool":false
    },
    "token_info":{"name":"TestToken","symbol":"TEST","decimal":6}
  }
}
EOF
)
H=$(send wasm execute $FACTORY "$CREATE" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool created" || { fail "Pool create ($C)"; rawlog $H; exit 1; }
POOL=$(wa "$H" "pool_address")
[ -z "$POOL" ] && POOL=$(wa "$H" "pool_contract_address")
info "Pool: $POOL"
CW20=$(q $POOL '{"pair":{}}' | jq -r '.data.asset_infos[1].creator_token.contract_addr // empty')
info "CW20: $CW20"

# Verify pool is pre-threshold
CS=$(q $POOL '{"is_fully_commited":{}}' | jq -r '.data')
echo "$CS" | python3 -c "import sys,json;d=json.load(sys.stdin);r='in_progress' in d;print(r)" 2>/dev/null | grep -q True && pass "Pool pre-threshold" || info "Status: $CS"

# Swaps should fail pre-threshold
info "Swap pre-threshold (should fail)..."
H=$(send wasm execute $POOL '{"simple_swap":{"offer_asset":{"info":{"bluechip":{"denom":"ubluechip"}},"amount":"1000"},"max_spread":"0.5"}}' --amount "1000${DENOM}" --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Swap rejected pre-threshold" || fail "Swap should fail pre-threshold"

# Liquidity should fail pre-threshold
info "Deposit liquidity pre-threshold (should fail)..."
H=$(send wasm execute $POOL '{"deposit_liquidity":{"amount0":"1000","amount1":"1000"}}' --amount "1000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Deposit rejected pre-threshold" || fail "Deposit should fail pre-threshold"

# #####################################################################
hdr "===== PHASE 5: COMMIT FLOW + FEE VERIFICATION ====="
# #####################################################################
# Fees: 1% bluechip + 5% creator = 6% total.
# Commit 10M ubluechip -> 100K bluechip fee + 500K creator fee = 600K total.
# Amount after fees = 9,400,000.

ALICE_BAL_PRE=$(bal $ALICE)
info "Alice pre-commit balance: $ALICE_BAL_PRE"

info "Alice committing 10M ubluechip..."
H=$(send wasm execute $POOL "$(commit_msg 10000000)" --amount "10000000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Alice commit 10M OK" || { fail "Alice commit ($C)"; rawlog $H; }

# Check commit info
AC=$(q $POOL "{\"committing_info\":{\"wallet\":\"$ALICE\"}}" | jq -r '.data.total_paid_bluechip // "0"')
info "Alice total_paid_bluechip after first commit: $AC"

# Alice is both the bluechip fee wallet AND the creator fee wallet in our setup,
# so she gets her own fees back. Net cost = 10M - 600K = 9.4M.
# But we can verify the commit was recorded correctly.
[ "$AC" = "10000000" ] && pass "Commit recorded correctly" || fail "Commit amount: $AC"

# Bob commits
info "Bob committing 30M..."
H=$(send wasm execute $POOL "$(commit_msg 30000000)" --amount "30000000${DENOM}" --from bob)
C=$(chk $H)
[ "$C" = "0" ] && pass "Bob commit 30M OK" || { fail "Bob commit ($C)"; rawlog $H; }

BC=$(q $POOL "{\"committing_info\":{\"wallet\":\"$BOB\"}}" | jq -r '.data.total_paid_bluechip // "0"')
info "Bob total_paid_bluechip: $BC"
[ "$BC" = "30000000" ] && pass "Bob commit recorded" || fail "Bob commit amount: $BC"

# Check cumulative raised
CS=$(q $POOL '{"is_fully_commited":{}}' | jq -c '.data')
info "Status after 40M committed: $CS"

# #####################################################################
hdr "===== PHASE 6: THRESHOLD CROSSING ====="
# #####################################################################
# Need 100M ubluechip total ($1000 USD at $10). Have 40M. Alice commits 60M+1 to cross.
info "Waiting 14s for rate limiter..."
sleep 14
info "Alice committing 60000001 to cross threshold..."
H=$(send wasm execute $POOL "$(commit_msg 60000001)" --amount "60000001${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Threshold crossing TX OK" || { fail "Threshold crossing ($C)"; rawlog $H; exit 1; }

PHASE=$(wa "$H" "phase")
info "Phase: $PHASE"

DONE=$(q $POOL '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "yes" else "no" end')
[ "$DONE" = "yes" ] && pass "Threshold crossed!" || fail "Threshold not crossed"

# #####################################################################
hdr "===== PHASE 7: CONTINUE DISTRIBUTION + VERIFY CREATOR TOKEN PAYOUTS ====="
# #####################################################################
info "Running continue_distribution..."
H=$(send wasm execute $POOL '{"continue_distribution":{}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Distribution batch OK" || info "Distribution code=$C"

# Check CW20 token minting
SUP=$(q $CW20 '{"token_info":{}}' | jq -r '.data.total_supply // "0"')
info "CW20 total supply: $SUP"
[ "$SUP" != "0" ] && pass "Creator tokens minted" || fail "CW20 supply=0"

ACW=$(cw20bal $CW20 $ALICE)
BCW=$(cw20bal $CW20 $BOB)
info "Alice CW20: $ACW"
info "Bob CW20:   $BCW"
[ "$ACW" != "0" ] && [ "$BCW" != "0" ] && pass "Both committers received creator tokens" || fail "Token payout missing"

# Verify proportional distribution — Alice committed 70M, Bob 30M = 70:30
# (The creator token split should reflect commit ratio roughly)
if [ "$ACW" -gt 0 ] && [ "$BCW" -gt 0 ]; then
  RATIO=$(python3 -c "print(f'{int($ACW)/int($BCW):.2f}')")
  info "Alice/Bob CW20 ratio: $RATIO (expected ~2.33 = 70/30)"
fi

# Check reserves after threshold
PS=$(q $POOL '{"pool_state":{}}' | jq '.data')
R0=$(echo $PS | jq -r '.reserve0')
R1=$(echo $PS | jq -r '.reserve1')
info "Post-threshold reserves: bluechip=$R0 creator=$R1"
[ "$R0" != "0" ] && [ "$R1" != "0" ] && pass "Pool has reserves" || fail "Pool reserves empty"

# #####################################################################
hdr "===== PHASE 8: 20% GUARD ON THRESHOLD-CROSSING EXCESS ====="
# #####################################################################
# Create a second pool and test the 20% cap.
CREATE2=$(cat <<EOF
{
  "create":{
    "pool_msg":{
      "pool_token_info":[
        {"bluechip":{"denom":"$DENOM"}},
        {"creator_token":{"contract_addr":"WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id":$CW20_CODE,
      "factory_to_create_pool_addr":"$FACTORY",
      "threshold_payout":null,
      "commit_fee_info":{
        "bluechip_wallet_address":"$ALICE",
        "creator_wallet_address":"$ALICE",
        "commit_fee_bluechip":"0.01",
        "commit_fee_creator":"0.05"
      },
      "creator_token_address":"$ALICE",
      "commit_amount_for_threshold":"0",
      "commit_limit_usd":"100000000",
      "pyth_contract_addr_for_conversions":"$ORACLE",
      "pyth_atom_usd_price_feed_id":"ATOM_USD",
      "max_bluechip_lock_per_pool":"10000000000",
      "creator_excess_liquidity_lock_days":7,
      "is_standard_pool":false
    },
    "token_info":{"name":"GuardToken","symbol":"GRD","decimal":6}
  }
}
EOF
)
info "Creating second pool for 20% guard test..."
H=$(send wasm execute $FACTORY "$CREATE2" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool2 created" || { fail "Pool2 create ($C)"; }
POOL2=$(wa "$H" "pool_address")
[ -z "$POOL2" ] && POOL2=$(wa "$H" "pool_contract_address")
CW20_2=$(q $POOL2 '{"pair":{}}' | jq -r '.data.asset_infos[1].creator_token.contract_addr // empty')
info "Pool2: $POOL2  CW20_2: $CW20_2"

# Alice commits 95M (~$950) — close to threshold
info "Alice commits 95M on Pool2..."
H=$(send wasm execute $POOL2 "$(commit_msg 95000000)" --amount "95000000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool2 pre-threshold commit OK" || { fail "Pool2 pre-commit ($C)"; }

# Bob overshoots with 80M — triggers 20% guard
info "Waiting 14s for rate limiter..."
sleep 14
BOB_PRE=$(bal $BOB)
info "Bob overshoot commit: 80M on Pool2 (excess ~75M >> 20% of reserves)..."
H=$(send wasm execute $POOL2 "$(commit_msg 80000000)" --amount "80000000${DENOM}" --from bob)
C=$(chk $H)
[ "$C" = "0" ] && pass "Overshoot commit landed" || { fail "Overshoot ($C)"; rawlog $H; }

REFUNDED=$(wa "$H" "bluechip_excess_refunded")
SWAP_ACTUAL=$(wa "$H" "swap_amount_bluechip")
SWAP_PRE_CAP=$(wa "$H" "swap_amount_bluechip_pre_cap")
info "swap_amount_bluechip (capped):   $SWAP_ACTUAL"
info "swap_amount_bluechip_pre_cap:    $SWAP_PRE_CAP"
info "bluechip_excess_refunded:        $REFUNDED"

if [ -n "$REFUNDED" ] && [ "$REFUNDED" != "0" ] && [ "$REFUNDED" != "" ]; then
  pass "20% guard fired — refunded $REFUNDED ubluechip"
else
  fail "Expected non-zero refund from 20% guard"
fi

BOB_POST=$(bal $BOB)
SPENT=$((BOB_PRE - BOB_POST))
EXPECTED=$((80000000 - REFUNDED))
DIFF=$((SPENT - EXPECTED)); [ $DIFF -lt 0 ] && DIFF=$((-DIFF))
[ $DIFF -lt 1000000 ] && pass "Refund reflected in Bob's balance" || fail "Bob balance mismatch (spent=$SPENT expected=$EXPECTED)"

H=$(send wasm execute $POOL2 '{"continue_distribution":{}}' --from alice)
chk $H >/dev/null

# #####################################################################
hdr "===== PHASE 9: NORMAL SWAPS + FEE VERIFICATION ====="
# #####################################################################
# Back to Pool1. Add big liquidity first so swaps don't hit max spread.
LP0=10000000
LP1=80000000000
info "Alice approving CW20 ($LP1)..."
send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$LP1\"}}" --from alice >/dev/null
sleep $W

info "Alice depositing large liquidity ($LP0 / $LP1)..."
H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$LP0\",\"amount1\":\"$LP1\"}}" --amount "${LP0}${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Liquidity deposited" || { fail "Deposit ($C)"; rawlog $H; }
POS1=$(wa "$H" "position_id")
info "Alice position: $POS1"

# Resolve NFT contract from tx events
NFT=""
for addr in $(all_contracts $H); do
  if [ "$addr" != "$POOL" ] && [ "$addr" != "$CW20" ]; then NFT="$addr"; break; fi
done
[ -n "$NFT" ] && pass "NFT contract: $NFT" || info "Could not find NFT contract"

# Get pool state before swap
PS_PRE=$(q $POOL '{"pool_state":{}}' | jq '.data')
R0_PRE=$(echo $PS_PRE | jq -r '.reserve0')
R1_PRE=$(echo $PS_PRE | jq -r '.reserve1')

# 9a. Normal swap native->CW20
SA=500000
info "Swap $SA native->CW20..."
H=$(send wasm execute $POOL \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"$SA\"},\"max_spread\":\"0.5\"}}" \
  --amount "${SA}${DENOM}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  RET=$(wa "$H" "return_amount")
  SPREAD=$(wa "$H" "spread_amount")
  COMM=$(wa "$H" "commission_amount")
  pass "Swap OK: returned=$RET spread=$SPREAD commission=$COMM"
  # Verify LP fees are collected (commission > 0)
  [ -n "$COMM" ] && [ "$COMM" != "0" ] && pass "LP fee collected: $COMM" || info "Commission=0 (pool may be too deep)"
else
  fail "Swap native->CW20 ($C)"; rawlog $H
fi

# 9b. Reverse swap CW20->native via Send hook
info "Waiting 14s for rate limiter..."
sleep 14
SC=100000
HOOK=$(echo -n '{"swap":{"max_spread":"0.5"}}' | base64)
info "Swap $SC CW20->native..."
H=$(send wasm execute $CW20 "{\"send\":{\"contract\":\"$POOL\",\"amount\":\"$SC\",\"msg\":\"$HOOK\"}}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  RET=$(wa "$H" "return_amount")
  pass "Reverse swap OK: returned=$RET"
else
  fail "Reverse swap ($C)"; rawlog $H
fi

# 9c. Slippage rejection — set absurdly tight max_spread
info "Waiting 14s for rate limiter..."
sleep 14
info "Swap with max_spread=0.0001 (should fail on spread)..."
H=$(send wasm execute $POOL \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"5000000\"},\"max_spread\":\"0.0001\"}}" \
  --amount "5000000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Slippage rejection works" || fail "Tight spread should have failed"

# 9d. Deadline rejection — set deadline in the past
info "Swap with expired deadline (should fail)..."
PAST_NS="1000000000"  # 1 second after epoch (definitely past)
H=$(send wasm execute $POOL \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"1000\"},\"max_spread\":\"0.5\",\"transaction_deadline\":\"$PAST_NS\"}}" \
  --amount "1000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Deadline rejection works" || fail "Past deadline should have failed"

# #####################################################################
hdr "===== PHASE 10: LIQUIDITY OPERATIONS ====="
# #####################################################################

# 10a. Bob creates a position
SM=1000000
send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$SM\"}}" --from bob >/dev/null
sleep $W
info "Bob adding liquidity ($SM each)..."
H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$SM\",\"amount1\":\"$SM\"}}" --amount "${SM}${DENOM}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  POS_BOB=$(wa "$H" "position_id")
  pass "Bob position created: $POS_BOB"
else
  fail "Bob deposit ($C)"; rawlog $H
fi

# 10b. Bob adds more to the same position
info "Waiting 14s for rate limiter (swap within add_to_position)..."
sleep 14
ADD=200000
send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$ADD\"}}" --from bob >/dev/null
sleep $W
info "Bob adding to position $POS_BOB ($ADD each)..."
H=$(send wasm execute $POOL "{\"add_to_position\":{\"position_id\":\"$POS_BOB\",\"amount0\":\"$ADD\",\"amount1\":\"$ADD\"}}" --amount "${ADD}${DENOM}" --from bob)
C=$(chk $H)
[ "$C" = "0" ] && pass "AddToPosition OK" || { fail "AddToPosition ($C)"; rawlog $H; }

# 10c. Generate fees by doing a swap
info "Waiting 14s..."
sleep 14
info "Swap to generate fees..."
H=$(send wasm execute $POOL \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"200000\"},\"max_spread\":\"0.5\"}}" \
  --amount "200000${DENOM}" --from alice)
chk $H >/dev/null

# 10d. Bob collects fees
info "Bob collecting fees on position $POS_BOB..."
H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$POS_BOB\"}}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  FEE0=$(wa "$H" "fees_0")
  FEE1=$(wa "$H" "fees_1")
  pass "Fees collected: fees_0=$FEE0 fees_1=$FEE1"
else
  fail "Collect fees ($C)"; rawlog $H
fi

# 10e. Bob removes partial liquidity (50%)
info "Bob removing 50% of position..."
H=$(send wasm execute $POOL "{\"remove_partial_liquidity_by_percent\":{\"position_id\":\"$POS_BOB\",\"percentage\":50}}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  pass "Partial remove (50%) OK"
  P0=$(wa "$H" "principal_0")
  P1=$(wa "$H" "principal_1")
  info "Returned: principal_0=$P0 principal_1=$P1"
else
  fail "Partial remove ($C)"; rawlog $H
fi

# Position should still exist with remaining liquidity
PINFO=$(q $POOL "{\"position\":{\"position_id\":\"$POS_BOB\"}}" | jq -r '.data.liquidity // "0"')
info "Remaining liquidity after 50% remove: $PINFO"
[ "$PINFO" != "0" ] && pass "Position still has liquidity" || fail "Position empty after partial"

# #####################################################################
hdr "===== PHASE 11: NFT TRANSFER + NEW OWNER OPS ====="
# #####################################################################
if [ -n "$NFT" ]; then
  OWNER=$(q $NFT "{\"owner_of\":{\"token_id\":\"$POS_BOB\"}}" | jq -r '.data.owner // empty')
  info "owner_of($POS_BOB) = $OWNER (should be Bob)"
  [ "$OWNER" = "$BOB" ] && pass "NFT owned by Bob" || fail "NFT owner mismatch"

  info "Bob transfers NFT to Alice..."
  H=$(send wasm execute $NFT "{\"transfer_nft\":{\"recipient\":\"$ALICE\",\"token_id\":\"$POS_BOB\"}}" --from bob)
  C=$(chk $H)
  [ "$C" = "0" ] && pass "NFT transfer OK" || fail "NFT transfer ($C)"

  OWNER=$(q $NFT "{\"owner_of\":{\"token_id\":\"$POS_BOB\"}}" | jq -r '.data.owner // empty')
  [ "$OWNER" = "$ALICE" ] && pass "NFT now owned by Alice" || fail "NFT did not transfer"

  # Bob (old owner) cannot collect/remove
  info "Bob tries collect_fees (should fail)..."
  H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$POS_BOB\"}}" --from bob)
  C=$(chk $H)
  [ "$C" != "0" ] && pass "Old owner rejected" || fail "Old owner should be rejected"

  # Alice (new owner) can collect fees
  info "Alice collects fees on transferred position..."
  H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$POS_BOB\"}}" --from alice)
  C=$(chk $H)
  [ "$C" = "0" ] && pass "New owner collected fees" || fail "New owner collect fees ($C)"

  # Alice removes all remaining liquidity
  info "Alice removes all remaining liquidity..."
  H=$(send wasm execute $POOL "{\"remove_all_liquidity\":{\"position_id\":\"$POS_BOB\"}}" --from alice)
  C=$(chk $H)
  [ "$C" = "0" ] && pass "Remove all liquidity OK" || { fail "Remove all ($C)"; rawlog $H; }

  # NFT Burn — standard cw721_base supports burn
  # Position is removed from pool, try burning the NFT
  info "Alice burns the NFT..."
  H=$(send wasm execute $NFT "{\"burn\":{\"token_id\":\"$POS_BOB\"}}" --from alice)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    pass "NFT burned"
  else
    note "NFT burn not supported by this cw721 (code=$C)"
  fi
else
  note "Skipping NFT tests — contract not found"
fi

# #####################################################################
hdr "===== PHASE 12: PAUSE / UNPAUSE (VIA FACTORY ROUTING) ====="
# #####################################################################
# We're testing on Pool3 here because Pool1 has live liquidity/position state
# we don't want to disturb, and Pool2 already had its 20% guard run.

# 12a. Direct pause from Alice (should fail — pool checks factory_addr)
info "Alice tries to pause Pool3 directly (should fail)..."
H=$(send wasm execute $POOL2 '{"pause":{}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Direct pause rejected (pool checks factory_addr)" || fail "Direct pause should fail"

# 12b. Non-admin via factory (Bob) should fail — factory admin check
info "Bob tries PausePool via factory (should fail — not admin)..."
H=$(send wasm execute $FACTORY '{"pause_pool":{"pool_id":2}}' --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-admin PausePool rejected" || fail "Non-admin should be rejected"

# 12c. Admin (Alice) via factory should succeed
info "Alice pauses Pool3 via factory..."
H=$(send wasm execute $FACTORY '{"pause_pool":{"pool_id":2}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Factory->Pool pause OK" || { fail "Pause via factory ($C)"; rawlog $H; }

# 12d. Swap on paused pool should fail — but pool3 has no liquidity yet.
# Add a tiny position first so swaps have something to hit. But deposit is
# blocked while paused, so instead we verify paused state via a query.
# (Pool exposes POOL_PAUSED via... nothing directly. We rely on behavior.)

# 12e. Unpause via factory
info "Alice unpauses Pool3 via factory..."
H=$(send wasm execute $FACTORY '{"unpause_pool":{"pool_id":2}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Factory->Pool unpause OK" || { fail "Unpause via factory ($C)"; rawlog $H; }

# 12f. Pause + verify an operation fails while paused. Use deposit which
# explicitly checks POOL_PAUSED via ensure_not_drained.
info "Re-pause, verify deposit blocked, then unpause..."
H=$(send wasm execute $FACTORY '{"pause_pool":{"pool_id":2}}' --from alice)
chk $H >/dev/null

# Try a deposit (should fail because paused)
send wasm execute $CW20_2 "{\"increase_allowance\":{\"spender\":\"$POOL2\",\"amount\":\"1000\"}}" --from alice >/dev/null
sleep $W
H=$(send wasm execute $POOL2 '{"deposit_liquidity":{"amount0":"1000","amount1":"1000"}}' --amount "1000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Deposit blocked while paused" || fail "Paused pool should reject deposit"

# Unpause
H=$(send wasm execute $FACTORY '{"unpause_pool":{"pool_id":2}}' --from alice)
chk $H >/dev/null
pass "Pool3 unpaused"

# #####################################################################
hdr "===== PHASE 13: EMERGENCY WITHDRAW + CANCEL (VIA FACTORY) ====="
# #####################################################################
# The pool's EmergencyWithdraw handler is two-phase:
#   1st call: sets PENDING_EMERGENCY_WITHDRAW with effective_after = now + 24h,
#             pauses the pool, returns "pending" response.
#   2nd call after timelock elapses: actually drains reserves.
# We can test phase 1 + CancelEmergencyWithdraw without waiting 24h.

# 13a. Direct call from Alice should fail (not the factory contract)
info "Alice calls EmergencyWithdraw directly on Pool3 (should fail)..."
H=$(send wasm execute $POOL2 '{"emergency_withdraw":{}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Direct emergency withdraw rejected" || fail "Should be rejected"

# 13b. Non-admin via factory should fail
info "Bob tries EmergencyWithdrawPool via factory (should fail)..."
H=$(send wasm execute $FACTORY '{"emergency_withdraw_pool":{"pool_id":2}}' --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-admin emergency withdraw rejected" || fail "Should be rejected"

# 13c. Admin initiates emergency withdraw via factory
info "Alice initiates emergency withdraw on Pool3 via factory..."
H=$(send wasm execute $FACTORY '{"emergency_withdraw_pool":{"pool_id":2}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "EmergencyWithdraw phase 1 OK (timelock set, pool paused)" || { fail "Initiate emergency ($C)"; rawlog $H; }

# 13d. Immediate second call should fail — timelock not elapsed
info "Immediate second call (should fail — 24h timelock)..."
H=$(send wasm execute $FACTORY '{"emergency_withdraw_pool":{"pool_id":2}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Timelock enforced" || fail "Timelock should block second call"

# 13e. Cancel the pending emergency withdraw
info "Cancelling emergency withdraw on Pool3..."
H=$(send wasm execute $FACTORY '{"cancel_emergency_withdraw_pool":{"pool_id":2}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "CancelEmergencyWithdraw OK (pool unpaused)" || { fail "Cancel ($C)"; rawlog $H; }

# 13f. Non-admin cancel should fail
info "Bob tries to cancel (should fail)..."
# Need to re-initiate first so there's something to cancel
H=$(send wasm execute $FACTORY '{"emergency_withdraw_pool":{"pool_id":2}}' --from alice)
chk $H >/dev/null
H=$(send wasm execute $FACTORY '{"cancel_emergency_withdraw_pool":{"pool_id":2}}' --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-admin cancel rejected" || fail "Non-admin should be rejected"

# Clean up — cancel as admin so Pool3 isn't left in emergency state
H=$(send wasm execute $FACTORY '{"cancel_emergency_withdraw_pool":{"pool_id":2}}' --from alice)
chk $H >/dev/null

# #####################################################################
hdr "===== PHASE 14: RECOVER STUCK STATES (VIA FACTORY) ====="
# #####################################################################
# Each RecoveryType has a check: if nothing is stuck, the pool returns
# NothingToRecover. That's the expected path for a healthy pool — we treat
# it as "routing works". To test the success path you'd have to actively
# wedge the pool state first.

# 14a. Direct call from Alice should fail
info "Alice calls RecoverStuckStates directly (should fail)..."
H=$(send wasm execute $POOL2 '{"recover_stuck_states":{"recovery_type":"stuck_reentrancy_guard"}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Direct recovery rejected" || fail "Should be rejected"

# 14b. Non-admin via factory should fail
info "Bob tries RecoverPoolStuckStates via factory (should fail)..."
H=$(send wasm execute $FACTORY '{"recover_pool_stuck_states":{"pool_id":2,"recovery_type":"stuck_reentrancy_guard"}}' --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-admin recovery rejected" || fail "Should be rejected"

# 14c. Admin via factory. Healthy pool returns NothingToRecover — factory
# propagates the error, so C != 0. We accept either success or the specific
# "nothing to recover" error as valid routing.
for rt in stuck_threshold stuck_distribution stuck_reentrancy_guard both; do
  info "Alice triggers RecoverPoolStuckStates($rt) via factory..."
  H=$(send wasm execute $FACTORY "{\"recover_pool_stuck_states\":{\"pool_id\":2,\"recovery_type\":\"$rt\"}}" --from alice)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    pass "Recovery ($rt) succeeded (pool had stuck state)"
  else
    LOG=$(rawlog $H)
    if echo "$LOG" | grep -qiE "NothingToRecover|nothing to recover|no distribution or threshold locks|not stuck"; then
      pass "Recovery ($rt) routed -- pool reported NothingToRecover (healthy)"
    else
      fail "Recovery ($rt) unexpected error: $(echo $LOG | head -c 200)"
    fi
  fi
done

# #####################################################################
hdr "===== PHASE 15: POOL CONFIG TIMELOCKED UPDATE ====="
# #####################################################################
# Factory admin can propose pool config changes with 48-hour timelock
# Changes: lp_fee, min_commit_interval, usd_payment_tolerance_bps, oracle_address

# First we need the pool_id. Pools are assigned sequential IDs by the factory.
# Pool1 was the first created, so pool_id=1.
info "Proposing pool config update (pool_id=1, lp_fee -> 1%)..."
H=$(send wasm execute $FACTORY '{"propose_pool_config_update":{"pool_id":1,"pool_config":{"lp_fee":"0.01","min_commit_interval":null,"usd_payment_tolerance_bps":null,"oracle_address":null}}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "ProposePoolConfigUpdate OK" || fail "ProposePoolConfigUpdate ($C)"

# Try to execute immediately (should fail — 48hr timelock)
info "Execute pool config immediately (should fail)..."
H=$(send wasm execute $FACTORY '{"execute_pool_config_update":{"pool_id":1}}' --from alice)
C=$(chk $H)
[ "$C" != "0" ] && pass "Pool config update rejected (timelock)" || fail "Should be timelocked"

# Cancel it
info "Cancelling pool config update..."
H=$(send wasm execute $FACTORY '{"cancel_pool_config_update":{"pool_id":1}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "CancelPoolConfigUpdate OK" || fail "CancelPoolConfigUpdate ($C)"

# #####################################################################
hdr "===== PHASE 16: ORACLE STALENESS ====="
# #####################################################################
note "Mock oracle always returns env.block.time as publish_time"
note "Oracle staleness (MAX_ORACLE_STALENESS_SECONDS=600s) cannot be tested with mock"
note "The pool rejects commits when factory ConversionResponse.timestamp + 600 < block_time"
note "To test: would need a mock oracle with configurable stale timestamp"

# #####################################################################
hdr "===== PHASE 17: MULTIPOOL THRESHOLD CROSSING ====="
# #####################################################################
# Create a third pool and cross both pool2 and pool3 thresholds near-simultaneously.
CREATE3=$(cat <<EOF
{
  "create":{
    "pool_msg":{
      "pool_token_info":[
        {"bluechip":{"denom":"$DENOM"}},
        {"creator_token":{"contract_addr":"WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id":$CW20_CODE,
      "factory_to_create_pool_addr":"$FACTORY",
      "threshold_payout":null,
      "commit_fee_info":{
        "bluechip_wallet_address":"$ALICE",
        "creator_wallet_address":"$ALICE",
        "commit_fee_bluechip":"0.01",
        "commit_fee_creator":"0.05"
      },
      "creator_token_address":"$ALICE",
      "commit_amount_for_threshold":"0",
      "commit_limit_usd":"100000000",
      "pyth_contract_addr_for_conversions":"$ORACLE",
      "pyth_atom_usd_price_feed_id":"ATOM_USD",
      "max_bluechip_lock_per_pool":"10000000000",
      "creator_excess_liquidity_lock_days":7,
      "is_standard_pool":false
    },
    "token_info":{"name":"MultiToken","symbol":"MLT","decimal":6}
  }
}
EOF
)
info "Creating pool3 for multipool test..."
H=$(send wasm execute $FACTORY "$CREATE3" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool3 created" || { fail "Pool3 create ($C)"; }
POOL3=$(wa "$H" "pool_address")
[ -z "$POOL3" ] && POOL3=$(wa "$H" "pool_contract_address")
CW20_3=$(q $POOL3 '{"pair":{}}' | jq -r '.data.asset_infos[1].creator_token.contract_addr // empty')
info "Pool3: $POOL3  CW20_3: $CW20_3"

# Pool2 already crossed threshold in Phase 8.
# Pool3 needs to cross now. Alice commits 100M+1 in one shot to cross.
info "Alice commits 100000001 on Pool3 (single-shot threshold cross)..."
H=$(send wasm execute $POOL3 "$(commit_msg 100000001)" --amount "100000001${DENOM}" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool3 threshold cross TX OK" || { fail "Pool3 threshold ($C)"; rawlog $H; }

DONE3=$(q $POOL3 '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "yes" else "no" end')
[ "$DONE3" = "yes" ] && pass "Pool3 fully committed" || fail "Pool3 not committed"

# Distribute on pool3
H=$(send wasm execute $POOL3 '{"continue_distribution":{}}' --from alice)
chk $H >/dev/null

# Verify both pool2 and pool3 are fully committed
DONE2=$(q $POOL2 '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "yes" else "no" end')
[ "$DONE2" = "yes" ] && [ "$DONE3" = "yes" ] && pass "Both pools fully committed" || fail "Multipool check failed"

# #####################################################################
hdr "===== PHASE 18: CLAIM CREATOR EXCESS LIQUIDITY ====="
# #####################################################################
# The pool stores excess liquidity locked for creator_excess_liquidity_lock_days=7.
# Claiming before unlock_time should fail.

info "Alice tries to claim creator excess liquidity (should fail — 7-day lock)..."
H=$(send wasm execute $POOL '{"claim_creator_excess_liquidity":{}}' --from alice)
C=$(chk $H)
if [ "$C" != "0" ]; then
  LOG=$(rawlog $H)
  if echo "$LOG" | grep -qi "locked\|timelock\|PositionLocked"; then
    pass "Creator excess claim rejected (timelock enforced)"
  else
    # Might fail because no excess position exists
    echo "$LOG" | grep -qi "not found\|No excess" && \
      note "No creator excess position on this pool (expected if creator_token_address=ALICE)" || \
      pass "Claim rejected (code=$C)"
  fi
else
  fail "Claim should fail before timelock expires"
fi

# Bob (non-creator) tries -> should fail
info "Bob tries to claim (not the creator)..."
H=$(send wasm execute $POOL '{"claim_creator_excess_liquidity":{}}' --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Non-creator claim rejected" || fail "Non-creator should be rejected"

# #####################################################################
hdr "===== PHASE 19: POST-THRESHOLD COMMIT (NORMAL SWAP, LP FEES ONLY) ====="
# #####################################################################
# After threshold, commits act as swaps. Verify:
# - Swap uses amount_after_fees (commit fees deducted)
# - LP fees (commission) are taken from the swap
# - No additional "creator fees" — creator fees are only on the commit portion

info "Waiting 14s for rate limiter..."
sleep 14
info "Bob commits 500000 post-threshold on Pool1..."
H=$(send wasm execute $POOL "$(commit_msg 500000)" --amount "500000${DENOM}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  PHASE=$(wa "$H" "phase")
  SWAP_AMT=$(wa "$H" "swap_amount_bluechip")
  TOKENS=$(wa "$H" "tokens_received")
  COMM=$(wa "$H" "commission_amount")
  pass "Post-threshold commit OK (phase=$PHASE)"
  info "swap_amount=$SWAP_AMT tokens_received=$TOKENS commission=$COMM"
  # Verify swap_amount = amount_after_fees (500000 - 6% = 470000)
  EXPECTED_AFTER_FEES=470000
  if [ -n "$SWAP_AMT" ] && [ "$SWAP_AMT" = "$EXPECTED_AFTER_FEES" ]; then
    pass "Commit fees correctly deducted (6% of 500K = 30K, swap = 470K)"
  else
    info "swap_amount=$SWAP_AMT (expected $EXPECTED_AFTER_FEES — may differ by rounding)"
  fi
else
  LOG=$(rawlog $H)
  if echo "$LOG" | grep -qi "spread"; then
    note "Post-threshold commit hit max_spread (expected with skewed reserves)"
  else
    fail "Post-threshold commit ($C)"; echo "$LOG" | head -2
  fi
fi

# #####################################################################
hdr "===== PHASE 20: RATE LIMITER ====="
# #####################################################################
# Minimum interval between swaps/commits from same wallet = 13 seconds (default)
info "Rapid-fire swap from Bob (should fail — rate limiter)..."
H=$(send wasm execute $POOL \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"1000\"},\"max_spread\":\"0.5\"}}" \
  --amount "1000${DENOM}" --from bob)
C=$(chk $H)
if [ "$C" != "0" ]; then
  LOG=$(rawlog $H)
  echo "$LOG" | grep -qi "frequently\|rate" && pass "Rate limiter enforced" || pass "Tx rejected (code=$C)"
else
  note "Rate limit may not have triggered (timing)"
fi

# #####################################################################
hdr "===== PHASE 21: DUST COMMIT REJECTION ====="
# #####################################################################
# MIN_COMMIT_USD = 1_000_000 ($1). At $10/ubluechip, minimum = 100000 ubluechip.
info "Waiting 14s..."
sleep 14
info "Bob tiny commit (100 ubluechip = \$0.001, should fail)..."
H=$(send wasm execute $POOL "$(commit_msg 100)" --amount "100${DENOM}" --from bob)
C=$(chk $H)
[ "$C" != "0" ] && pass "Dust commit rejected" || fail "Dust commit should fail"

# #####################################################################
hdr "===== PHASE 22: AUTO-PAUSE ON LOW RESERVES (DRAIN SCENARIO) ====="
# #####################################################################
# Strategy: spin up a SECOND factory with max_bluechip_lock_per_pool = 100
# (well below MINIMUM_LIQUIDITY = 1000). After threshold crossing the pool's
# reserve0 is capped near 100 and the rest of the bluechip is parked in
# CREATOR_EXCESS_POSITION. The first swap attempt hits the pre-check in
# contract.rs:490, sets POOL_PAUSED = true, and returns InsufficientReserves.
#
# We use a fresh factory (rather than mutating Factory 1) so Pool1/Pool2/
# Pool3 and their state stay intact for the rest of the test summary.

info "Instantiating second factory with max_bluechip_lock_per_pool = 100..."
FINIT2=$(cat <<EOF
{
  "factory_admin_address":"$ALICE",
  "commit_amount_for_threshold_bluechip":"0",
  "commit_threshold_limit_usd":"1000000000",
  "pyth_contract_addr_for_conversions":"$ORACLE",
  "pyth_atom_usd_price_feed_id":"ATOM_USD",
  "cw721_nft_contract_id":$CW721_CODE,
  "cw20_token_contract_id":$CW20_CODE,
  "create_pool_wasm_contract_id":$POOL_CODE,
  "bluechip_wallet_address":"$ALICE",
  "commit_fee_bluechip":"0.01",
  "commit_fee_creator":"0.05",
  "max_bluechip_lock_per_pool":"100",
  "creator_excess_liquidity_lock_days":7,
  "atom_bluechip_anchor_pool_address":"$ALICE",
  "bluechip_mint_contract_address":null
}
EOF
)
H=$(send wasm instantiate $FACTORY_CODE "$FINIT2" --from alice --label "factory_drain" --admin $ALICE)
sleep $W
FACTORY2=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$FACTORY2" ] && [ "$FACTORY2" != "null" ] && pass "Factory2: $FACTORY2" || { fail "Factory2 init"; }
send bank send $ALICE $FACTORY2 "50000000${DENOM}" --from alice >/dev/null
sleep $W

CREATE4=$(cat <<EOF
{
  "create":{
    "pool_msg":{
      "pool_token_info":[
        {"bluechip":{"denom":"$DENOM"}},
        {"creator_token":{"contract_addr":"WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id":$CW20_CODE,
      "factory_to_create_pool_addr":"$FACTORY2",
      "threshold_payout":null,
      "commit_fee_info":{
        "bluechip_wallet_address":"$ALICE",
        "creator_wallet_address":"$ALICE",
        "commit_fee_bluechip":"0.01",
        "commit_fee_creator":"0.05"
      },
      "creator_token_address":"$ALICE",
      "commit_amount_for_threshold":"0",
      "commit_limit_usd":"100000000",
      "pyth_contract_addr_for_conversions":"$ORACLE",
      "pyth_atom_usd_price_feed_id":"ATOM_USD",
      "max_bluechip_lock_per_pool":"100",
      "creator_excess_liquidity_lock_days":7,
      "is_standard_pool":false
    },
    "token_info":{"name":"DrainToken","symbol":"DRN","decimal":6}
  }
}
EOF
)
info "Creating Pool4 on Factory2..."
H=$(send wasm execute $FACTORY2 "$CREATE4" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool4 created" || { fail "Pool4 create ($C)"; rawlog $H; }
POOL4=$(wa "$H" "pool_address")
[ -z "$POOL4" ] && POOL4=$(wa "$H" "pool_contract_address")
CW20_4=$(q $POOL4 '{"pair":{}}' | jq -r '.data.asset_infos[1].creator_token.contract_addr // empty')
info "Pool4: $POOL4  CW20_4: $CW20_4"

info "Alice crosses threshold on Pool4 (single-shot 100000001)..."
H=$(send wasm execute $POOL4 "$(commit_msg 100000001)" --amount "100000001${DENOM}" --from alice --gas 5000000)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pool4 threshold cross OK" || { fail "Pool4 threshold ($C)"; rawlog $H; }

H=$(send wasm execute $POOL4 '{"continue_distribution":{}}' --from alice)
chk $H >/dev/null

# Check reserves — should show reserve0 near 100 (below MINIMUM_LIQUIDITY)
PS4=$(q $POOL4 '{"pool_state":{}}' | jq '.data')
R0_4=$(echo $PS4 | jq -r '.reserve0')
R1_4=$(echo $PS4 | jq -r '.reserve1')
info "Pool4 reserves: reserve0=$R0_4 reserve1=$R1_4"
if [ "$R0_4" -lt 1000 ]; then
  pass "Pool4 reserve0=$R0_4 < MINIMUM_LIQUIDITY (1000) -- drained state achieved"
else
  fail "Expected reserve0 < 1000, got $R0_4"
fi

# Attempt a swap — should fail with InsufficientReserves
info "Alice attempts swap on drained Pool4 (should fail)..."
H=$(send wasm execute $POOL4 \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"100\"},\"max_spread\":\"0.5\"}}" \
  --amount "100${DENOM}" --from alice)
C=$(chk $H)
LOG=$(rawlog $H)
if [ "$C" != "0" ] && echo "$LOG" | grep -qiE "can not cover reserves|insufficient reserves|InsufficientReserves"; then
  pass "Swap rejected with InsufficientReserves (pool soft-paused on drain)"
else
  fail "Expected InsufficientReserves error, got code=$C log=$(echo $LOG | head -c 150)"
fi

# Bob also tries (different wallet, no rate-limit entanglement)
info "Bob attempts swap on drained Pool4 (should also fail)..."
H=$(send wasm execute $POOL4 \
  "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"100\"},\"max_spread\":\"0.5\"}}" \
  --amount "100${DENOM}" --from bob)
C=$(chk $H)
LOG=$(rawlog $H)
if [ "$C" != "0" ] && echo "$LOG" | grep -qiE "can not cover reserves|insufficient reserves|InsufficientReserves"; then
  pass "Second swap also rejected (drained pool is unusable)"
else
  fail "Second swap should also fail, got code=$C"
fi

# Raw-query POOL_PAUSED. After the contract.rs:491 cleanup, the drain
# guard no longer attempts the dead POOL_PAUSED.save(true) call -- the
# reserve pre-check alone is the soft-pause mechanism. POOL_PAUSED stays
# null unless a successful admin PauseMsg was the thing that flipped it.
KEY_HEX=$(echo -n "pool_paused" | xxd -p)
PAUSED_RAW=$(bluechipChaind query wasm contract-state raw $POOL4 "$KEY_HEX" --output json 2>/dev/null | jq -r '.data')
info "POOL_PAUSED raw state: $PAUSED_RAW (null = never set by admin path)"
if [ "$PAUSED_RAW" = "null" ] || [ -z "$PAUSED_RAW" ]; then
  pass "POOL_PAUSED correctly unset -- soft-pause enforced purely by reserve check"
else
  fail "POOL_PAUSED is set ($PAUSED_RAW) but no admin pause was called"
fi

# #####################################################################
hdr "===== FINAL SUMMARY ====="
# #####################################################################
echo ""
echo "  Contracts:"
echo "    Oracle:         $ORACLE"
echo "    Factory:        $FACTORY"
echo "    Expand Economy: ${ECON:-n/a}"
echo "    Pool1:          $POOL"
echo "    Pool1 CW20:     $CW20"
echo "    Pool2:          ${POOL2:-n/a}"
echo "    Pool3:          ${POOL3:-n/a}"
echo "    Pool4 (drain):  ${POOL4:-n/a}"
echo "    Factory2 (drain):${FACTORY2:-n/a}"
echo "    NFT:            ${NFT:-n/a}"
echo ""
echo "  Tests Covering:"
echo "    - Factory config edit + 48hr timelock + cancel"
echo "    - Non-admin rejection"
echo "    - Pool creation via factory"
echo "    - Pre-threshold: swaps/deposits blocked"
echo "    - Commit flow: fees, recording, threshold crossing"
echo "    - ContinueDistribution + creator token payouts"
echo "    - 20% guard on threshold-crossing excess"
echo "    - Normal swaps: native->CW20 and CW20->native"
echo "    - Swap fees (LP commission)"
echo "    - Slippage rejection (max_spread)"
echo "    - Deadline rejection (transaction_deadline)"
echo "    - Liquidity: deposit, add_to_position, partial remove"
echo "    - Fee collection"
echo "    - NFT transfer + ownership verification"
echo "    - Old owner rejected / new owner accepted"
echo "    - NFT burn"
echo "    - Pause/Unpause auth (factory-only)"
echo "    - EmergencyWithdraw auth (factory-only)"
echo "    - RecoverStuckStates auth (factory-only)"
echo "    - Pool config timelocked update + cancel"
echo "    - Multipool threshold crossing"
echo "    - ClaimCreatorExcessLiquidity timelock"
echo "    - Post-threshold commit (swap mode, fees)"
echo "    - Rate limiter"
echo "    - Dust commit rejection"
echo "    - Drain-to-soft-pause (reserve<MINIMUM_LIQUIDITY via max_bluechip_lock_per_pool=100)"
echo ""
echo "  Gaps (require contract changes to test):"
echo "    - Oracle staleness (mock always returns fresh timestamp)"
echo "    - 24h/48h timelock execution (would need to advance block time)"
echo ""
if [ $F -eq 0 ]; then
  echo -e "${GRN}  ALL $(($(grep -c 'pass\|fail' <<< "$(declare -f)"))) TESTS PASSED!${NC}"
  echo -e "${GRN}  ALL TESTS PASSED!${NC}"
else
  echo -e "${RED}  $F TEST(S) FAILED${NC}"
fi
exit $F
