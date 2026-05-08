#!/usr/bin/env bash
# =====================================================================
# test_twap_advance.sh — drive + observe TWAP movement on the anchor
# =====================================================================
# Subcommands:
#   query                  Show anchor pool reserves, price
#                          accumulators, and the factory's published
#                          bluechip-USD price.
#   swap <uosmo-amount>    Execute a SimpleSwap on the anchor pool
#                          (offer asset = uosmo). Drives a price
#                          accumulator step which the next
#                          UpdateOraclePrice round samples into TWAP.
#                          Defaults to 1_000_000 (1 OSMO) if omitted.
#   update                 Call factory.UpdateOraclePrice. Subject to
#                          the keeper UPDATE_INTERVAL = 300s cooldown
#                          — cooldown enforcement happens on-chain
#                          and reverts the tx if not elapsed.
#
# Operator workflow:
#   1. After the VAA pusher has been running for a few minutes:
#      bash test_twap_advance.sh query
#   2. Run several swaps spaced ~30s apart to walk the accumulator:
#      bash test_twap_advance.sh swap 1000000
#      sleep 30 && bash test_twap_advance.sh swap 1000000
#      sleep 30 && bash test_twap_advance.sh swap 1000000
#   3. Wait UPDATE_INTERVAL (300s) since the last UpdateOraclePrice,
#      then call:
#      bash test_twap_advance.sh update
#   4. Re-query to verify observation_count incremented and twap_price
#      moved (or stayed within the 30% drift cap if branch (a)).
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

if [ -z "${ANCHOR_POOL_ADDR:-}" ]; then
    echo "error: ANCHOR_POOL_ADDR not in $STATE_FILE — run deploy_osmo_testnet_anchor.sh first" >&2
    exit 1
fi

MODE="${1:-query}"

show_state() {
    echo "=== anchor pool state ($ANCHOR_POOL_ADDR) ==="
    POOL_STATE="$(query_smart "$ANCHOR_POOL_ADDR" '{"pool_state":{}}')"
    echo "$POOL_STATE" | jq '{
        reserve0,
        reserve1,
        total_liquidity,
        block_time_last,
        nft_ownership_accepted
    }'
    echo ""
    echo "=== factory.GetOracleState (raw storage: internal_oracle) ==="
    ORACLE_STATE="$(query_raw_storage "$FACTORY_ADDR" 'internal_oracle')"
    if [ -n "$ORACLE_STATE" ]; then
        echo "$ORACLE_STATE" | jq '{
            selected_pools,
            last_rotation,
            rotation_interval,
            update_interval,
            twap_price: .bluechip_price_cache.last_price,
            last_update: .bluechip_price_cache.last_update,
            observation_count: (.bluechip_price_cache.twap_observations | length)
        }'
    else
        echo "(internal_oracle storage entry not found — oracle uninitialized)"
    fi
    echo ""
    echo "=== factory.GetBluechipUsdPrice ==="
    if PRICE_RESP="$(query_smart "$FACTORY_ADDR" \
        '{"internal_blue_chip_oracle_query":{"get_bluechip_usd_price":{}}}' 2>&1)"; then
        echo "$PRICE_RESP" | jq .
    else
        echo "(query failed — oracle hasn't published yet)"
    fi
}

swap() {
    local amount="${1:-1000000}"
    echo "executing simple_swap: $amount $NATIVE_DENOM → anchor pool"
    SWAP_MSG="$(jq -nc \
        --arg uosmo "$NATIVE_DENOM" \
        --arg amt "$amount" \
        '{simple_swap:{
            offer_asset:{
                info:{bluechip:{denom:$uosmo}},
                amount:$amt
            },
            belief_price:null,
            max_spread:"0.05",
            allow_high_max_spread:null,
            to:null,
            transaction_deadline:null
        }}')"
    RESULT="$(submit_tx wasm execute "$ANCHOR_POOL_ADDR" "$SWAP_MSG" \
        --amount "${amount}${NATIVE_DENOM}")"
    echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
    RETURN_AMOUNT="$(extract_attr "$RESULT" wasm return_amount)"
    echo "received: ${RETURN_AMOUNT:-?} $BLUECHIP_DENOM"
}

update_oracle() {
    echo "calling factory.UpdateOraclePrice ..."
    RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" '{"update_oracle_price":{}}')"
    echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
}

case "$MODE" in
    query)        show_state ;;
    swap)         swap "${2:-1000000}"; echo ""; show_state ;;
    update)       update_oracle; echo ""; show_state ;;
    *)
        echo "usage: $0 [query | swap <uosmo-amount> | update]" >&2
        exit 1
        ;;
esac
