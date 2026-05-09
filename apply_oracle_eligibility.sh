#!/usr/bin/env bash
# =====================================================================
# apply_oracle_eligibility.sh — land the 48h timelocked flag flip
# =====================================================================
# Slice 4 entry point. Run this AFTER the 48h timelock proposed by
# propose_oracle_eligibility.sh has expired.
#
# Subcommands:
#   query    (default) Show pending entry + remaining time, plus the
#            current value of COMMIT_POOLS_AUTO_ELIGIBLE in storage.
#   apply    factory.ApplySetCommitPoolsAutoEligible (will revert if
#            the timelock hasn't elapsed). Then immediately calls
#            factory.RefreshOraclePoolSnapshot to rebuild the
#            ELIGIBLE_POOL_SNAPSHOT against the new flag value.
#   cancel   factory.CancelSetCommitPoolsAutoEligible (only works
#            BEFORE the apply lands).
#   refresh  factory.RefreshOraclePoolSnapshot only — useful between
#            commit-pool creations to force the snapshot to pick up
#            the new pools without waiting on the lazy 5-day refresh.
#            Rate-limited on-chain to ~12h via
#            ORACLE_REFRESH_RATE_LIMIT_BLOCKS so it can't be spammed.
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
    echo "=== pending_commit_auto_eligible (raw storage) ==="
    PENDING="$(query_raw_storage "$FACTORY_ADDR" 'pending_commit_auto_eligible')"
    if [ -n "$PENDING" ]; then
        echo "$PENDING" | jq .
        PROPOSED_AT_NS="$(echo "$PENDING" | jq -r '.proposed_at // empty')"
        if [ -n "$PROPOSED_AT_NS" ]; then
            EFFECTIVE_AFTER_S=$(( PROPOSED_AT_NS / 1000000000 + 86400 * 2 ))
            NOW_S="$(date -u +%s)"
            REMAINING=$(( EFFECTIVE_AFTER_S - NOW_S ))
            echo ""
            if [ "$REMAINING" -gt 0 ]; then
                MIN=$(( REMAINING / 60 ))
                echo "timelock remaining: ${REMAINING}s (~${MIN} min). apply will revert until elapsed."
            else
                echo "timelock ELAPSED — \`apply\` subcommand will succeed."
            fi
        fi
    else
        echo "(no pending entry. either nothing was proposed yet, or apply/cancel already ran.)"
    fi
    echo ""
    echo "=== commit_pools_auto_eligible (current value) ==="
    CURR="$(query_raw_storage "$FACTORY_ADDR" 'commit_pools_auto_eligible')"
    if [ -n "$CURR" ]; then
        echo "current value: $CURR"
    else
        echo "(unset — defaults to false on fresh instantiates)"
    fi
}

case "$MODE" in
    query)
        show_state
        ;;
    apply)
        echo "sending factory.ApplySetCommitPoolsAutoEligible ..."
        APPLY_RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" \
            '{"apply_set_commit_pools_auto_eligible":{}}')"
        echo "OK — tx $(echo "$APPLY_RESULT" | jq -r '.txhash')"
        echo ""
        echo "refreshing oracle pool snapshot (so the new flag takes"
        echo "effect immediately rather than at next lazy refresh) ..."
        REFRESH_RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" \
            '{"refresh_oracle_pool_snapshot":{}}')"
        echo "OK — tx $(echo "$REFRESH_RESULT" | jq -r '.txhash')"
        echo ""
        show_state
        ;;
    cancel)
        echo "sending factory.CancelSetCommitPoolsAutoEligible ..."
        RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" \
            '{"cancel_set_commit_pools_auto_eligible":{}}')"
        echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
        echo ""
        show_state
        ;;
    refresh)
        echo "sending factory.RefreshOraclePoolSnapshot ..."
        RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" \
            '{"refresh_oracle_pool_snapshot":{}}')"
        echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
        ;;
    *)
        echo "usage: $0 [query | apply | cancel | refresh]" >&2
        exit 1
        ;;
esac
