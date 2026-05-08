#!/usr/bin/env bash
# =====================================================================
# test_bootstrap.sh — inspect / drive the bootstrap-price candidate
# =====================================================================
# Subcommands:
#   query     (default) Show the current bluechip-USD price + the raw
#             pending_bootstrap_price storage entry.
#   confirm   Send factory.ConfirmBootstrapPrice. Reverts on-chain if
#             BOOTSTRAP_OBSERVATION_SECONDS (1h) hasn't elapsed since
#             the candidate was first proposed.
#   cancel    Send factory.CancelBootstrapPrice. Discards the candidate;
#             the next successful UpdateOraclePrice round will re-enter
#             branch (d) and propose a fresh candidate.
#
# Operator workflow for slice 3:
#   1. Run `bash test_bootstrap.sh query` immediately after the VAA
#      pusher has had a few cycles. Expect:
#        - get_bluechip_usd_price returns 0 / fails (no published
#          price yet) OR the legacy bootstrap value if INITIAL_ANCHOR_SET
#          was true at deploy time (not our case).
#        - pending_bootstrap_price contains a candidate proposed at the
#          first successful TWAP round inside branch (d).
#   2. Wait BOOTSTRAP_OBSERVATION_SECONDS = 3600s (1h). Re-run query
#      to see observation_count grow as fresh TWAP rounds land.
#   3. Once you're satisfied the candidate is stable, run
#      `bash test_bootstrap.sh confirm` to publish it as last_price.
#      The next get_bluechip_usd_price call will return a non-zero
#      published value.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MODE="${1:-query}"

show_state() {
    echo "=== bluechip-USD price (factory.GetBluechipUsdPrice) ==="
    if PRICE_RESP="$(query_smart "$FACTORY_ADDR" \
        '{"internal_blue_chip_oracle_query":{"get_bluechip_usd_price":{}}}' 2>&1)"; then
        echo "$PRICE_RESP" | jq .
    else
        echo "(query failed — typically means oracle has not published yet)"
        echo "$PRICE_RESP"
    fi
    echo ""
    echo "=== pending_bootstrap_price (raw storage) ==="
    PENDING="$(query_raw_storage "$FACTORY_ADDR" 'pending_bootstrap_price')"
    if [ -n "$PENDING" ]; then
        echo "$PENDING" | jq .
        PROPOSED_AT_NS="$(echo "$PENDING" | jq -r '.proposed_at // empty')"
        if [ -n "$PROPOSED_AT_NS" ]; then
            PROPOSED_S=$(( PROPOSED_AT_NS / 1000000000 ))
            EARLIEST_CONFIRM_S=$(( PROPOSED_S + 3600 ))
            NOW_S="$(date -u +%s)"
            REMAINING=$(( EARLIEST_CONFIRM_S - NOW_S ))
            if [ "$REMAINING" -gt 0 ]; then
                echo ""
                echo "earliest confirm: ${REMAINING}s from now"
            else
                echo ""
                echo "earliest confirm: ELAPSED — confirm subcommand will succeed"
            fi
        fi
    else
        echo "(no pending candidate — either bootstrap already published or oracle hasn't reached branch (d) yet)"
    fi
}

case "$MODE" in
    query)
        show_state
        ;;
    confirm)
        echo "sending factory.ConfirmBootstrapPrice ..."
        RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" '{"confirm_bootstrap_price":{}}')"
        echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
        echo ""
        show_state
        ;;
    cancel)
        echo "sending factory.CancelBootstrapPrice ..."
        RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" '{"cancel_bootstrap_price":{}}')"
        echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
        echo ""
        show_state
        ;;
    *)
        echo "usage: $0 [query | confirm | cancel]" >&2
        exit 1
        ;;
esac
