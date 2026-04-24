#!/bin/bash
# =====================================================================
# NFT Position Transfer Integration Test
#
# Tests:
#   1. Add a (large) liquidity position → Alice gets position NFT
#   2. Transfer the position NFT from Alice to Bob via cw721
#   3. Verify owner_of() now reports Bob
#   4. Bob collects fees on the position
#   5. Bob removes all liquidity from the position
# =====================================================================
set -uo pipefail

CHAIN_ID="bluechipChain"
KR="test"
DENOM="ubluechip"
GAS=3000000
W=7

RED='\033[0;31m'; GRN='\033[0;32m'; YEL='\033[1;33m'; CYN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GRN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; F=$((F+1)); }
info() { echo -e "${CYN}[INFO]${NC} $1"; }
hdr()  { echo ""; echo -e "${YEL}══ $1 ══${NC}"; }
F=0

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
# Get every distinct _contract_address from a tx (so we can find the NFT contract)
all_contracts() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | python3 -c "
import sys,json
d=json.load(sys.stdin)
seen=[]
for e in d.get('events',[]):
    if e['type']=='wasm':
        for a in e['attributes']:
            if a['key']=='_contract_address' and a['value'] not in seen:
                seen.append(a['value'])
                print(a['value'])" 2>/dev/null
}
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
hdr "1. UPLOAD CONTRACTS"
# =====================================================================
upload() {
  local f=$1 lbl=$2
  info "Uploading $lbl..." >&2
  local h=$(bluechipChaind tx wasm store "artifacts/$f" --from alice \
    --chain-id $CHAIN_ID --keyring-backend $KR --gas 5000000 -y --output json 2>/dev/null | jq -r '.txhash')
  sleep $W
  local c=$(codeid "$h")
  [ -n "$c" ] && pass "$lbl → code $c" >&2 || fail "$lbl upload failed" >&2
  echo "$c"
}
POOL_CODE=$(upload creator_pool.wasm "Creator Pool")
STANDARD_POOL_CODE=$(upload standard_pool.wasm "Standard Pool")
ORACLE_CODE=$(upload oracle.wasm "Mock Oracle")
FACTORY_CODE=$(upload factory.wasm "Factory")
info "Pool=$POOL_CODE Oracle=$ORACLE_CODE Factory=$FACTORY_CODE"

# =====================================================================
hdr "2. INSTANTIATE ORACLE + FACTORY"
# =====================================================================
H=$(send wasm instantiate $ORACLE_CODE '{}' --from alice --label "oracle_nft_test" --no-admin)
sleep $W
ORACLE=$(bluechipChaind query wasm list-contract-by-code $ORACLE_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
pass "Oracle: $ORACLE"
send wasm execute $ORACLE '{"set_price":{"price_id":"ATOM_USD","price":"1000000000"}}' --from alice >/dev/null
sleep $W

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
  "standard_pool_wasm_contract_id":$STANDARD_POOL_CODE,
  "bluechip_wallet_address":"$ALICE",
  "commit_fee_bluechip":"0.01",
  "commit_fee_creator":"0.05",
  "max_bluechip_lock_per_pool":"25000000000",
  "creator_excess_liquidity_lock_days":7,
  "atom_bluechip_anchor_pool_address":"$ALICE",
  "bluechip_mint_contract_address":null,
  "bluechip_denom": "ubluechip",
  "standard_pool_creation_fee_usd": "1000000"
}
EOF
)
H=$(send wasm instantiate $FACTORY_CODE "$FINIT" --from alice --label "factory_nft_test" --admin $ALICE)
sleep $W
FACTORY=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
pass "Factory: $FACTORY"
send bank send $ALICE $FACTORY "50000000${DENOM}" --from alice >/dev/null
sleep $W

# =====================================================================
hdr "3. CREATE POOL & CROSS THRESHOLD"
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
    "token_info":{"name":"NftTestToken","symbol":"NTT","decimal":6}
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

commit_msg() {
  echo "{\"commit\":{\"asset\":{\"info\":{\"bluechip\":{\"denom\":\"$DENOM\"}},\"amount\":\"$1\"},\"amount\":\"$1\"}}"
}

info "Alice + Bob commit to cross threshold..."
H=$(send wasm execute $POOL "$(commit_msg 30000000)" --amount "30000000${DENOM}" --from alice); chk $H >/dev/null
H=$(send wasm execute $POOL "$(commit_msg 30000000)" --amount "30000000${DENOM}" --from bob);   chk $H >/dev/null
H=$(send wasm execute $POOL "$(commit_msg 40000001)" --amount "40000001${DENOM}" --from alice); chk $H >/dev/null
DONE=$(q $POOL '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "fully_committed" else "no" end')
[ "$DONE" = "fully_committed" ] && pass "Threshold crossed" || { fail "Threshold not crossed"; exit 1; }
H=$(send wasm execute $POOL '{"continue_distribution":{}}' --from alice); chk $H >/dev/null

PS=$(q $POOL '{"pool_state":{}}' | jq '.data')
R0=$(echo $PS | jq -r '.reserve0'); R1=$(echo $PS | jq -r '.reserve1')
info "Reserves: bluechip=$R0 creator=$R1"

# =====================================================================
hdr "4. ALICE ADDS LARGE LIQUIDITY POSITION"
# =====================================================================
# Use big numbers — pool will pin to existing reserve ratio and refund the rest.
LP0=20000000      # 20M ubluechip
LP1=200000000000  # 200B creator units (Alice has ~700B)

info "Alice approving CW20 ($LP1)..."
send wasm execute $CW20 "{\"increase_allowance\":{\"spender\":\"$POOL\",\"amount\":\"$LP1\"}}" --from alice >/dev/null
sleep $W

info "Alice depositing liquidity ($LP0 bluechip / $LP1 creator)..."
H=$(send wasm execute $POOL "{\"deposit_liquidity\":{\"amount0\":\"$LP0\",\"amount1\":\"$LP1\"}}" --amount "${LP0}${DENOM}" --from alice)
C=$(chk $H)
if [ "$C" = "0" ]; then
  pass "Liquidity deposited"
  POSID=$(wa "$H" "position_id")
  info "Position id: $POSID"
  info "Liquidity minted: $(wa $H liquidity)"
  info "Used: amount0=$(wa $H actual_amount0) amount1=$(wa $H actual_amount1) refunded0=$(wa $H refunded_amount0)"
else
  fail "Deposit ($C)"; rawlog $H; exit 1
fi

# =====================================================================
hdr "5. RESOLVE NFT CONTRACT ADDRESS"
# =====================================================================
# The deposit tx touches: Pool, CW20 (TransferFrom), and the cw721 (Mint).
# Pool and CW20 are known — the leftover address is the position NFT contract.
NFT=""
for addr in $(all_contracts $H); do
  if [ "$addr" != "$POOL" ] && [ "$addr" != "$CW20" ]; then
    NFT="$addr"
    break
  fi
done
[ -n "$NFT" ] && pass "Position NFT contract: $NFT" || { fail "Could not resolve NFT contract"; exit 1; }

# Sanity-check: query owner_of from cw721 — should be Alice
OWNER=$(q $NFT "{\"owner_of\":{\"token_id\":\"$POSID\"}}" | jq -r '.data.owner // empty')
info "owner_of($POSID) = $OWNER"
[ "$OWNER" = "$ALICE" ] && pass "NFT owned by Alice" || fail "NFT owner mismatch (expected Alice)"

# =====================================================================
hdr "6. ALICE TRANSFERS POSITION NFT → BOB"
# =====================================================================
TRANSFER="{\"transfer_nft\":{\"recipient\":\"$BOB\",\"token_id\":\"$POSID\"}}"
H=$(send wasm execute $NFT "$TRANSFER" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "transfer_nft tx OK" || { fail "transfer_nft ($C)"; rawlog $H; exit 1; }

OWNER=$(q $NFT "{\"owner_of\":{\"token_id\":\"$POSID\"}}" | jq -r '.data.owner // empty')
info "owner_of($POSID) = $OWNER"
[ "$OWNER" = "$BOB" ] && pass "NFT now owned by Bob" || fail "NFT did not land at Bob"

# =====================================================================
hdr "7. ALICE (OLD OWNER) CANNOT COLLECT FEES"
# =====================================================================
H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$POSID\"}}" --from alice)
C=$(chk $H)
if [ "$C" != "0" ]; then
  pass "Alice correctly rejected (code=$C)"
else
  fail "Alice should not be able to collect fees after transfer"
fi

# =====================================================================
hdr "8. BOB COLLECTS FEES ON TRANSFERRED POSITION"
# =====================================================================
H=$(send wasm execute $POOL "{\"collect_fees\":{\"position_id\":\"$POSID\"}}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  pass "Bob collected fees"
  info "fees_0=$(wa $H fees_0) fees_1=$(wa $H fees_1)"
else
  fail "Bob collect_fees ($C)"; rawlog $H
fi

# =====================================================================
hdr "9. BOB REMOVES ALL LIQUIDITY FROM POSITION"
# =====================================================================
BOB_BC_BEFORE=$(bluechipChaind query bank balances $BOB --output json 2>/dev/null | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount")
BOB_CW_BEFORE=$(q $CW20 "{\"balance\":{\"address\":\"$BOB\"}}" | jq -r '.data.balance // "0"')
info "Bob before remove: bluechip=$BOB_BC_BEFORE  cw20=$BOB_CW_BEFORE"

H=$(send wasm execute $POOL "{\"remove_all_liquidity\":{\"position_id\":\"$POSID\"}}" --from bob)
C=$(chk $H)
if [ "$C" = "0" ]; then
  pass "Remove liquidity OK"
  info "principal_0=$(wa $H principal_0) principal_1=$(wa $H principal_1) fees_0=$(wa $H fees_0) fees_1=$(wa $H fees_1)"
else
  fail "Remove liquidity ($C)"; rawlog $H
fi

BOB_BC_AFTER=$(bluechipChaind query bank balances $BOB --output json 2>/dev/null | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount")
BOB_CW_AFTER=$(q $CW20 "{\"balance\":{\"address\":\"$BOB\"}}" | jq -r '.data.balance // "0"')
info "Bob after remove:  bluechip=$BOB_BC_AFTER  cw20=$BOB_CW_AFTER"
[ "$BOB_CW_AFTER" -gt "$BOB_CW_BEFORE" ] && pass "Bob received CW20 from removed liquidity" || fail "Bob CW20 did not increase"

# Position should now be gone from the pool
POS_AFTER=$(q $POOL "{\"position\":{\"position_id\":\"$POSID\"}}" 2>&1 || true)
echo "$POS_AFTER" | grep -qiE "not found|does not exist|error" && pass "Position cleaned up" || info "Position query: $POS_AFTER"

# =====================================================================
hdr "FINAL SUMMARY"
# =====================================================================
echo ""
echo "  Oracle:   $ORACLE"
echo "  Factory:  $FACTORY"
echo "  Pool:     $POOL"
echo "  CW20:     $CW20"
echo "  NFT:      $NFT"
echo "  Position: $POSID  (Alice → Bob)"
echo ""
if [ $F -eq 0 ]; then
  echo -e "${GRN}  ALL TESTS PASSED!${NC}"
else
  echo -e "${RED}  $F TEST(S) FAILED${NC}"
fi
exit $F
