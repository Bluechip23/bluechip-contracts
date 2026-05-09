#!/usr/bin/env bash
# =====================================================================
# cross_threshold.sh — commit enough BLUECHIP into a commit pool to
#                      cross the USD threshold and trigger payout +
#                      threshold-mint event.
# =====================================================================
# usage: bash cross_threshold.sh <pool_addr> [bluechip-amount-micro]
#
#   <pool_addr>             The commit pool address to commit into.
#                           Pass one of the addresses from
#                           commit_pools.txt (column 2).
#   [bluechip-amount-micro] Optional. Total BLUECHIP_DENOM to commit
#                           in one tx, in micro-units (6 decimals).
#                           Defaults to 60_000 BC = 60_000_000_000
#                           micro — sized to comfortably exceed the
#                           default $25_000 threshold across the
#                           range of OSMO/USD spot prices the test
#                           plan assumes (~$0.30–$1.00).
#
# How the threshold actually crosses on-chain:
#   1. The factory's commit handler converts the attached BLUECHIP
#      amount to USD via `bluechip_to_usd` (which reads the
#      currently-published bluechip price from the internal oracle).
#   2. Cumulative USD raised is compared against
#      `commit_threshold_limit_usd` (set at factory instantiate;
#      $25_000 by default, see osmo_testnet.env).
#   3. The first commit that crosses the threshold triggers
#      `trigger_threshold_payout` — mints the 1.2M creator-token
#      supply and routes it per ThresholdPayoutAmounts. Subsequent
#      activity on the pool flips to AMM swap / liquidity mode.
#
# Sizing notes:
#   - You must have at least the requested amount in BLUECHIP_DENOM
#     in the deployer wallet. The slice-1 deploy minted
#     BLUECHIP_INITIAL_MINT (10M BC); after seeding the anchor
#     (8K BC) and funding expand-economy (10K BC) you have ~9.98M
#     BC — so 60K BC fits trivially.
#   - If the oracle hasn't published yet (you skipped slice 3's
#     bootstrap confirm), the conversion will fail with "oracle has
#     no published price" and the commit reverts. Confirm the
#     bootstrap first via `test_bootstrap.sh confirm`.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

POOL_ADDR="${1:-}"
AMOUNT="${2:-60000000000}"  # 60_000 BC at 6 decimals
if [ -z "$POOL_ADDR" ]; then
    echo "usage: $0 <pool_addr> [bluechip-amount-micro]" >&2
    echo ""
    if [ -f "$SCRIPT_DIR/commit_pools.txt" ]; then
        echo "known commit pools (from commit_pools.txt):" >&2
        awk -F '\t' '{printf "  pool_id=%s addr=%s symbol=%s\n", $1, $2, $4}' \
            "$SCRIPT_DIR/commit_pools.txt" >&2
    else
        echo "(commit_pools.txt not found — run create_commit_pool.sh first)" >&2
    fi
    exit 1
fi

echo "pool:        $POOL_ADDR"
echo "committing:  $AMOUNT $BLUECHIP_DENOM"
echo ""

# Show the oracle-derived USD value of this commit before submitting,
# so the operator can sanity-check sizing.
if PRICE_RESP="$(query_smart "$FACTORY_ADDR" \
    '{"internal_blue_chip_oracle_query":{"convert_bluechip_to_usd":{"amount":"'$AMOUNT'"}}}' 2>&1)"; then
    USD_VALUE="$(echo "$PRICE_RESP" | jq -r '.amount // empty' 2>/dev/null || true)"
    if [ -n "$USD_VALUE" ]; then
        # USD is 6-decimal; print the human-readable value too
        USD_HUMAN="$(awk -v u="$USD_VALUE" 'BEGIN { printf "%.2f", u/1000000 }')"
        echo "oracle says: $AMOUNT $BLUECHIP_DENOM ≈ \$$USD_HUMAN USD"
    fi
else
    echo "warning: convert_bluechip_to_usd query failed — oracle may"
    echo "         not have a published price yet. The commit may revert."
    echo "         Run \`bash test_bootstrap.sh confirm\` if needed."
fi
echo ""

COMMIT_MSG="$(jq -nc \
    --arg blue "$BLUECHIP_DENOM" \
    --arg amt  "$AMOUNT" \
    '{commit:{
        asset:{
            info:{bluechip:{denom:$blue}},
            amount:$amt
        },
        transaction_deadline:null,
        belief_price:null,
        max_spread:null
    }}')"

RESULT="$(submit_tx wasm execute "$POOL_ADDR" "$COMMIT_MSG" \
    --amount "${AMOUNT}${BLUECHIP_DENOM}")"
echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"

# Pull the threshold-state out of the response if present —
# trigger_threshold_payout emits attributes when it fires.
THRESHOLD_FIRED="$(extract_attr "$RESULT" wasm threshold_crossed)"
USD_RAISED="$(extract_attr "$RESULT" wasm total_usd_raised)"
if [ -n "$THRESHOLD_FIRED" ]; then
    echo ""
    echo "threshold_crossed=$THRESHOLD_FIRED"
fi
if [ -n "$USD_RAISED" ]; then
    USD_HUMAN="$(awk -v u="$USD_RAISED" 'BEGIN { printf "%.2f", u/1000000 }')"
    echo "total_usd_raised: \$$USD_HUMAN"
fi

# Show the pool's commit status from the dedicated query.
echo ""
echo "=== pool.is_fully_commited ==="
if STATUS="$(query_smart "$POOL_ADDR" '{"is_fully_commited":{}}' 2>&1)"; then
    echo "$STATUS"
else
    echo "(query failed: $STATUS)"
fi

echo ""
echo "NEXT (if threshold crossed):"
echo "  1. Force snapshot refresh so the now-eligible commit pool"
echo "     enters the oracle sample set:"
echo "     bash $SCRIPT_DIR/apply_oracle_eligibility.sh refresh"
echo "  2. Watch rotation:"
echo "     bash $SCRIPT_DIR/test_rotation.sh"
