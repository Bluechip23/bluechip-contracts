#!/usr/bin/env bash
# =====================================================================
# test_weighted_oracle.sh — multi-pool weighted-oracle math
# =====================================================================
# The oracle samples up to ORACLE_POOL_COUNT eligible pools per round
# and produces a single weighted bluechip-per-OSMO TWAP. Each pool
# contributes weight proportional to its bluechip-side reserve. The
# H3 "single-pool-per-pair" gate prevents creating a second
# bluechip/uosmo pool, so this script bootstraps a second STANDARD
# pool against a freshly-instantiated CW20 mock asset, allowlists
# it via the curated source (`ProposeAddOracleEligiblePool`), then
# verifies the published TWAP matches the on-chain weighted formula.
#
# Subcommands:
#   setup-mock <symbol> <supply-micro>
#       Instantiate a CW20 from the cw20_base code already uploaded
#       in slice 1 (CW20_CODE_ID), with `supply-micro` minted to the
#       deployer. Stores the address in MOCK_CW20_ADDR (oracle.state).
#   create-pool <mock_addr> <bluechip-seed-micro> <mock-seed-micro>
#       factory.CreateStandardPool([bluechip, MOCK]) → deposit_liquidity.
#       The seed ratio determines the pool's spot price; choose it
#       distinct from the anchor's so the weighted blend is observable
#       (anchor seeded at 1600 BC/uosmo; pick e.g. 800 or 3200 BC/MOCK).
#   propose-allowlist <pool_addr>
#       factory.ProposeAddOracleEligiblePool. Waits ADMIN_TIMELOCK
#       seconds (300 in the testnet build), then applies. Pool enters
#       ORACLE_ELIGIBLE_POOLS; the next snapshot refresh + oracle
#       update lifts it into selected_pools.
#   refresh-and-update
#       factory.RefreshOraclePoolSnapshot + factory.UpdateOraclePrice.
#       Uses the existing test_twap_advance.sh swap helper to advance
#       the bluechip/uosmo anchor's accumulator, then drives an
#       update so both pools land in `pool_cumulative_snapshots`.
#   observe
#       Read every pool in selected_pools, compute the weighted spot
#       price = Σ (R_bluechip_i * P_i) / Σ R_bluechip_i, compare
#       against the oracle's published `last_price`. The published
#       value is a TWAP (time-integrated average), not the
#       instantaneous spot, so the values agree only when reserves
#       have been stable across the prior update window — see the
#       observe output for the expected vs published deltas.
#   demo <symbol> <bluechip-seed-micro> <mock-seed-micro>
#       Full pipeline: setup-mock → create-pool → propose-allowlist →
#       (5min wait) → apply → refresh-and-update → observe. End-to-end.
#
# Prerequisites:
#   - Slice 1+2 have run; osmo_testnet.state is populated.
#   - The pyth_vaa_pusher.sh is running so Pyth-dependent commit
#     pricing is available where needed (allowlist apply doesn't
#     need it, but refresh-and-update does for the breaker check).
#   - Deployer wallet has gas + at least the seed-funding bluechip
#     plus the CW20 standard-pool creation fee
#     (STANDARD_POOL_FEE_FUNDS_BLUECHIP).
#
# Side effects:
#   Appends MOCK_CW20_ADDR, MOCK_POOL_ID, MOCK_POOL_ADDR to
#   osmo_testnet.state so subsequent runs can pick up the addresses
#   without re-deriving them from tx events.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

MODE="${1:-}"

# Append a key=value line to osmo_testnet.state, replacing any prior
# entry for the same key. Keeps state-file readable as a sourceable
# bash file (`source osmo_testnet.state` works after).
state_set() {
    local key="$1" val="$2"
    local file="$SCRIPT_DIR/$STATE_FILE"
    # Strip prior key= line if present
    if grep -q "^${key}=" "$file" 2>/dev/null; then
        # Use a tmp file to avoid in-place sed issues
        grep -v "^${key}=" "$file" > "${file}.tmp" && mv "${file}.tmp" "$file"
    fi
    echo "${key}=\"${val}\"" >> "$file"
}

setup_mock() {
    local symbol="${1:?usage: setup-mock <symbol> <supply-micro>}"
    local supply="${2:?supply-micro required (6 decimals)}"
    [ -z "${CW20_CODE_ID:-}" ] && {
        echo "error: CW20_CODE_ID not in $STATE_FILE — slice 1 incomplete" >&2
        exit 1
    }
    echo "instantiating CW20 mock '$symbol' with supply $supply micro to $ADMIN_ADDR"
    local init
    init="$(jq -nc \
        --arg name   "Mock $symbol" \
        --arg symbol "$symbol" \
        --arg owner  "$ADMIN_ADDR" \
        --arg amount "$supply" \
        '{name:$name, symbol:$symbol, decimals:6,
          initial_balances:[{address:$owner, amount:$amount}]}')"
    local result
    result="$(submit_tx wasm instantiate "$CW20_CODE_ID" "$init" \
        --label "weighted_oracle_mock_${symbol}" --no-admin)"
    local addr
    addr="$(extract_attr "$result" instantiate _contract_address)"
    [ -z "$addr" ] && { echo "error: could not extract CW20 address" >&2; exit 1; }
    echo "mock CW20: $addr"
    state_set MOCK_CW20_ADDR "$addr"
}

create_pool() {
    local mock_addr="${1:?usage: create-pool <mock_addr> <bc-seed> <mock-seed>}"
    local bc_seed="${2:?bc-seed required}"
    local mock_seed="${3:?mock-seed required}"
    echo "create-pool: bluechip/$mock_addr  seeds=($bc_seed BC, $mock_seed MOCK)"

    # 1. Create the pool via factory. Pays standard-pool creation fee
    # in canonical bluechip; the second token is a CreatorToken (which
    # is just the wire-tag for "any CW20", validated by the factory's
    # CW20 TokenInfo probe).
    local create_msg
    create_msg="$(jq -nc \
        --arg blue "$BLUECHIP_DENOM" \
        --arg mock "$mock_addr" \
        '{create_standard_pool:{
            pool_token_info:[
                {bluechip:{denom:$blue}},
                {creator_token:{contract_addr:$mock}}
            ],
            label:"bluechip_mock_pool"
        }}')"
    local result
    result="$(submit_tx wasm execute "$FACTORY_ADDR" "$create_msg" \
        --amount "${STANDARD_POOL_FEE_FUNDS_BLUECHIP}${BLUECHIP_DENOM}")"

    local pool_id pool_addr
    pool_id="$(extract_attr "$result" wasm pool_id)"
    pool_addr="$(echo "$result" | jq -r --arg cid "$STANDARD_POOL_CODE_ID" '
        .events[] | select(.type == "instantiate") |
        (.attributes | from_entries) as $a |
        select($a.code_id == $cid) | $a._contract_address' | head -n 1)"
    [ -z "$pool_addr" ] && { echo "error: pool address not resolved" >&2; exit 1; }
    echo "  pool_id:   $pool_id"
    echo "  pool_addr: $pool_addr"
    state_set MOCK_POOL_ID "$pool_id"
    state_set MOCK_POOL_ADDR "$pool_addr"

    # 2. Authorise the pool to spend the deployer's MOCK tokens, then
    # deposit. Standard CW20 increase-allowance pattern: deposit_liquidity
    # transfers via transfer-from rather than receive-cw20, so an
    # explicit allowance is required.
    echo "  approving $mock_seed MOCK to pool spender"
    local approve_msg
    approve_msg="$(jq -nc \
        --arg spender "$pool_addr" \
        --arg amount "$mock_seed" \
        '{increase_allowance:{spender:$spender, amount:$amount}}')"
    submit_tx wasm execute "$mock_addr" "$approve_msg" >/dev/null

    echo "  deposit_liquidity"
    local deposit_msg
    deposit_msg="$(jq -nc \
        --arg a0 "$bc_seed" \
        --arg a1 "$mock_seed" \
        '{deposit_liquidity:{
            amount0:$a0, amount1:$a1,
            min_amount0:null, min_amount1:null,
            transaction_deadline:null}}')"
    # bluechip side is index 0 (Native bluechip listed first in pair).
    # Funds attached: bluechip only (the CW20 leg comes via allowance).
    local result2
    result2="$(submit_tx wasm execute "$pool_addr" "$deposit_msg" \
        --amount "${bc_seed}${BLUECHIP_DENOM}")"
    local pos_id
    pos_id="$(extract_attr "$result2" wasm position_id)"
    echo "  position_id: ${pos_id:-(not captured)}"
}

propose_allowlist() {
    local pool_addr="${1:?usage: propose-allowlist <pool_addr>}"
    echo "factory.ProposeAddOracleEligiblePool { pool_addr: $pool_addr }"
    local msg
    msg="$(jq -nc --arg p "$pool_addr" '{propose_add_oracle_eligible_pool:{pool_addr:$p}}')"
    submit_tx wasm execute "$FACTORY_ADDR" "$msg" >/dev/null
    echo ""
    echo "Pending allowlist add for $pool_addr."
    echo "Wait ADMIN_TIMELOCK_SECONDS (testnet build: 300s), then run"
    echo "  bash $0 apply-allowlist $pool_addr"
}

apply_allowlist() {
    local pool_addr="${1:?usage: apply-allowlist <pool_addr>}"
    echo "factory.ApplyAddOracleEligiblePool { pool_addr: $pool_addr }"
    local msg
    msg="$(jq -nc --arg p "$pool_addr" '{apply_add_oracle_eligible_pool:{pool_addr:$p}}')"
    submit_tx wasm execute "$FACTORY_ADDR" "$msg" >/dev/null
    echo "  applied — pool added to ORACLE_ELIGIBLE_POOLS"
}

refresh_and_update() {
    echo "factory.RefreshOraclePoolSnapshot"
    submit_tx wasm execute "$FACTORY_ADDR" '{"refresh_oracle_pool_snapshot":{}}' >/dev/null
    echo ""
    echo "advancing anchor accumulator with a small swap"
    bash "$SCRIPT_DIR/test_twap_advance.sh" swap 100000 >/dev/null 2>&1 || true
    echo "(also swap on the new pool would help, skipping for brevity — re-run if zero pool_used delta)"
    echo ""
    echo "waiting UPDATE_INTERVAL ($([ -n "${UPDATE_INTERVAL:-}" ] && echo "$UPDATE_INTERVAL" || echo "60-300"))s before update"
    sleep 65
    echo "factory.UpdateOraclePrice"
    submit_tx wasm execute "$FACTORY_ADDR" '{"update_oracle_price":{}}' >/dev/null
    echo "  done"
}

observe() {
    echo "=== selected_pools ==="
    local oracle
    oracle="$(query_raw_storage "$FACTORY_ADDR" 'internal_oracle')"
    echo "$oracle" | jq '{
        selected_pools,
        published_last_price: .bluechip_price_cache.last_price,
        last_update: .bluechip_price_cache.last_update,
        warmup_remaining
    }'
    local pools
    pools="$(echo "$oracle" | jq -r '.selected_pools[]')"

    # Pull the anchor index pin so we know which side is bluechip on the anchor.
    local anchor_idx
    anchor_idx="$(echo "$oracle" | jq -r '.anchor_bluechip_index // 0')"

    echo ""
    echo "=== per-pool spot snapshot ==="
    local total_weight=0
    local weighted_sum=0
    for pool in $pools; do
        local state
        state="$(query_smart "$pool" '{"pool_state":{}}' 2>/dev/null \
            || query_smart "$pool" '{"pool_state":{}}')"
        # standard pools and creator pools both expose reserve0/reserve1
        local r0 r1 t
        r0="$(echo "$state" | jq -r '.reserve0 // .data.reserve0 // 0')"
        r1="$(echo "$state" | jq -r '.reserve1 // .data.reserve1 // 0')"
        t="$(echo "$state"  | jq -r '.block_time_last // .data.block_time_last // 0')"
        # Bluechip-side index lookup. For the anchor we pinned it; for
        # eligible pools it's stored in eligible_pool_snap.bluechip_indices,
        # parallel to .pool_addresses. Read both and pick the matching one.
        local bidx=0
        if [ "$pool" = "$ANCHOR_POOL_ADDR" ]; then
            bidx="$anchor_idx"
        else
            local snap
            snap="$(query_raw_storage "$FACTORY_ADDR" 'eligible_pool_snap')"
            bidx="$(echo "$snap" | jq -r --arg p "$pool" '
                (.pool_addresses // []) as $addrs |
                (.bluechip_indices // []) as $idxs |
                ($addrs | index($p)) as $i |
                if $i == null then 0 else $idxs[$i] end' 2>/dev/null || echo 0)"
        fi
        # Spot price (bluechip-per-other) = R_bluechip / R_other.
        # Weight (in the factory's weighted_sum) = R_bluechip.
        local rb ro
        if [ "$bidx" = "1" ]; then
            rb="$r1"; ro="$r0"
        else
            rb="$r0"; ro="$r1"
        fi
        # bash arithmetic on big-integers; awk-handle scaling.
        local spot
        if [ "$ro" = "0" ] || [ -z "$ro" ]; then
            spot=0
        else
            spot="$(awk -v rb="$rb" -v ro="$ro" 'BEGIN { printf "%.0f", rb*1000000/ro }')"
            # Spot scaled by 1e6 so we keep precision in integer arith.
        fi
        printf "  %s\n    bluechip_idx=%s  R_bluechip=%s  R_other=%s  spot(BC/other)=%s/1e6  block_time_last=%s\n" \
            "$pool" "$bidx" "$rb" "$ro" "$spot" "$t"
        # Accumulate weighted sum. weighted_sum += spot * R_bluechip;
        # total_weight += R_bluechip.
        weighted_sum="$(awk -v s="$weighted_sum" -v p="$spot" -v w="$rb" 'BEGIN { printf "%.0f", s + p*w }')"
        total_weight="$(awk -v s="$total_weight" -v w="$rb" 'BEGIN { printf "%.0f", s + w }')"
    done

    echo ""
    if [ "$total_weight" = "0" ]; then
        echo "no eligible pools sampled"
        return
    fi
    local expected
    expected="$(awk -v ws="$weighted_sum" -v tw="$total_weight" 'BEGIN { printf "%.0f", ws/tw }')"
    local published
    published="$(echo "$oracle" | jq -r '.bluechip_price_cache.last_price')"
    # Published is in PRICE_PRECISION units (1e6, matches our 1e6 spot scale).
    echo "=== expected weighted_spot ≈ $expected (1e6 scale)"
    echo "=== published last_price = $published"
    if [ -n "$expected" ] && [ "$expected" != "0" ] && [ -n "$published" ] && [ "$published" != "0" ]; then
        local delta_bps
        delta_bps="$(awk -v e="$expected" -v p="$published" 'BEGIN { d = (p-e); if (d<0) d=-d; printf "%.0f", d*10000/e }')"
        echo "=== |delta| = ${delta_bps} bps"
        echo ""
        echo "Note: the published value is a TWAP averaged over the [prev_snapshot, current] window."
        echo "When pool reserves have been stable for the full window, expected ≈ published."
        echo "When a pool was swung mid-window, the TWAP lags the spot in proportion to the time slice."
    fi
}

case "$MODE" in
    setup-mock)        shift; setup_mock "$@" ;;
    create-pool)       shift; create_pool "$@" ;;
    propose-allowlist) shift; propose_allowlist "$@" ;;
    apply-allowlist)   shift; apply_allowlist "$@" ;;
    refresh-and-update) shift; refresh_and_update ;;
    observe)           shift; observe ;;
    demo)
        SYMBOL="${2:-MOCK}"
        BC_SEED="${3:-1000000000}"     # 1000 BC default
        MOCK_SEED="${4:-1000000000}"   # 1000 MOCK default
        SUPPLY="${5:-100000000000000}" # 100M MOCK default
        echo "=== demo: $SYMBOL ==="
        setup_mock "$SYMBOL" "$SUPPLY"
        # shellcheck disable=SC1090
        source "$SCRIPT_DIR/$STATE_FILE"
        create_pool "$MOCK_CW20_ADDR" "$BC_SEED" "$MOCK_SEED"
        # shellcheck disable=SC1090
        source "$SCRIPT_DIR/$STATE_FILE"
        propose_allowlist "$MOCK_POOL_ADDR"
        local_wait_target=$(( $(date -u +%s) + 305 ))
        echo ""
        echo "waiting 305s for ADMIN_TIMELOCK_SECONDS to elapse..."
        until [ "$(date -u +%s)" -ge "$local_wait_target" ]; do sleep 30; done
        apply_allowlist "$MOCK_POOL_ADDR"
        refresh_and_update
        echo ""
        observe
        ;;
    *)
        cat >&2 <<EOF
usage: $0 <subcommand> [args]

  setup-mock <symbol> <supply-micro>
  create-pool <mock_addr> <bc-seed> <mock-seed>
  propose-allowlist <pool_addr>
  apply-allowlist <pool_addr>
  refresh-and-update
  observe
  demo <symbol> [bc-seed=1e9] [mock-seed=1e9] [supply=1e14]

example end-to-end:
  bash $0 demo MOCK 1000000000 1000000000
EOF
        exit 1
        ;;
esac
