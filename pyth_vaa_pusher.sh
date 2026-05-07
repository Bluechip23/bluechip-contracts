#!/usr/bin/env bash
# =====================================================================
# pyth_vaa_pusher.sh — keeps Pyth on-chain price fresh on osmo-test-5
# =====================================================================
# Loop:
#   1. curl Hermes-beta for the latest VAA on $PYTH_OSMO_USD_FEED_ID.
#   2. Build {update_price_feeds:{data:[<vaa>]}} message.
#   3. Submit to $PYTH_CONTRACT.update_price_feeds, attaching the
#      per-VAA fee returned by Pyth's get_update_fee query.
#   4. Sleep $VAA_PUSH_INTERVAL_SECONDS and repeat.
#
# Usage:
#   bash pyth_vaa_pusher.sh           # loop until killed (Ctrl-C)
#   bash pyth_vaa_pusher.sh --once    # push exactly one VAA and exit
#
# The factory's MAX_PRICE_AGE_SECONDS_BEFORE_STALE = 90s, so the
# default 30s push interval gives 3x headroom. Pause the pusher
# (Ctrl-C) for the test_staleness.sh scenario; on resume, the next
# successful push restores oracle service — the breaker / cache-
# fallback paths handle the gap gracefully.
#
# Errors during the loop (Hermes 5xx, Pyth fee bumps, RPC blip) are
# logged and retried on the next cycle. The script does NOT exit
# unless --once is passed or it can't compute the update fee at all.
# =====================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"

MODE="${1:-loop}"
case "$MODE" in
    loop|--loop)  MODE="loop" ;;
    once|--once)  MODE="once" ;;
    *)
        echo "usage: $0 [--loop | --once]" >&2
        exit 1
        ;;
esac

echo "vaa pusher: pyth=$PYTH_CONTRACT feed=$PYTH_OSMO_USD_FEED_ID"
echo "            hermes=$HERMES_ENDPOINT interval=${VAA_PUSH_INTERVAL_SECONDS}s mode=$MODE"
echo ""

# Fetch the next VAA from Hermes. Returns a JSON array of base64
# strings on success (typically one element for a single-feed query).
fetch_vaas() {
    local resp
    resp="$(curl --silent --show-error --fail --max-time 10 \
        "${HERMES_ENDPOINT}/api/latest_vaas?ids[]=${PYTH_OSMO_USD_FEED_ID}" 2>&1)" || {
        echo "hermes fetch failed: $resp" >&2
        return 1
    }
    # Hermes returns base64 by default. Validate JSON shape.
    if ! echo "$resp" | jq -e 'type == "array" and length > 0' >/dev/null 2>&1; then
        echo "hermes returned unexpected payload: $resp" >&2
        return 1
    fi
    echo "$resp"
}

# Query the per-VAA update fee. Pyth's CW contract returns a
# cosmwasm_std::Coin {denom, amount}. Cache after first successful
# call — in steady state it doesn't change.
FEE_AMOUNT=""
FEE_DENOM=""
compute_fee() {
    local vaas="$1"
    local fee_q
    fee_q="$(jq -nc --argjson v "$vaas" '{get_update_fee:{vaas:$v}}')"
    local fee_resp
    fee_resp="$(query_smart "$PYTH_CONTRACT" "$fee_q" 2>/dev/null)" || return 1
    FEE_AMOUNT="$(echo "$fee_resp" | jq -r '.amount // empty')"
    FEE_DENOM="$(echo "$fee_resp" | jq -r '.denom // empty')"
    [ -n "$FEE_AMOUNT" ] && [ -n "$FEE_DENOM" ] || return 1
    return 0
}

push_one() {
    local vaas
    vaas="$(fetch_vaas)" || return 1

    if [ -z "$FEE_AMOUNT" ]; then
        if compute_fee "$vaas"; then
            echo "per-update fee: ${FEE_AMOUNT}${FEE_DENOM}"
        else
            # Default to 1uosmo — testnet Pyth fees are typically tiny
            # or zero. If the contract rejects, the next cycle will
            # try again.
            FEE_AMOUNT="1"
            FEE_DENOM="uosmo"
            echo "warning: get_update_fee failed; defaulting to 1uosmo per push" >&2
        fi
    fi

    local update_msg
    update_msg="$(jq -nc --argjson v "$vaas" '{update_price_feeds:{data:$v}}')"

    local result
    if result="$(submit_tx wasm execute "$PYTH_CONTRACT" "$update_msg" \
            --amount "${FEE_AMOUNT}${FEE_DENOM}")"; then
        local tx_hash
        tx_hash="$(echo "$result" | jq -r '.txhash')"
        echo "$(date -u +%H:%M:%SZ) push ok  tx=$tx_hash"
        return 0
    else
        echo "$(date -u +%H:%M:%SZ) push fail — will retry" >&2
        # Drop the cached fee in case Pyth bumped it; recompute next cycle.
        FEE_AMOUNT=""
        FEE_DENOM=""
        return 1
    fi
}

if [ "$MODE" = "once" ]; then
    push_one
    exit $?
fi

while true; do
    push_one || true
    sleep "$VAA_PUSH_INTERVAL_SECONDS"
done
