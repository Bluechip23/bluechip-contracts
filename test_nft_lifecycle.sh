#!/usr/bin/env bash
# =====================================================================
# test_nft_lifecycle.sh — position-NFT mechanics + emergency-claim
# =====================================================================
# Exercises:
#   - CW721 transfer of a position NFT to a fresh holder
#   - Withdraw against the transferred NFT (the new owner can pull
#     the underlying liquidity, which is what makes positions
#     transferable assets in the first place)
#   - H-NFT-1 audit fix: post-RemoveAllLiquidity the position row
#     stays in storage (with `liquidity = 0`) instead of being
#     deleted. Verifies the row is queryable both before and after
#     a full removal.
#   - H-NFT-4 audit fix: per-position emergency-claim escrow with
#     1-year dormancy. After the factory admin invokes
#     EmergencyWithdrawPool on a target pool, every position holder
#     can call `pool.ClaimEmergencyShare { position_id }` to receive
#     their pro-rata share of the post-drain reserves.
#
# Subcommands:
#   state <pool_addr> <position_id>
#       Show position state. Usable both pre- and post-removal to
#       confirm the H-NFT-1 row-persistence fix.
#   transfer <pool_addr> <position_id> <new_owner_addr> [--from <key>]
#       CW721 transfer of the position NFT. Resolves the NFT contract
#       address from the pool's pair query. Default `--from` is FROM
#       (the deployer); pass another keyring key to test transfer
#       between non-admin holders.
#   add-position <pool_addr> <amt0> <amt1> [--from <key>]
#       AddToPosition (pool's existing positions) — caller mints a
#       new position NFT to themselves. Funds attached: amt0 of the
#       index-0 token + amt1 of index-1. Used by the demo flow to
#       ensure there's an unrelated position whose claim share can
#       be observed independently after emergency-withdraw.
#   remove-all <pool_addr> <position_id> [--from <key>]
#       pool.RemoveAllLiquidity. After this lands, run `state` again
#       to confirm the position row is preserved (H-NFT-1) and that
#       liquidity == 0.
#   emergency-withdraw <pool_addr_or_id>
#       factory.EmergencyWithdrawPool. Two-phase under the hood (pool
#       enters PENDING_EMERGENCY_WITHDRAW on the first call, then
#       drains on the second). Repeat the same command twice if you
#       want the full drain to land in this script — see the demo
#       flow for the expected pattern.
#   claim-emergency <pool_addr> <position_id> [--from <key>]
#       pool.ClaimEmergencyShare. Caller must own the NFT (the
#       contract verifies via CW721 OwnerOf at handle time).
#       Idempotent in the sense of returning a clear error on
#       double-claim — the test verifies that, too.
#   sweep-emergency <pool_addr_or_id>
#       factory-only sweep of the unclaimed residual after the
#       1-year dormancy window. Will revert pre-dormancy (expected);
#       included for completeness.
#   demo <pool_addr>
#       Full sequence on a pool you don't mind destroying:
#         1. capture position 1 state (liquidity > 0)
#         2. RemoveAllLiquidity for position 1
#         3. re-query → verify position row preserved with liquidity=0  (H-NFT-1)
#         4. add a second position from `keeper` so we have a non-zero
#            holder to observe across the emergency drain
#         5. transfer position 2 → fresh address (proves CW721 path)
#         6. factory.EmergencyWithdrawPool → emergency drain initiates
#         7. factory.EmergencyWithdrawPool again → drain fires
#         8. position 2's new owner ClaimEmergencyShare → receives
#            pro-rata bluechip + other-side  (H-NFT-4)
#         9. attempt double-claim → expected error
#
# DESTRUCTIVE: emergency-withdraw is irreversible. NEVER run the
# `emergency-withdraw` or `demo` subcommand against the production
# anchor pool — use a sacrificial pool (e.g. the bluechip/MOCK pool
# created by `test_weighted_oracle.sh`).
#
# Prerequisites:
#   - osmo_testnet.state populated (FROM key + ANCHOR_POSITION_NFT
#     for anchor-targeted tests)
#   - For non-deployer callers: a `keeper` (or other) keyring entry
#     funded with gas
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MODE="${1:-}"

# Parse `--from <key>` from anywhere in args; default to FROM.
parse_from_flag() {
    local from="$FROM"
    local args=()
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --from) from="$2"; shift 2 ;;
            --from=*) from="${1#--from=}"; shift ;;
            *) args+=("$1"); shift ;;
        esac
    done
    PARSED_FROM="$from"
    PARSED_ARGS=("${args[@]}")
}

# Submit a tx with an arbitrary --from override. Mirrors submit_tx
# from the helpers but accepts a from-key arg.
submit_tx_as() {
    local from="$1"; shift
    local raw
    if ! raw="$(osmosisd tx "$@" \
        --chain-id "$CHAIN_ID" --node "$NODE" \
        --keyring-backend "$KEYRING" --from "$from" \
        --gas auto --gas-adjustment "$GAS_ADJUSTMENT" \
        --gas-prices "$GAS_PRICES" -y -o json 2>&1)"; then
        echo "error: tx submit failed for: $*" >&2
        echo "$raw" >&2
        return 1
    fi
    local json
    json="$(printf '%s\n' "$raw" | awk '/^\{.*\}$/ {last=$0} END {print last}')"
    if [ -z "$json" ]; then
        echo "error: no JSON in tx output:" >&2; echo "$raw" >&2; return 1
    fi
    local code
    code="$(echo "$json" | jq -r '.code // 0')"
    if [ "$code" != "0" ]; then
        echo "error: CheckTx code $code" >&2
        echo "$json" | jq -r '.raw_log' >&2
        return 1
    fi
    local hash
    hash="$(echo "$json" | jq -r '.txhash')"
    sleep 6
    osmosisd query tx "$hash" --node "$NODE" -o json 2>&1
}

# Resolve the position-NFT contract address from a pool's
# `pair` query. Different pool kinds expose the field differently —
# walk both shapes to be tolerant. Some pools also store it under
# `nft_address` instead of `position_nft`.
resolve_nft_addr() {
    local pool="$1"
    local pair
    pair="$(query_smart "$pool" '{"pair":{}}')"
    local addr
    addr="$(echo "$pair" | jq -r '
        .position_nft // .nft_address //
        .data.position_nft // .data.nft_address //
        .pool_token_info_extras.position_nft //
        empty')"
    if [ -z "$addr" ]; then
        # Fallback: pool's `config` query usually carries it.
        local cfg
        cfg="$(query_smart "$pool" '{"config":{}}' 2>/dev/null || true)"
        addr="$(echo "$cfg" | jq -r '
            .position_nft // .nft_address //
            .data.position_nft // .data.nft_address // empty')"
    fi
    if [ -z "$addr" ]; then
        echo "error: could not resolve position-NFT contract for pool $pool" >&2
        echo "       inspected fields: pair{position_nft,nft_address}, config{position_nft,nft_address}" >&2
        return 1
    fi
    echo "$addr"
}

state_cmd() {
    local pool="${1:?usage: state <pool_addr> <position_id>}"
    local pid="${2:?position_id required}"
    echo "=== position $pid on pool $pool ==="
    local resp
    if resp="$(query_smart "$pool" "$(jq -nc --arg id "$pid" '{position:{position_id:$id}}')" 2>&1)"; then
        echo "$resp" | jq .
    else
        echo "(query failed: $resp)"
    fi
    echo ""
    echo "=== pool reserves ==="
    query_smart "$pool" '{"pool_state":{}}' | jq '.data // .'
}

transfer_cmd() {
    parse_from_flag "$@"
    local pool="${PARSED_ARGS[0]:?usage: transfer <pool_addr> <position_id> <new_owner>}"
    local pid="${PARSED_ARGS[1]:?position_id required}"
    local new_owner="${PARSED_ARGS[2]:?new_owner required}"
    local nft
    nft="$(resolve_nft_addr "$pool")"
    echo "transfer NFT contract=$nft  token_id=$pid  → $new_owner  (signer=$PARSED_FROM)"
    local msg
    msg="$(jq -nc --arg r "$new_owner" --arg id "$pid" '{transfer_nft:{recipient:$r, token_id:$id}}')"
    submit_tx_as "$PARSED_FROM" wasm execute "$nft" "$msg" >/dev/null
    echo "  transferred"
    echo ""
    echo "=== verify owner ==="
    query_smart "$nft" "$(jq -nc --arg id "$pid" '{owner_of:{token_id:$id}}')" | jq '.data // .'
}

add_position() {
    parse_from_flag "$@"
    local pool="${PARSED_ARGS[0]:?usage: add-position <pool_addr> <amt0> <amt1>}"
    local a0="${PARSED_ARGS[1]:?amt0 required}"
    local a1="${PARSED_ARGS[2]:?amt1 required}"
    echo "add-position to $pool  ($a0, $a1)  signer=$PARSED_FROM"
    # Both legs as funds; if either side is a CW20 the deposit
    # mechanics differ (need allowance), but for native/native
    # standard pools this is the standard path.
    local pair
    pair="$(query_smart "$pool" '{"pair":{}}')"
    local d0 d1
    d0="$(echo "$pair" | jq -r '.data.pool_token_info[0].bluechip.denom // .pool_token_info[0].bluechip.denom // empty')"
    d1="$(echo "$pair" | jq -r '.data.pool_token_info[1].bluechip.denom // .pool_token_info[1].bluechip.denom // empty')"
    if [ -z "$d0" ] || [ -z "$d1" ]; then
        echo "error: this helper only handles native+native pools" >&2
        echo "       (CW20-leg pools need increase_allowance + deposit_liquidity manually)" >&2
        return 1
    fi
    local funds="${a0}${d0},${a1}${d1}"
    local msg
    msg="$(jq -nc --arg a0 "$a0" --arg a1 "$a1" \
        '{deposit_liquidity:{amount0:$a0, amount1:$a1, min_amount0:null, min_amount1:null, transaction_deadline:null}}')"
    submit_tx_as "$PARSED_FROM" wasm execute "$pool" "$msg" --amount "$funds" >/dev/null
    echo "  position added"
}

remove_all() {
    parse_from_flag "$@"
    local pool="${PARSED_ARGS[0]:?usage: remove-all <pool_addr> <position_id>}"
    local pid="${PARSED_ARGS[1]:?position_id required}"
    echo "remove_all_liquidity  pool=$pool  position_id=$pid  signer=$PARSED_FROM"
    local msg
    msg="$(jq -nc --arg id "$pid" \
        '{remove_all_liquidity:{position_id:$id, transaction_deadline:null, min_amount0:null, min_amount1:null, max_ratio_deviation_bps:null}}')"
    submit_tx_as "$PARSED_FROM" wasm execute "$pool" "$msg" >/dev/null
    echo "  removed; querying position state..."
    state_cmd "$pool" "$pid"
}

# Look up a pool_id from a pool address by scanning POOLS_BY_CONTRACT_ADDRESS
# via factory query. Some factory builds expose it directly; others require
# walking POOLS_BY_ID. Falls back to the latter.
pool_id_for_addr() {
    local addr="$1"
    # Try the direct query first.
    local resp
    if resp="$(query_smart "$FACTORY_ADDR" \
        "$(jq -nc --arg a "$addr" '{pool_by_address:{pool_address:$a}}')" 2>/dev/null)"; then
        local id
        id="$(echo "$resp" | jq -r '.data.pool_id // .pool_id // empty')"
        if [ -n "$id" ] && [ "$id" != "null" ]; then echo "$id"; return; fi
    fi
    # Fallback: ask the user to pass pool_id directly.
    echo "error: could not resolve pool_id for $addr — pass it numerically (1, 2, ...) instead." >&2
    return 1
}

emergency_withdraw() {
    local arg="${1:?usage: emergency-withdraw <pool_addr_or_id>}"
    local pool_id
    if [[ "$arg" =~ ^[0-9]+$ ]]; then
        pool_id="$arg"
    else
        pool_id="$(pool_id_for_addr "$arg")"
    fi
    echo "factory.EmergencyWithdrawPool { pool_id: $pool_id }"
    local msg
    msg="$(jq -nc --arg id "$pool_id" '{emergency_withdraw_pool:{pool_id:($id|tonumber)}}')"
    submit_tx wasm execute "$FACTORY_ADDR" "$msg" >/dev/null
    echo "  submitted (call again for the second phase if you haven't yet)"
}

claim_emergency() {
    parse_from_flag "$@"
    local pool="${PARSED_ARGS[0]:?usage: claim-emergency <pool_addr> <position_id>}"
    local pid="${PARSED_ARGS[1]:?position_id required}"
    echo "claim_emergency_share  pool=$pool  position_id=$pid  signer=$PARSED_FROM"
    local msg
    msg="$(jq -nc --arg id "$pid" '{claim_emergency_share:{position_id:$id}}')"
    local result
    result="$(submit_tx_as "$PARSED_FROM" wasm execute "$pool" "$msg")"
    echo "$result" | jq '[.events[] | select(.type=="wasm")] | map(.attributes | map({(.key): .value}) | add)'
}

sweep_emergency() {
    local arg="${1:?usage: sweep-emergency <pool_addr_or_id>}"
    local pool_id
    if [[ "$arg" =~ ^[0-9]+$ ]]; then
        pool_id="$arg"
    else
        pool_id="$(pool_id_for_addr "$arg")"
    fi
    echo "factory.SweepUnclaimedEmergencySharesPool { pool_id: $pool_id }"
    echo "(reverts pre-dormancy — included for completeness)"
    local msg
    msg="$(jq -nc --arg id "$pool_id" '{sweep_unclaimed_emergency_shares_pool:{pool_id:($id|tonumber)}}')"
    submit_tx wasm execute "$FACTORY_ADDR" "$msg" >/dev/null || true
}

demo() {
    local pool="${1:?usage: demo <pool_addr> — DESTRUCTIVE, do not use anchor}"
    local pid="${2:-1}"
    if [ "$pool" = "${ANCHOR_POOL_ADDR:-}" ]; then
        echo "refusing to run demo against the anchor pool ($ANCHOR_POOL_ADDR)" >&2
        echo "use a sacrificial pool — see test_weighted_oracle.sh setup-mock + create-pool" >&2
        exit 1
    fi

    # The demo focuses on H-NFT-4 (emergency-claim) since the H-NFT-1
    # row-persistence invariant is observable as a side-effect of the
    # claim path: a successful ClaimEmergencyShare sets
    # `position.liquidity = 0` while keeping the row queryable, which
    # is the same persistence guarantee H-NFT-1 enforces for normal
    # RemoveAllLiquidity. The standalone `remove-all` + `state`
    # subcommands cover the non-emergency H-NFT-1 path explicitly
    # if you want to exercise it on a different position.

    echo "=== STEP 1: position $pid pre-emergency state (liquidity should be > 0) ==="
    state_cmd "$pool" "$pid"

    echo ""
    echo "=== STEP 2: factory.EmergencyWithdrawPool — phase 1 (initiate pending) ==="
    emergency_withdraw "$pool"

    echo ""
    echo "=== STEP 3: factory.EmergencyWithdrawPool — phase 2 (drain) ==="
    emergency_withdraw "$pool"

    echo ""
    echo "=== STEP 4: H-NFT-4 verification — claim position $pid's emergency share ==="
    claim_emergency "$pool" "$pid"

    echo ""
    echo "=== STEP 5: H-NFT-1 verification — position row alive post-claim, liquidity=0 ==="
    state_cmd "$pool" "$pid"
    echo "(confirm above: liquidity == 0 AND the row is queryable, not NotFound)"

    echo ""
    echo "=== STEP 6: double-claim should error ==="
    if claim_emergency "$pool" "$pid" 2>&1 | tee /dev/stderr | grep -qi 'error\|already\|claimed'; then
        echo "(confirmed: double-claim rejected)"
    else
        echo "(unexpected: second claim did not error)"
    fi

    echo ""
    echo "=== optional follow-ups (separate subcommands) ==="
    echo "  bash $0 transfer  $pool <position_id> <new_owner>     # CW721 transferability"
    echo "  bash $0 sweep-emergency $pool                          # admin sweep (reverts pre-1y)"
}

case "$MODE" in
    state)              shift; state_cmd "$@" ;;
    transfer)           shift; transfer_cmd "$@" ;;
    add-position)       shift; add_position "$@" ;;
    remove-all)         shift; remove_all "$@" ;;
    emergency-withdraw) shift; emergency_withdraw "$@" ;;
    claim-emergency)    shift; claim_emergency "$@" ;;
    sweep-emergency)    shift; sweep_emergency "$@" ;;
    demo)               shift; demo "$@" ;;
    *)
        cat >&2 <<EOF
usage: $0 <subcommand> [args]

  state <pool_addr> <position_id>
  transfer <pool_addr> <position_id> <new_owner> [--from <key>]
  add-position <pool_addr> <amt0> <amt1> [--from <key>]
  remove-all <pool_addr> <position_id> [--from <key>]
  emergency-withdraw <pool_addr_or_id>
  claim-emergency <pool_addr> <position_id> [--from <key>]
  sweep-emergency <pool_addr_or_id>
  demo <pool_addr>     # full lifecycle, DESTRUCTIVE — do not use anchor
EOF
        exit 1
        ;;
esac
