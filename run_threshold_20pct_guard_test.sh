#!/bin/bash
# =====================================================================
# Threshold-Crossing 20% Guard Test
#
# Verifies the safety guard in process_threshold_crossing_with_excess:
# when a single commit pushes the pool past the threshold AND the
# leftover ("excess") would consume more than 20% of the freshly seeded
# bluechip reserve, the over-cap portion is refunded to the sender via
# BankMsg instead of being swapped.
#
# Flow:
#   1. Set up oracle/factory/pool fresh
#   2. Bob commits 95M ubluechip — just under the $1000 threshold
#   3. Bob commits 80M ubluechip — overshoots by ~80M (≫ 20% of seed)
#   4. Confirm bluechip_excess_refunded > 0
#   5. Confirm Bob's bank balance change ≈ (committed − refund)
# =====================================================================
set -uo pipefail

GAS=3000000
W=7
source "$(dirname "$0")/test_lib.sh"

# Re-bind bluechipChaind so `--home/--node` are always present even when the
# script's helpers forget to pass them (lets us reuse the chain that
# run_full_test.sh leaves running).
shopt -s expand_aliases
bluechipChaind() {
  case "${1:-}" in
    tx|query)
      command bluechipChaind "$@" --node "$NODE" --home "$CHAIN_HOME" ;;
    keys)
      command bluechipChaind "$@" --home "$CHAIN_HOME" ;;
    *)
      command bluechipChaind "$@" ;;
  esac
}
export -f bluechipChaind

# Bridge this script's pass/fail/info/hdr names to the lib's counters,
# so the lib's exit summary prints the right tally.
pass() { log_pass "$1"; }
fail() { log_fail "$1"; F=$((F+1)); }
info() { echo "      $1"; }
hdr()  { log_header "$1"; }
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
bal() {
  bluechipChaind query bank balances "$1" --output json 2>/dev/null \
    | jq -r ".balances[]|select(.denom==\"$DENOM\")|.amount // \"0\""
}
gas_used() {
  bluechipChaind query tx "$1" --output json 2>/dev/null | jq -r '.gas_used // "0"'
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
  local raw=$(bluechipChaind tx wasm store "artifacts/$f" --from alice \
    --chain-id $CHAIN_ID --keyring-backend $KR --gas 5000000 -y --output json 2>&1)
  local h=$(echo "$raw" | grep -v WARNING | jq -r '.txhash // empty' 2>/dev/null)
  if [ -z "$h" ]; then echo "[DEBUG $lbl] raw: $raw" >&2; fi
  sleep 10
  local c=$(codeid "$h")
  if [ -z "$c" ]; then sleep 5; c=$(codeid "$h"); fi
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
H=$(send wasm instantiate $ORACLE_CODE '{}' --from alice --label "oracle_guard_test" --no-admin)
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
H=$(send wasm instantiate $FACTORY_CODE "$FINIT" --from alice --label "factory_guard_test" --admin $ALICE)
sleep $W
FACTORY=$(bluechipChaind query wasm list-contract-by-code $FACTORY_CODE --output json 2>/dev/null | jq -r '.contracts[-1]')
pass "Factory: $FACTORY"
send bank send $ALICE $FACTORY "50000000${DENOM}" --from alice >/dev/null
sleep $W

# =====================================================================
hdr "3. CREATE POOL"
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
    "token_info":{"name":"GuardTest","symbol":"GRD","decimal":6}
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

# =====================================================================
hdr "4. COMMIT JUST UNDER THRESHOLD"
# =====================================================================
# Threshold = $1000 USD = 1,000,000,000 µUSD.
# Price = $10/bluechip → 100,000,000 ubluechip needed total to cross.
# Commit 95M ubluechip ≈ $950 — leaves only ~$50 needed to cross.
NEAR=95000000
info "Alice committing $NEAR (~\$950) — just below threshold..."
H=$(send wasm execute $POOL "$(commit_msg $NEAR)" --amount "${NEAR}${DENOM}" --from alice)
C=$(chk $H)
[ "$C" = "0" ] && pass "Pre-threshold commit OK" || { fail "Pre-threshold commit ($C)"; rawlog $H; exit 1; }

CS=$(q $POOL '{"is_fully_commited":{}}' | jq -r '.data')
info "Pool status: $CS"
RAISED_USD=$(echo $CS | jq -r '.in_progress.raised_usd // .in_progress.raised // "?"')
info "Raised so far: $RAISED_USD"

# =====================================================================
hdr "5. OVERSHOOT — TRIGGER 20% GUARD"
# =====================================================================
# Need only ~5M ubluechip (~\$50) more to cross. Commit 80M instead so the
# excess swap (~75M) is far above 20% of the freshly seeded reserve0.
OVER=80000000
info "Waiting 14s for per-wallet commit rate limiter..."
sleep 14
BOB_BAL_PRE=$(bal $BOB)
info "Bob balance pre-overshoot: $BOB_BAL_PRE"
info "Bob committing $OVER (~\$800) — should overshoot by ~75M..."
H=$(send wasm execute $POOL "$(commit_msg $OVER)" --amount "${OVER}${DENOM}" --from bob)
C=$(chk $H)
if [ "$C" != "0" ]; then
  fail "Overshoot commit ($C)"; rawlog $H; exit 1
fi
pass "Overshoot commit landed"

PHASE=$(wa "$H" "phase")
TOTAL_BC=$(wa "$H" "total_amount_bluechip")
THR_BC=$(wa "$H" "threshold_amount_bluechip")
SWAP_BC=$(wa "$H" "swap_amount_bluechip")
RETURNED=$(wa "$H" "bluechip_excess_returned")
SPREAD=$(wa "$H" "bluechip_excess_spread")
COMMISSION=$(wa "$H" "bluechip_excess_commission")
REFUNDED=$(wa "$H" "bluechip_excess_refunded")
R0=$(wa "$H" "reserve0_after")
R1=$(wa "$H" "reserve1_after")

info "phase                       = $PHASE"
info "total_amount_bluechip       = $TOTAL_BC"
info "threshold_amount_bluechip   = $THR_BC"
info "effective_swap_bluechip     = $SWAP_BC"
info "bluechip_excess_returned    = $RETURNED  (creator tokens received)"
info "bluechip_excess_spread      = $SPREAD"
info "bluechip_excess_commission  = $COMMISSION"
info "bluechip_excess_refunded    = $REFUNDED  ← 20% guard refund"
info "reserve0_after              = $R0"
info "reserve1_after              = $R1"

# Assertions
[ "$PHASE" = "threshold_crossing" ] && pass "Phase = threshold_crossing" || fail "Phase wrong: $PHASE"

if [ -n "$REFUNDED" ] && [ "$REFUNDED" != "0" ] && [ "$REFUNDED" != "" ]; then
  pass "20% guard fired — refunded $REFUNDED ubluechip"
else
  fail "Expected non-zero bluechip_excess_refunded but got: '$REFUNDED'"
fi

# Sanity: capped_excess should be ≤ 20% of reserve0_after
# (reserve0_after = seeded_reserve0 + capped_excess; so seeded ≈ R0 - capped_excess)
# Just print the ratio for visibility.
if [ -n "$SWAP_BC" ] && [ "$SWAP_BC" != "0" ]; then
  python3 -c "print(f'  swap/{int($R0)} = {int($SWAP_BC)/int($R0)*100:.2f}% of reserve0_after')"
fi

BOB_BAL_POST=$(bal $BOB)
info "Bob balance post-overshoot: $BOB_BAL_POST"
SPENT=$((BOB_BAL_PRE - BOB_BAL_POST))
info "Bob bluechip net spent (incl. gas): $SPENT"

# Without refund Bob would have spent $OVER + gas. With refund he should
# have spent ~ ($OVER - $REFUNDED) + gas. Verify spent ≈ OVER - REFUNDED
# within a generous margin (gas + tx fee = single-digit thousands).
EXPECTED_NET=$((OVER - REFUNDED))
DIFF=$((SPENT - EXPECTED_NET))
[ $DIFF -lt 0 ] && DIFF=$((-DIFF))
info "expected net (OVER-REFUNDED) = $EXPECTED_NET, actual spent = $SPENT, diff = $DIFF"
if [ $DIFF -lt 1000000 ]; then
  pass "Refund landed in Bob's bank balance (within gas margin)"
else
  fail "Refund missing — Bob's net spend ($SPENT) does not match $EXPECTED_NET"
fi

# Confirm threshold actually crossed
DONE=$(q $POOL '{"is_fully_commited":{}}' | jq -r 'if .data == "fully_committed" then "fully_committed" else "no" end')
[ "$DONE" = "fully_committed" ] && pass "Pool fully committed" || fail "Pool not fully committed (got $DONE)"

# Bob also got creator tokens from the capped swap portion
BCW=$(q $CW20 "{\"balance\":{\"address\":\"$BOB\"}}" | jq -r '.data.balance // "0"')
info "Bob CW20 after = $BCW"
[ "$BCW" != "0" ] && pass "Bob received creator tokens from capped swap" || info "(Bob CW20 still 0 — distribution may need continue_distribution)"

# =====================================================================
hdr "FINAL SUMMARY"
# =====================================================================
echo ""
echo "  Oracle:   $ORACLE"
echo "  Factory:  $FACTORY"
echo "  Pool:     $POOL"
echo "  CW20:     $CW20"
echo ""
echo "  Threshold-crossing commit attributes:"
echo "    total committed   = $TOTAL_BC ubluechip"
echo "    used to threshold = $THR_BC ubluechip"
echo "    capped swap       = $SWAP_BC ubluechip"
echo "    refunded (20%)    = $REFUNDED ubluechip"
echo ""
if [ $F -eq 0 ]; then
  echo -e "${GRN}  ALL TESTS PASSED!${NC}"
else
  echo -e "${RED}  $F TEST(S) FAILED${NC}"
fi
exit $F
