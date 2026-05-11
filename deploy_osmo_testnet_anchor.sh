#!/usr/bin/env bash
# =====================================================================
# osmo-test-5 deploy — Slice 2: anchor pool + SetAnchorPool
# =====================================================================
# Steps:
#   1. factory.CreateStandardPool([BLUECHIP_DENOM, uosmo], <label>)
#      Pays the standard-pool creation fee in BLUECHIP_DENOM
#      (sized over the bootstrap fallback floor; surplus refunded).
#   2. Resolve the new pool's contract address from the create tx's
#      instantiate events (filtered by STANDARD_POOL_CODE_ID).
#   3. anchor_pool.deposit_liquidity to seed the pool's reserves.
#      Both legs are native (BLUECHIP_DENOM and uosmo) so funds are
#      attached directly with no CW20 allowance step.
#   4. factory.SetAnchorPool { pool_id } — the one-shot bootstrap that
#      flips INITIAL_ANCHOR_SET so the factory's USD↔bluechip oracle
#      path now has somewhere to read prices from.
#   5. Append ANCHOR_POOL_ID, ANCHOR_POOL_ADDR, ANCHOR_POSITION_NFT
#      to the slice-1 state file.
#
# AFTER THIS SCRIPT COMPLETES (in this order):
#   a. Start `bash pyth_vaa_pusher.sh` in a separate terminal so the
#      Pyth on-chain price stays under the 90s staleness window. The
#      factory's first oracle update needs fresh Pyth data; until the
#      pusher runs, every UpdateOraclePrice will hit the stale-Pyth
#      branch.
#   b. Run `bash propose_oracle_eligibility.sh` to start the 48h
#      auto-eligible flag-flip timelock (slice 4 will apply it).
#   c. Run the slice-3 tests (test_bootstrap.sh, test_twap_advance.sh,
#      test_staleness.sh) during the 48h wait — they only need the
#      anchor pool, not the eligible-pool set.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

ADDR="$ADMIN_ADDR"
echo "deployer:        $ADDR"
echo "factory:         $FACTORY_ADDR"
echo "bluechip denom:  $BLUECHIP_DENOM"
echo ""

# ---- Sanity: gas + bluechip balance ---------------------------------
# osmosisd v29 routes query JSON to stderr in non-TTY contexts; merge with 2>&1
# so jq sees the response regardless of which stream osmosisd picked.
BAL_JSON="$(osmosisd query bank balances "$ADDR" --node "$NODE" -o json 2>&1 \
    || { echo "error: cannot reach $NODE" >&2; exit 1; })"
GAS_BAL="$(echo "$BAL_JSON" | jq -r --arg d "$NATIVE_DENOM" \
    '.balances[]? | select(.denom == $d) | .amount' || echo 0)"
BLUE_BAL="$(echo "$BAL_JSON" | jq -r --arg d "$BLUECHIP_DENOM" \
    '.balances[]? | select(.denom == $d) | .amount' || echo 0)"
[ -z "$GAS_BAL"  ] && GAS_BAL=0
[ -z "$BLUE_BAL" ] && BLUE_BAL=0

if [ "$GAS_BAL" -lt "$MIN_GAS_BALANCE_ANCHOR" ]; then
    echo "error: $ADDR has only $GAS_BAL u$NATIVE_DENOM, need >= $MIN_GAS_BALANCE_ANCHOR" >&2
    echo "       (anchor seed wants $ANCHOR_POOL_SEED_UOSMO uosmo + gas overhead)" >&2
    exit 1
fi

# Need: fee-fund + per-side seed = STANDARD_POOL_FEE_FUNDS_BLUECHIP + ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE.
NEEDED_BLUE=$(( STANDARD_POOL_FEE_FUNDS_BLUECHIP + ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE ))
if [ "$BLUE_BAL" -lt "$NEEDED_BLUE" ]; then
    echo "error: $ADDR has only $BLUE_BAL of $BLUECHIP_DENOM, need >= $NEEDED_BLUE" >&2
    echo "       (fee fund $STANDARD_POOL_FEE_FUNDS_BLUECHIP + seed $ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE)" >&2
    echo "       — re-run deploy_osmo_testnet.sh to top up the tokenfactory mint" >&2
    exit 1
fi
echo "gas balance:     $GAS_BAL u$NATIVE_DENOM (>= $MIN_GAS_BALANCE_ANCHOR required)"
echo "bluechip bal:    $BLUE_BAL (>= $NEEDED_BLUE required)"
echo ""

# ---- 1. CreateStandardPool ------------------------------------------
ANCHOR_LABEL="bluechip_anchor_uosmo"
echo "[1/4] factory.create_standard_pool $ANCHOR_LABEL"

# Pair: BLUECHIP_DENOM at index 0 (canonical first), uosmo at index 1.
# Both are TokenType::Native, which serde-renames to "bluechip" — so
# both legs serialize as {"bluechip":{"denom":"..."}}. The factory's
# validate_standard_pool_token_info only requires that one leg equals
# the canonical bluechip_denom; both natives is fine.
CREATE_MSG="$(jq -nc \
    --arg blue "$BLUECHIP_DENOM" \
    --arg uosmo "$NATIVE_DENOM" \
    --arg label "$ANCHOR_LABEL" \
    '{create_standard_pool:{
        pool_token_info:[
            {bluechip:{denom:$blue}},
            {bluechip:{denom:$uosmo}}
        ],
        label:$label
    }}')"

CREATE_RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" "$CREATE_MSG" \
    --amount "${STANDARD_POOL_FEE_FUNDS_BLUECHIP}${BLUECHIP_DENOM}")"

ANCHOR_POOL_ID="$(extract_attr "$CREATE_RESULT" wasm pool_id)"
if [ -z "$ANCHOR_POOL_ID" ] || [ "$ANCHOR_POOL_ID" = "null" ]; then
    echo "error: could not extract pool_id from create_standard_pool tx" >&2
    echo "$CREATE_RESULT" | jq '.events[] | select(.type=="wasm")'
    exit 1
fi
echo "      pool_id:        $ANCHOR_POOL_ID"

# Resolve the pool address: filter the tx's instantiate events for the
# one whose code_id matches STANDARD_POOL_CODE_ID. The CW721 NFT
# instantiate also appears in this tx; we ignore it here (the pool's
# pair query exposes the NFT address downstream).
ANCHOR_POOL_ADDR="$(echo "$CREATE_RESULT" | jq -r --arg cid "$STANDARD_POOL_CODE_ID" '
    .events[] | select(.type == "instantiate") |
    (.attributes | from_entries) as $a |
    select($a.code_id == $cid) | $a._contract_address
' | head -n 1)"
if [ -z "$ANCHOR_POOL_ADDR" ] || [ "$ANCHOR_POOL_ADDR" = "null" ]; then
    echo "error: could not resolve anchor pool address (code_id=$STANDARD_POOL_CODE_ID)" >&2
    echo "$CREATE_RESULT" | jq '.events[] | select(.type=="instantiate")'
    exit 1
fi
echo "      pool address:   $ANCHOR_POOL_ADDR"

ANCHOR_POSITION_NFT="$(echo "$CREATE_RESULT" | jq -r --arg cid "$CW721_CODE_ID" '
    .events[] | select(.type == "instantiate") |
    (.attributes | from_entries) as $a |
    select($a.code_id == $cid) | $a._contract_address
' | head -n 1)"
echo "      position NFT:   ${ANCHOR_POSITION_NFT:-(none captured)}"

# ---- 2. Initial DepositLiquidity ------------------------------------
echo ""
echo "[2/4] anchor_pool.deposit_liquidity"
echo "      seeding: $ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE $BLUECHIP_DENOM"
echo "               $ANCHOR_POOL_SEED_UOSMO $NATIVE_DENOM"

# amount0 corresponds to pool_token_info[0] (BLUECHIP_DENOM) and
# amount1 to pool_token_info[1] (uosmo). Both legs are native so
# we attach both as funds in a single tx. Order in --amount is
# arbitrary but the comma-separated coin list must be sorted by denom
# alphabetically by some Cosmos-SDK versions; keep it deterministic.
DEPOSIT_MSG="$(jq -nc \
    --arg a0 "$ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE" \
    --arg a1 "$ANCHOR_POOL_SEED_UOSMO" \
    '{deposit_liquidity:{
        amount0:$a0,
        amount1:$a1,
        min_amount0:null,
        min_amount1:null,
        transaction_deadline:null
    }}')"

# Build the funds string with denoms sorted alphabetically. The
# tokenfactory denom "factory/..." sorts before "uosmo" so we list
# bluechip first.
FUNDS_COMBINED="${ANCHOR_POOL_SEED_BLUECHIP_PER_SIDE}${BLUECHIP_DENOM},${ANCHOR_POOL_SEED_UOSMO}${NATIVE_DENOM}"

DEPOSIT_RESULT="$(submit_tx wasm execute "$ANCHOR_POOL_ADDR" "$DEPOSIT_MSG" \
    --amount "$FUNDS_COMBINED")"
ANCHOR_POSITION_ID="$(extract_attr "$DEPOSIT_RESULT" wasm position_id)"
echo "      position_id:    ${ANCHOR_POSITION_ID:-(not captured)}"

# ---- 3. SetAnchorPool ------------------------------------------------
echo ""
echo "[3/4] factory.set_anchor_pool { pool_id: $ANCHOR_POOL_ID }"

SET_ANCHOR_MSG="$(jq -nc --arg id "$ANCHOR_POOL_ID" '{set_anchor_pool:{pool_id:($id|tonumber)}}')"
SET_ANCHOR_RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" "$SET_ANCHOR_MSG")"
echo "      OK — INITIAL_ANCHOR_SET flipped, factory.atom_bluechip_anchor_pool_address → $ANCHOR_POOL_ADDR"

# Verify by querying factory config.
FACTORY_QUERY_RESP="$(query_smart "$FACTORY_ADDR" '{"factory":{}}')"
VERIFIED_ANCHOR="$(echo "$FACTORY_QUERY_RESP" | jq -r '.factory.atom_bluechip_anchor_pool_address // empty')"
if [ "$VERIFIED_ANCHOR" != "$ANCHOR_POOL_ADDR" ]; then
    echo "warning: factory config still reports anchor as $VERIFIED_ANCHOR (expected $ANCHOR_POOL_ADDR)" >&2
else
    echo "      verified via factory.factory query: anchor = $VERIFIED_ANCHOR"
fi

# ---- 4. Append to state file ----------------------------------------
echo ""
echo "[4/4] persisting state"
cat >> "$SCRIPT_DIR/$STATE_FILE" <<EOF

# Slice 2 — anchor pool
ANCHOR_POOL_ID="$ANCHOR_POOL_ID"
ANCHOR_POOL_ADDR="$ANCHOR_POOL_ADDR"
ANCHOR_POSITION_NFT="$ANCHOR_POSITION_NFT"
ANCHOR_POSITION_ID="$ANCHOR_POSITION_ID"
EOF
echo "      appended to $STATE_FILE"

echo ""
echo "=================================================="
echo "slice 2 complete"
echo "=================================================="
echo "anchor pool_id:  $ANCHOR_POOL_ID"
echo "anchor address:  $ANCHOR_POOL_ADDR"
echo "position NFT:    $ANCHOR_POSITION_NFT"
echo "position id:     $ANCHOR_POSITION_ID"
echo ""
echo "NEXT (do these in order):"
echo "  1. Start the VAA pusher in a separate terminal:"
echo "     bash $SCRIPT_DIR/pyth_vaa_pusher.sh"
echo "  2. Kick off the 48h auto-eligible flag-flip timelock:"
echo "     bash $SCRIPT_DIR/propose_oracle_eligibility.sh"
echo "  3. Run the slice-3 anchor-only tests during the wait:"
echo "     bash $SCRIPT_DIR/test_bootstrap.sh"
echo "     bash $SCRIPT_DIR/test_twap_advance.sh swap 1000000"
echo "     bash $SCRIPT_DIR/test_staleness.sh"
