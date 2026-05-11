#!/usr/bin/env bash
# =====================================================================
# Shared helpers for the osmo-test-5 deploy + test scripts.
# =====================================================================
# Source from any caller after sourcing osmo_testnet.env. Reads:
#   CHAIN_ID, NODE, KEYRING, FROM, GAS_PRICES, GAS_ADJUSTMENT,
#   STATE_FILE.
#
# Exports (functions):
#   submit_tx <subcommand-and-args>           tx-result JSON to stdout
#   query_smart <contract> <msg_json>         response JSON to stdout
#   query_raw_storage <contract> <key>        decoded JSON or empty
#   extract_attr <tx_json> <type> <key>       first matching attr value
#   require_state                             asserts slice 1 ran, sources state file
#
# Conventions:
#   - All status / error messages go to stderr; only the requested
#     value goes to stdout. Safe to capture function output via $(...).
#   - submit_tx polls for inclusion (~30s) and fails loudly on any
#     non-zero tx code; raw_log is printed on failure.
# =====================================================================

__TX_FLAGS=(
    --chain-id "$CHAIN_ID"
    --node "$NODE"
    --keyring-backend "$KEYRING"
    --from "$FROM"
    --gas auto
    --gas-adjustment "$GAS_ADJUSTMENT"
    --gas-prices "$GAS_PRICES"
    -y -o json
)

submit_tx() {
    local raw
    if ! raw="$(osmosisd tx "$@" "${__TX_FLAGS[@]}" 2>&1)"; then
        echo "error: tx submit (mempool admission) failed for: $*" >&2
        echo "--- osmosisd output ---" >&2
        echo "$raw" >&2
        echo "-----------------------" >&2
        return 1
    fi
    # osmosisd v29 prints a "gas estimate: N" line on stderr before the
    # JSON response. With 2>&1 it lands in $raw alongside the JSON, so
    # walk lines from the bottom and pick the first that parses as JSON.
    # awk avoids subshell quirks (set -u + shell snapshot hooks) seen with
    # `grep '^{' | tail -n 1` when called inside $(...).
    local json
    json="$(printf '%s\n' "$raw" | awk '/^\{.*\}$/ {last=$0} END {print last}')"
    if [ -z "$json" ]; then
        echo "error: tx submit returned no JSON. raw output:" >&2
        echo "$raw" >&2
        return 1
    fi
    # CheckTx rejection: response carries height="0" and a non-zero code
    # (insufficient fee, contract revert at simulate, etc). Surface raw_log
    # so the operator sees *why* instead of a generic "no hash" failure.
    local check_code
    check_code="$(echo "$json" | jq -r '.code // 0' 2>/dev/null || echo 0)"
    if [ "$check_code" != "0" ]; then
        echo "error: tx rejected at CheckTx with code $check_code for: $*" >&2
        echo "$json" | jq -r '.raw_log' 2>/dev/null >&2 || echo "$json" >&2
        return 1
    fi
    local tx_hash
    tx_hash="$(echo "$json" | jq -r '.txhash // empty')"
    if [ -z "$tx_hash" ]; then
        echo "error: tx submit returned no hash. raw output:" >&2
        echo "$raw" >&2
        return 1
    fi
    local i result code
    for i in 1 2 3 4 5 6; do
        sleep 5
        if result="$(osmosisd query tx "$tx_hash" --node "$NODE" -o json 2>&1)"; then
            code="$(echo "$result" | jq -r '.code // 0')"
            if [ "$code" != "0" ]; then
                echo "error: tx $tx_hash failed with code $code" >&2
                echo "$result" | jq -r '.raw_log' >&2
                return 1
            fi
            echo "$result"
            return 0
        fi
    done
    echo "error: tx $tx_hash not indexed after 30s. check $NODE manually." >&2
    return 1
}

query_smart() {
    local contract="$1" msg="$2"
    local raw
    # osmosisd v29 routes query JSON to stderr in non-TTY contexts; merge so
    # jq finds the response regardless of which stream osmosisd picked.
    raw="$(osmosisd query wasm contract-state smart "$contract" "$msg" \
        --node "$NODE" -o json 2>&1)"
    # Newer osmosisd wraps responses in {data: ...}; older versions
    # return the raw response directly. Strip the wrapper if present.
    local data
    data="$(echo "$raw" | jq -c '.data // empty' 2>/dev/null || true)"
    if [ -n "$data" ] && [ "$data" != "null" ]; then
        echo "$data"
    else
        echo "$raw"
    fi
}

# Raw-storage read by Item key. Useful for state that isn't exposed
# via the public QueryMsg surface (PENDING_BOOTSTRAP_PRICE,
# INTERNAL_ORACLE, etc). Returns the JSON-decoded value or empty
# string if the key isn't set.
query_raw_storage() {
    local contract="$1" key_str="$2"
    local hex_key
    hex_key="$(printf '%s' "$key_str" | xxd -p -c 256)"
    local raw
    raw="$(osmosisd query wasm contract-state raw "$contract" "$hex_key" \
        --node "$NODE" -o json 2>&1 || echo '{}')"
    local b64
    b64="$(echo "$raw" | jq -r '.data // empty')"
    if [ -z "$b64" ] || [ "$b64" = "null" ]; then
        return 0
    fi
    echo "$b64" | base64 -d 2>/dev/null || true
}

extract_attr() {
    local json="$1" type="$2" key="$3"
    echo "$json" | jq -r --arg t "$type" --arg k "$key" '
        .events[] | select(.type == $t) | .attributes[]
        | select(.key == $k) | .value' | head -n 1
}

require_state() {
    if [ ! -f "$SCRIPT_DIR/$STATE_FILE" ]; then
        echo "error: $STATE_FILE not found in $SCRIPT_DIR — run deploy_osmo_testnet.sh first" >&2
        exit 1
    fi
    # shellcheck disable=SC1090
    source "$SCRIPT_DIR/$STATE_FILE"
    if [ -z "${FACTORY_ADDR:-}" ]; then
        echo "error: FACTORY_ADDR not set in $STATE_FILE — slice 1 incomplete" >&2
        exit 1
    fi
}
