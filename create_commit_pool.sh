#!/usr/bin/env bash
# =====================================================================
# create_commit_pool.sh — spin up a commit (creator) pool via factory
# =====================================================================
# usage: bash create_commit_pool.sh <name> <symbol>
#
#   <name>   3–50 printable ASCII chars  (e.g. "Alpha Creator")
#   <symbol> 3–12 chars A-Z + 0-9, must contain at least one letter
#            (e.g. "ALPHA")
#
# Sends factory.Create with the post-cleanup CreatePool shape — only
# pool_token_info + token_info; everything else (commit threshold,
# fees, threshold-payout amounts, lock caps, oracle config) is read
# from the factory's stored config and silently overwrites any
# caller-supplied value.
#
# Pays the per-pool creation fee in BLUECHIP_DENOM. Currently sized at
# STANDARD_POOL_FEE_FUNDS_BLUECHIP (200 BC, 2x the bootstrap fallback)
# — same value used for the standard-pool create in slice 2.
#
# Per-address rate limit (state.rs:COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS
# = 3600s): the same address can only create one commit pool per hour.
# To create multiple in close succession, fund a second key, switch
# FROM in osmo_testnet.env, and re-run.
#
# Side effects:
#   - Appends one line per created pool to commit_pools.txt:
#       <pool_id>\t<pool_addr>\t<creator_token_addr>\t<symbol>
#     This file is the source of truth for downstream slices that need
#     to iterate over the test commit pools (cross_threshold.sh,
#     test_rotation.sh, test_swing.sh).
#   - Prints the addresses to stdout so the operator can paste them
#     into the next command.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

NAME="${1:-}"
SYMBOL="${2:-}"
if [ -z "$NAME" ] || [ -z "$SYMBOL" ]; then
    echo "usage: $0 <name> <symbol>" >&2
    echo "  example: $0 'Alpha Creator' ALPHA" >&2
    exit 1
fi

# Client-side validation matching factory's validate_creator_token_info.
# Catches obvious mistakes before burning a tx.
NAME_LEN="${#NAME}"
if [ "$NAME_LEN" -lt 3 ] || [ "$NAME_LEN" -gt 50 ]; then
    echo "error: name must be 3–50 printable ASCII chars (got $NAME_LEN)" >&2
    exit 1
fi
if ! [[ "$SYMBOL" =~ ^[A-Z0-9]{3,12}$ ]]; then
    echo "error: symbol must be 3–12 chars matching ^[A-Z0-9]+$ (got '$SYMBOL')" >&2
    exit 1
fi
if ! [[ "$SYMBOL" =~ [A-Z] ]]; then
    echo "error: symbol must contain at least one A-Z letter (got '$SYMBOL')" >&2
    exit 1
fi

echo "creating commit pool: name='$NAME' symbol='$SYMBOL'"
echo "factory:    $FACTORY_ADDR"
echo "creator:    $ADMIN_ADDR"
echo ""

CREATE_MSG="$(jq -nc \
    --arg blue   "$BLUECHIP_DENOM" \
    --arg name   "$NAME" \
    --arg symbol "$SYMBOL" \
    '{create:{
        pool_msg:{
            pool_token_info:[
                {bluechip:{denom:$blue}},
                {creator_token:{contract_addr:"WILL_BE_CREATED_BY_FACTORY"}}
            ]
        },
        token_info:{
            name:$name,
            symbol:$symbol,
            decimal:6
        }
    }}')"

CREATE_RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" "$CREATE_MSG" \
    --amount "${STANDARD_POOL_FEE_FUNDS_BLUECHIP}${BLUECHIP_DENOM}")"

POOL_ID="$(extract_attr "$CREATE_RESULT" wasm pool_id)"
if [ -z "$POOL_ID" ] || [ "$POOL_ID" = "null" ]; then
    echo "error: could not extract pool_id from create tx" >&2
    echo "$CREATE_RESULT" | jq '.events[] | select(.type=="wasm" or .type=="create_pool")' >&2
    exit 1
fi
echo "pool_id:        $POOL_ID"

# Resolve pool address: filter the tx's instantiate events for the one
# whose code_id matches CREATOR_POOL_CODE_ID. The CW20 (creator token)
# and CW721 (position NFT) instantiate events also appear in this tx;
# we capture all three.
POOL_ADDR="$(echo "$CREATE_RESULT" | jq -r --arg cid "$CREATOR_POOL_CODE_ID" '
    .events[] | select(.type == "instantiate") |
    (.attributes | from_entries) as $a |
    select($a.code_id == $cid) | $a._contract_address
' | head -n 1)"
CREATOR_TOKEN_ADDR="$(echo "$CREATE_RESULT" | jq -r --arg cid "$CW20_CODE_ID" '
    .events[] | select(.type == "instantiate") |
    (.attributes | from_entries) as $a |
    select($a.code_id == $cid) | $a._contract_address
' | head -n 1)"
NFT_ADDR="$(echo "$CREATE_RESULT" | jq -r --arg cid "$CW721_CODE_ID" '
    .events[] | select(.type == "instantiate") |
    (.attributes | from_entries) as $a |
    select($a.code_id == $cid) | $a._contract_address
' | head -n 1)"

echo "pool address:   ${POOL_ADDR:-?}"
echo "creator token:  ${CREATOR_TOKEN_ADDR:-?}"
echo "position NFT:   ${NFT_ADDR:-?}"

# Append to commit_pools.txt (TSV, one line per pool).
LOG_FILE="$SCRIPT_DIR/commit_pools.txt"
printf '%s\t%s\t%s\t%s\n' "$POOL_ID" "$POOL_ADDR" "$CREATOR_TOKEN_ADDR" "$SYMBOL" >> "$LOG_FILE"
echo ""
echo "appended entry to $LOG_FILE"

echo ""
echo "NEXT:"
echo "  1. Cross the commit threshold:"
echo "     bash $SCRIPT_DIR/cross_threshold.sh $POOL_ADDR"
echo "  2. After threshold-cross, force a snapshot rebuild so the"
echo "     new commit pool flows into the oracle's eligible set:"
echo "     bash $SCRIPT_DIR/apply_oracle_eligibility.sh refresh"
