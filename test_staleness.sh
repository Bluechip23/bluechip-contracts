#!/usr/bin/env bash
# =====================================================================
# test_staleness.sh — observe Pyth staleness behaviour
# =====================================================================
# Read-only. Reports:
#   - Current bluechip-USD price + freshness timestamp
#   - Seconds since last_update vs. MAX_PRICE_AGE_SECONDS_BEFORE_STALE
#     (90s); flags whether the next conversion will hit the live
#     branch, the cache-fallback branch, or fail entirely.
#   - Whether is_cached is true on the published price (i.e. we're
#     already serving from the bridge cache).
#
# Operator workflow:
#   1. While pyth_vaa_pusher.sh is running, run this script. last_update
#      should be < 30s; is_cached should be false; "will hit" should
#      report "live Pyth branch".
#   2. Ctrl-C the VAA pusher. Wait ~100s.
#   3. Re-run this script. last_update is now >90s old, the live Pyth
#      query inside `usd_to_bluechip` has started returning the cached
#      Pyth price (is_cached = true) until even the cache itself ages
#      out. The published bluechip-USD price stays stable but is now
#      bridge-served.
#   4. Wait until cache-fallback also fails (the cached_pyth ages out
#      past MAX_PRICE_AGE_SECONDS_BEFORE_STALE relative to the
#      sample-time of the cache). Re-run — query should now return an
#      error (no live Pyth, no usable cache).
#   5. Restart pyth_vaa_pusher.sh. Re-run — within one push interval
#      the freshness recovers.
#
# This script does NOT alter chain state; it's pure observability. To
# drive the staleness scenario, the operator pauses / resumes the
# pusher manually.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MAX_AGE=90  # MAX_PRICE_AGE_SECONDS_BEFORE_STALE (state.rs)

echo "=== bluechip-USD price ==="
if PRICE_RESP="$(query_smart "$FACTORY_ADDR" \
    '{"internal_blue_chip_oracle_query":{"get_bluechip_usd_price":{}}}' 2>&1)"; then
    echo "$PRICE_RESP" | jq .
    PUBLISHED_TS="$(echo "$PRICE_RESP" | jq -r '.timestamp // empty')"
    IS_CACHED="$(echo "$PRICE_RESP" | jq -r '.is_cached // false')"
    if [ -n "$PUBLISHED_TS" ]; then
        NOW_S="$(date -u +%s)"
        AGE=$(( NOW_S - PUBLISHED_TS ))
        echo ""
        echo "published_ts:        $PUBLISHED_TS"
        echo "now:                 $NOW_S"
        echo "age (seconds):       $AGE"
        echo "is_cached:           $IS_CACHED"
        echo ""
        if [ "$AGE" -le "$MAX_AGE" ] && [ "$IS_CACHED" = "false" ]; then
            echo "verdict:             LIVE — next conversion uses fresh Pyth"
        elif [ "$IS_CACHED" = "true" ]; then
            echo "verdict:             CACHED — bridging Pyth outage from snapshot"
            echo "                     (Pyth conf re-validated against current"
            echo "                     PYTH_CONF_THRESHOLD_BPS gate)"
        else
            echo "verdict:             STALE BUT UNCACHED — next conversion likely"
            echo "                     to error with \"Pyth price stale and no valid"
            echo "                     cached price available\""
        fi
    fi
else
    echo "(query failed — oracle has not published yet, or storage uninitialized)"
fi

echo ""
echo "=== Pyth contract last update (raw observation) ==="
echo "(if this script reports a stale price, run the same query against"
echo " $PYTH_CONTRACT to confirm whether Pyth itself is stale)"
echo ""
PYTH_QUERY="$(jq -nc --arg id "$PYTH_OSMO_USD_FEED_ID" '{price_feed:{id:$id}}')"
if PYTH_RESP="$(query_smart "$PYTH_CONTRACT" "$PYTH_QUERY" 2>&1)"; then
    echo "$PYTH_RESP" | jq '{
        price: .price_feed.price.price,
        publish_time: .price_feed.price.publish_time,
        conf: .price_feed.price.conf,
        ema_price: .price_feed.ema_price.price
    }'
    PYTH_TS="$(echo "$PYTH_RESP" | jq -r '.price_feed.price.publish_time // empty')"
    if [ -n "$PYTH_TS" ]; then
        NOW_S="$(date -u +%s)"
        echo ""
        echo "pyth publish_time:   $PYTH_TS"
        echo "pyth age (seconds):  $(( NOW_S - PYTH_TS ))"
    fi
else
    echo "(pyth query failed: $PYTH_RESP)"
fi
