#!/usr/bin/env bash
# =====================================================================
# propose_oracle_eligibility.sh — kick off the 48h auto-eligible flag
# =====================================================================
# Sends:
#   factory.ProposeSetCommitPoolsAutoEligible { enabled: true }
#
# This stages a flip of the COMMIT_POOLS_AUTO_ELIGIBLE flag from the
# default (false on fresh instantiates) to true. The flip lands after
# ADMIN_TIMELOCK_SECONDS (48h) via factory.ApplySetCommitPoolsAutoEligible
# — see slice 4. Cancel before the timelock with
# CancelSetCommitPoolsAutoEligible.
#
# Run this immediately after deploy_osmo_testnet_anchor.sh so the 48h
# timer starts running while you're doing the slice-3 anchor-only
# tests. By the time bootstrap + TWAP + staleness verification is
# done, the timelock has expired and slice 4 (commit-pool creation +
# rotation tests) is ready to go.
#
# Idempotency: the factory's PENDING_COMMIT_POOLS_AUTO_ELIGIBLE Item
# rejects a second proposal while one is in flight. Re-running this
# script after a successful first run will fail with a clear error
# from the factory.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MSG='{"propose_set_commit_pools_auto_eligible":{"enabled":true}}'
echo "factory:        $FACTORY_ADDR"
echo "msg:            $MSG"
echo ""

RESULT="$(submit_tx wasm execute "$FACTORY_ADDR" "$MSG")"
TX_HASH="$(echo "$RESULT" | jq -r '.txhash')"
BLOCK_TIME="$(echo "$RESULT" | jq -r '.timestamp // empty')"

echo "OK — tx $TX_HASH"
if [ -n "$BLOCK_TIME" ]; then
    echo "block timestamp:  $BLOCK_TIME"
fi

# Print the on-chain pending entry so the operator knows when the
# timelock expires.
PENDING="$(query_raw_storage "$FACTORY_ADDR" 'pending_commit_auto_eligible')"
if [ -n "$PENDING" ]; then
    echo ""
    echo "pending entry:    $PENDING"
    PROPOSED_AT_NS="$(echo "$PENDING" | jq -r '.proposed_at // empty')"
    if [ -n "$PROPOSED_AT_NS" ]; then
        # proposed_at is nanoseconds (cosmwasm Timestamp). Convert to
        # seconds and add 48h (ADMIN_TIMELOCK_SECONDS).
        EFFECTIVE_AFTER_S=$(( PROPOSED_AT_NS / 1000000000 + 86400 * 2 ))
        if command -v date >/dev/null && date -u -d "@$EFFECTIVE_AFTER_S" +%Y-%m-%dT%H:%M:%SZ >/dev/null 2>&1; then
            echo "effective after:  $(date -u -d "@$EFFECTIVE_AFTER_S" +%Y-%m-%dT%H:%M:%SZ)"
        else
            echo "effective after:  unix=$EFFECTIVE_AFTER_S (proposed_at + 48h)"
        fi
    fi
fi

echo ""
echo "NEXT (after 48h elapses):"
echo "  bash $SCRIPT_DIR/apply_oracle_eligibility.sh   # slice 4 (TBD)"
