#!/usr/bin/env bash
# =====================================================================
# test_rotation.sh — observe the oracle's pool-set rotation
# =====================================================================
# Usage:
#   bash test_rotation.sh             show one-shot oracle state
#   bash test_rotation.sh watch       poll every 60s, highlight changes
#
# What rotation looks like in the contract:
#   - Every UPDATE_INTERVAL = 300s, a keeper calls UpdateOraclePrice.
#   - Inside that handler, if more than ROTATION_INTERVAL = 3600s has
#     elapsed since `last_rotation`, the sample set is re-randomized
#     from the eligible-pool snapshot (anchor handled separately).
#   - The snapshot is built from ORACLE_ELIGIBLE_POOLS ∪ (commit pools
#     with COMMIT_POOLS_AUTO_ELIGIBLE=true). Slice 4 makes both inputs
#     non-empty.
#
# This script is read-only — it doesn't drive UpdateOraclePrice or
# RefreshOraclePoolSnapshot. Pair with:
#   - test_twap_advance.sh update      to trigger a keeper update
#   - apply_oracle_eligibility.sh refresh   to rebuild the snapshot
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MODE="${1:-once}"

show_state() {
    echo "=== eligible_pool_snap (raw storage) ==="
    SNAP="$(query_raw_storage "$FACTORY_ADDR" 'eligible_pool_snap')"
    if [ -n "$SNAP" ]; then
        echo "$SNAP" | jq '{
            pool_count: (.pool_addresses | length),
            captured_at_block,
            pool_addresses,
            bluechip_indices
        }'
    else
        echo "(snapshot not built yet — happens lazily on the next oracle update,"
        echo " or immediately when RefreshOraclePoolSnapshot is called.)"
    fi
    echo ""
    echo "=== internal_oracle.selected_pools + rotation timing ==="
    ORACLE="$(query_raw_storage "$FACTORY_ADDR" 'internal_oracle')"
    if [ -n "$ORACLE" ]; then
        echo "$ORACLE" | jq '{
            selected_pool_count: (.selected_pools | length),
            selected_pools,
            last_rotation,
            rotation_interval,
            update_interval,
            twap_price: .bluechip_price_cache.last_price,
            last_update: .bluechip_price_cache.last_update,
            observation_count: (.bluechip_price_cache.twap_observations | length)
        }'
        LAST_ROT="$(echo "$ORACLE" | jq -r '.last_rotation // 0')"
        ROT_INT="$(echo "$ORACLE" | jq -r '.rotation_interval // 3600')"
        if [ "$LAST_ROT" -gt 0 ]; then
            NEXT_ROT=$(( LAST_ROT + ROT_INT ))
            NOW_S="$(date -u +%s)"
            REMAINING=$(( NEXT_ROT - NOW_S ))
            echo ""
            if [ "$REMAINING" -gt 0 ]; then
                MIN=$(( REMAINING / 60 ))
                echo "next rotation eligible in: ${REMAINING}s (~${MIN} min)"
            else
                echo "next rotation: ELAPSED — the next UpdateOraclePrice will rotate."
            fi
        fi
    else
        echo "(internal_oracle storage entry not found — oracle uninitialized)"
    fi
}

case "$MODE" in
    once|--once|query)
        show_state
        ;;
    watch|--watch)
        echo "polling every 60s. press Ctrl-C to stop."
        echo ""
        while true; do
            echo "=== $(date -u +%H:%M:%SZ) ==="
            show_state
            echo ""
            sleep 60
        done
        ;;
    *)
        echo "usage: $0 [once | watch]" >&2
        exit 1
        ;;
esac
