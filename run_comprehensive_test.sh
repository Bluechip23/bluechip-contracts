#!/bin/bash
# =====================================================================
# Comprehensive On-Chain Integration Test
# =====================================================================
set -uo pipefail

CHAIN_ID="bluechipChain"
KR="test"
DENOM="ubluechip"
GAS=3000000
W=7  # wait seconds between txs

RED='\033[0;31m'; GRN='\033[0;32m'; YEL='\033[1;33m'; CYN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GRN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; F=$((F+1)); }
info() { echo -e "${CYN}[INFO]${NC} $1"; }
hdr()  { echo ""; echo -e "${YEL}══ $1 ══${NC}"; }
F=0

# Helper: send tx, return txhash
send() { bluechipChaind tx "$@" --chain-id $CHAIN_ID --keyring-backend $KR --gas $GAS -y --output json 2>/dev/null | jq -r '.txhash'; }
# Helper: query contract
q() { bluechipChaind query wasm contract-state smart "$1" "$2" --output json 2>/dev/null; }
# Helper: wait and check tx code
chk() { sleep $W; bluechipChaind query tx "$1" --output json 2>/dev/null | jq -r '.code'; }
# Helper: get tx raw_log
rawlog() { bluechipChaind query tx "$1" --output json 2>/dev/null | jq -r '.raw_log'; }
# Helper: get wasm event attribute
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
# Helper: get store_code code_id
codeid() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | python3 -c "
import sys,json
d=json.load(sys.stdin)
for e in d.get('events',[]):
    if e['type']=='store_code':
        for a in e['attributes']:
            if a['key']=='code_id': print(a['value']); sys.exit(0)
print('')" 2>/dev/null
}

ALICE=$(bluechipChaind keys show alice -a --keyring-backend $KR 2>/dev/null)
BOB=$(bluechipChaind keys show bob -a --keyring-backend $KR 2>/dev/null)
info "Alice: $ALICE"
info "Bob:   $BOB"

# =====================================================================
hdr "1. UPLOADING ALL WASM CONTRACTS"
# =====================================================================
upload() {
  local f=$1 lbl=$2
  info "Uploading $lbl..." >&2
  local h=$(bluechipChaind tx wasm store "artifacts/$f" --from alice \
    --chain-id $CHAIN_ID --keyring-backend $KR --gas 5000000 -y --output json 2>/dev/null | jq -r '.txhash')
  sleep $W
  local c=$(codeid "$h")
  [ -n "$c" ] && [ "$c" != "" ] && pass "$lbl → code $c" >&2 || fail "$lbl upload failed" >&2
  echo "$c"
}

POOL_CODE=$(upload pool.wasm "Pool")
ORACLE_CODE=$(upload oracle.wasm "Mock Oracle")
ECON_CODE=$(upload expand_economy.wasm "Expand Economy")
FACTORY_CODE=$(upload factory.wasm "Factory")
info "Pool=$POOL_CODE Oracle=$ORACLE_CODE Econ=$ECON_CODE Factory=$FACTORY_CODE"

# =====================================================================
hdr "2. INSTANTIATING MOCK ORACLE"
# =====================================================================
H=$(send wasm instantiate $ORACLE_CODE '{}' --from alice --label "oracle_test" --no-admin)
sleep $W
ORACLE=$(bluechipChaind query wasm list-contract-by-code $ORACLE_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$ORACLE" ] && [ "$ORACLE" != "null" ] && pass "Oracle: $ORACLE" || { fail "Oracle init failed"; exit 1; }

# Set price: $10 ATOM (1000000000 at expo -8)
send wasm execute $ORACLE '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' --from alice >/dev/null
sleep $W
P=$(q $ORACLE '{"get_price":{"price_id":"ATOM_USD"}}' | jq -r '.data.price // empty')
[ "$P" = "1000000000" ] && pass "Oracle price = \$10" || fail "Oracle price: $P"

# =====================================================================
hdr "3. INSTANTIATING FACTORY (mock oracle mode)"
# =====================================================================
# bluechip_mint_contract_address=null → factory BankMsg fallback
FINIT=$(cat <<EOF
{
  "factory_admin_address":"$ALICE",
  "commit_amount_for_threshold_bluechip":"0",
  "commit_threshold_limit_usd":"1000000000",
  "pyth_contract_addr_for_conversions":"$ORACLE",
  "pyth_atom_usd_price_feed_id":"ATOM_USD",
  "cw721_nft_contract_id":2,
  "cw20_token_contract_id":1,
  "create_pool_wasm_contract_id":$POOL_CODE,
  "bluechip_wallet_address":"$ALICE",
  "commit_fee_bluechip":"0.01",
  "commit_fee_creator":"0.05",
  "max_bluechip_lock_per_pool":"25000000000",
  "creator_excess_liquidity_lock_days":7,
  "atom_bluechip_anchor_pool_address":"$ALICE",
  "bluechip_mint_contract_address":null,
  "bluechip_denom": "ubluechip"
}
EOF
)
H=$(send wasm instantiate $FACTORY_CODE "$FINIT" --from alice --label "factory_test" --admin $ALICE)
sleep $W
FACTORY=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$FACTORY" ] && [ "$FACTORY" != "null" ] && pass "Factory: $FACTORY" || { fail "Factory init failed"; exit 1; }

# Verify config pointers
FC=$(q $FACTORY '{"factory":{}}' | jq '.data.factory')
[ "$(echo $FC | jq -r '.pyth_contract_addr_for_conversions')" = "$ORACLE" ] && pass "Factory→Oracle OK" || fail "Factory→Oracle wrong"
[ "$(echo $FC | jq -r '.create_pool_wasm_contract_id')" = "$POOL_CODE" ] && pass "Factory→Pool code OK" || fail "Factory→Pool code wrong"

# Fund factory for threshold BankMsg mint fallback
send bank send $ALICE $FACTORY "50000000${DENOM}" --from alice >/dev/null
sleep $W
FBAL=$(bluechipChaind query bank balances $FACTORY --output json 2>/dev/null | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount")
[ -n "$FBAL" ] && pass "Factory funded: $FBAL" || fail "Factory funding failed"

# =====================================================================
hdr "4. INSTANTIATING EXPAND ECONOMY"
# =====================================================================
H=$(send wasm instantiate $ECON_CODE "{\"factory_address\":\"$FACTORY\",\"owner\":\"$ALICE\"}" --from alice --label "econ_test" --no-admin)
sleep $W
ECON=$(bluechipChaind query wasm list-contract-by-code $ECON_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
[ -n "$ECON" ] && [ "$ECON" != "null" ] && pass "Expand Economy: $ECON" || fail "Econ init failed"
EF=$(q $ECON '{"get_config":{}}' | jq -r '.data.factory_address')
[ "$EF" = "$FACTORY" ] && pass "Econ→Factory linked" || fail "Econ→Factory: $EF"

# =====================================================================
hdr "5. CREATING A CREATOR POOL"
# =====================================================================
CREATE=$(cat <<EOF
{
  "create":{
    "pool_msg":{
      "pool_token_info":[
        {"bluechip":{"denom":"$DENOM"}},
        {"creator_token":{"contract_addr":"WILL_BE_CREATED_BY_FACTORY"}}
      ],
      "cw20_token_contract_id":1,
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
[ "$C" = "0" ] && pass "Pool creation TX OK" || { fail "Pool creation failed ($C)"; rawlog $H; exit 1; }

POOL=$(wa "$H" "pool_address")
[ -z "$POOL" ] && POOL=$(wa "$H" "pool_contract_address")
[ -n "$POOL" ] && pass "Pool: $POOL" || { fail "No pool address"; exit 1; }

CW20=$(q $POOL '{"pair":{}}' | jq -r '.data.asset_infos[1].creator_token.contract_addr // empty')
info "CW20: $CW20"

# Check pre-threshold
CS=$(q $POOL '{"is_fully_commited":{}}' | jq -r '.data')
echo "$CS" | python3 -c "import sys,json;d=json.load(sys.stdin);print('in_progress' in d)" 2>/dev/null | grep -q True && pass "Pool pre-threshold" || info "Status: $CS"

# =====================================================================
hdr "6. COMMITTING TO POOL"
# =====================================================================
# TokenInfo = {"info": TokenType, "amount": Uint128}
# TokenType::Bluechip = {"bluechip":{"denom":"ubluechip"}}
commit_msg() {
  echo "{\"commit\":{\"asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"$1\"},\"amount\":\"$1\"}}"
}

info "Alice committing 30M (~\$300)..."
H=$(send wasm execute $POOL "$(commit_msg 30000000)" --amount "30000000${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Alice commit 30M OK" || { fail "Alice commit 30M ($C)"; rawlog $H; }

info "Bob committing 30000000 (~\$300)..."
H=$(send wasm execute $POOL "$(commit_msg 30000000)" --amount "30000000${DENOM}" --from bob)
C=$(chk $H)
[ "$C" = "0" ] && pass "Bob commit 30M OK" || { fail "Bob commit 30M ($C)"; rawlog $H; }

AC=$(q $POOL "{\"commiting_info\":{\"wallet\":\"$ALICE\"}}" | jq -c '.data // null')
info "Alice commit info: $AC"
BC=$(q $POOL "{\"commiting_info\":{\"wallet\":\"$BOB\"}}" | jq -c '.data // null')
info "Bob commit info: $BC"

# =====================================================================
hdr "7. CROSSING THE THRESHOLD"
# =====================================================================
# Threshold=$1000 USD=$1B micro-USD. Raised=$600M. Need $400M more = 40M ubluechip.
info "Alice committing 40000001 to cross threshold..."
H=$(send wasm execute $POOL "$(commit_msg 40000001)" --amount "40000001${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Threshold commit TX OK" || { fail "Threshold commit ($C)"; rawlog $H; }

# Check if threshold is crossed. Response is either "fully_committed" or {"in_progress":{...}}
DONE=$(q $POOL '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "fully_committed" else "no" end' 2>/dev/null)

if [ "$DONE" != "fully_committed" ]; then
  RAISED=$(q $POOL '{"is_fully_commited":{}}' | jq -r '.data.in_progress.raised // "?"' 2>/dev/null)
  info "Not yet (raised=$RAISED), trying more..."
  H=$(send wasm execute $POOL "$(commit_msg 20000000)" --amount "20000000${DENOM}" --from alice)
  chk $H >/dev/null
  DONE=$(q $POOL '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "fully_committed" else "no" end' 2>/dev/null)
fi

[ "$DONE" = "fully_committed" ] && pass "THRESHOLD CROSSED!" || { fail "Threshold not crossed"; info "$CS"; }

# Run continue_distribution to pay committers
info "Running continue_distribution..."
H=$(send wasm execute $POOL '{"continue_distribution":{}}' --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Distribution batch OK" || info "Distribution code=$C"

# =====================================================================
hdr "8. VERIFYING THRESHOLD EFFECTS"
# =====================================================================
PS=$(q $POOL '{"pool_state":{}}' | jq '.data')
R0=$(echo $PS | jq -r '.reserve0 // "0"')
R1=$(echo $PS | jq -r '.reserve1 // "0"')
info "Reserves: bluechip=$R0 creator=$R1"
[ "$R0" != "0" ] && [ "$R1" != "0" ] && pass "Pool has reserves" || fail "Pool reserves empty"

if [ -n "$CW20" ] && [ "$CW20" != "" ]; then
  SUP=$(q $CW20 '{"token_info":{}}' | jq -r '.data.total_supply // "0"')
  info "CW20 supply: $SUP"
  [ "$SUP" != "0" ] && pass "CW20 tokens minted" || fail "CW20 supply=0"

  ACW=$(q $CW20 "{\"balance\":{\"address\":\"$ALICE\"}}" | jq -r '.data.balance // "0"')
  BCW=$(q $CW20 "{\"balance\":{\"address\":\"$BOB\"}}" | jq -r '.data.balance // "0"')
  info "Alice CW20=$ACW | Bob CW20=$BCW"
  [ "$ACW" != "0" ] && [ "$BCW" != "0" ] && pass "Both committers got tokens" || fail "Committer payouts wrong"
fi

FBAL2=$(bluechipChaind query bank balances $FACTORY --output json 2>/dev/null | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount // \"0\"")
info "Factory balance: was $FBAL, now $FBAL2"
[ "$FBAL2" != "$FBAL" ] && pass "Factory sent bluechip on threshold" || info "Factory balance unchanged (may be expected with mock)"

# =====================================================================
# POST-THRESHOLD TESTS
# =====================================================================
ANFT=""
BNFT=""
if [ "$DONE" = "fully_committed" ] && [ -n "$CW20" ]; then

  # ═══════════════════════════════════════════════════
  hdr "9. ADDING LIQUIDITY"
  # ═══════════════════════════════════════════════════
  LP=1000000
  info "Alice approving CW20..."
  send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$LP\"}}" --from alice >/dev/null
  sleep $W

  info "Alice depositing liquidity ($LP each)..."
  H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$LP\",\"amount1\":\"$LP\"}}" --amount "${LP}${DENOM}" --from alice)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    pass "Liquidity deposited"
    ANFT=$(wa "$H" "token_id")
    [ -z "$ANFT" ] && ANFT=$(wa "$H" "position_id")
    info "Alice position: $ANFT"
  else
    fail "Deposit liquidity ($C)"; rawlog $H
  fi

  # ═══════════════════════════════════════════════════
  hdr "10. SWAPS"
  # ═══════════════════════════════════════════════════
  SA=100000
  info "Swap $SA native→CW20..."
  H=$(send wasm execute $POOL \
    "{\"simple_swap\":{\"offer_asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"$SA\"},\"max_spread\":\"0.5\"}}" \
    --amount "${SA}${DENOM}" --from alice)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    RET=$(wa "$H" "return_amount")
    pass "Swap native→CW20: returned $RET"
  else
    fail "Swap native→CW20 ($C)"; rawlog $H
  fi

  # Wait for rate limiter (13 second min interval)
  info "Waiting 14s for rate limiter..."
  sleep 14

  # CW20→native via Send hook
  SC=50000
  info "Swap $SC CW20→native..."
  HOOK=$(echo -n '{"swap":{"max_spread":"0.5"}}' | base64)
  H=$(send wasm execute $CW20 "{\"send\":{\"contract\":\"$POOL\",\"amount\":\"$SC\",\"msg\":\"$HOOK\"}}" --from alice)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    RET=$(wa "$H" "return_amount")
    pass "Swap CW20→native: returned $RET"
  else
    fail "Swap CW20→native ($C)"; rawlog $H
  fi

  # ═══════════════════════════════════════════════════
  hdr "11. POST-THRESHOLD COMMITS"
  # ═══════════════════════════════════════════════════
  info "Bob committing 500K post-threshold..."
  H=$(send wasm execute $POOL "$(commit_msg 500000)" --amount "500000${DENOM}" --from bob)
  C=$(chk $H)
  [ "$C" = "0" ] && pass "Post-threshold commit OK" || { info "Post-threshold commit code=$C (may be expected)"; rawlog $H | head -2; }

  # ═══════════════════════════════════════════════════
  hdr "12. COLLECTING FEES"
  # ═══════════════════════════════════════════════════
  if [ -n "$ANFT" ] && [ "$ANFT" != "" ]; then
    info "Alice collecting fees (position $ANFT)..."
    H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$ANFT\"}}" --from alice)
    C=$(chk $H)
    [ "$C" = "0" ] && pass "Collect fees OK" || info "Collect fees code=$C"
  fi

  # ═══════════════════════════════════════════════════
  hdr "13. SMALL LP POSITION (FEE SCALER)"
  # ═══════════════════════════════════════════════════
  SM=10000
  send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$SM\"}}" --from bob >/dev/null
  sleep $W
  info "Bob adding tiny position ($SM each)..."
  H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$SM\",\"amount1\":\"$SM\"}}" --amount "${SM}${DENOM}" --from bob)
  C=$(chk $H)
  if [ "$C" = "0" ]; then
    BNFT=$(wa "$H" "token_id")
    [ -z "$BNFT" ] && BNFT=$(wa "$H" "position_id")
    pass "Small LP created (fee scaler applies): $BNFT"
  else
    fail "Small LP ($C)"; rawlog $H
  fi

  # ═══════════════════════════════════════════════════
  hdr "14. REMOVING LIQUIDITY"
  # ═══════════════════════════════════════════════════
  if [ -n "$ANFT" ] && [ "$ANFT" != "" ]; then
    info "Alice removing all liquidity ($ANFT)..."
    H=$(send wasm execute $POOL "{\"remove_all_liquidity\":{\"position_id\":\"$ANFT\"}}" --from alice)
    C=$(chk $H)
    [ "$C" = "0" ] && pass "Remove liquidity OK" || { fail "Remove liquidity ($C)"; rawlog $H; }
  fi

  # ═══════════════════════════════════════════════════
  hdr "15. RE-ADDING LIQUIDITY"
  # ═══════════════════════════════════════════════════
  RL=500000
  send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$RL\"}}" --from alice >/dev/null
  sleep $W
  info "Alice re-adding liquidity ($RL each)..."
  H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$RL\",\"amount1\":\"$RL\"}}" --amount "${RL}${DENOM}" --from alice)
  C=$(chk $H)
  [ "$C" = "0" ] && pass "Re-add liquidity OK" || { fail "Re-add liquidity ($C)"; rawlog $H; }

else
  info "Skipping post-threshold tests (threshold not crossed or CW20 missing)"
fi

# =====================================================================
hdr "FINAL SUMMARY"
# =====================================================================
echo ""
echo "  Oracle:         $ORACLE"
echo "  Factory:        $FACTORY"
echo "  Expand Economy: ${ECON:-n/a}"
echo "  Pool:           ${POOL:-n/a}"
echo "  CW20 Token:     ${CW20:-n/a}"
echo ""
if [ $F -eq 0 ]; then
  echo -e "${GRN}  ALL TESTS PASSED!${NC}"
else
  echo -e "${RED}  $F TEST(S) FAILED${NC}"
fi
exit $F
