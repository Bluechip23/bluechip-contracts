#!/usr/bin/env bash
# =====================================================================
# test_swing.sh — trigger the 30% TWAP drift breaker on a target pool
# =====================================================================
# Slice 6. The objective: drive a single oversized swap on a thinly-
# funded pool so reserves move >30% in one block, then verify the
# next UpdateOraclePrice round either rejects the drift (steady-state
# branch (a)) or buffers it as a candidate (post-reset branch (c) /
# bootstrap branch (d)).
#
# Subcommands:
#   query <pool_addr>          Show pool reserves + suggest a swap
#                              size that crosses 30% drift.
#   swap <pool_addr> <amount>  Execute a SimpleSwap on the pool with
#                              <amount> BLUECHIP_DENOM as the offer
#                              asset. Uses --amount and the canonical
#                              bluechip side as the funded leg.
#                              max_spread=0.99 + allow_high_max_spread=true
#                              so the pool's spread cap doesn't reject
#                              the trade itself — we want the swap
#                              to LAND so the breaker has something to
#                              react to.
#   observe <pool_addr>        Read pool.pool_state and show the
#                              accumulator + reserve pre/post snapshot,
#                              plus current factory.GetBluechipUsdPrice.
#
# Operator workflow:
#   1. PICK A POOL THAT IS NOT THE ANCHOR. The anchor needs to stay
#      healthy through bootstrap + TWAP + staleness tests. Use one of
#      the commit pools created in slice 4 (commit_pools.txt has them).
#      Verify it's threshold-crossed (cross_threshold.sh already ran)
#      and oracle-eligible (apply_oracle_eligibility.sh apply ran).
#
#   2. Optional: thin out the pool's bluechip side. The 30% threshold
#      bites on the SAMPLED reserves at oracle-update time. With a
#      tiny pool, an attacker-sized swap (or you, in this test) can
#      shove it past 30% trivially. Specifically aim for the pool's
#      bluechip-side reserve to be near — but above —
#      MIN_POOL_LIQUIDITY_FALLBACK_BLUECHIP_PER_SIDE (5_000 BC). Below
#      that and M-4's per-side floor drops the pool from the eligible
#      set entirely, defeating the test.
#
#   3. bash test_swing.sh query <pool_addr>
#      Reads reserves and suggests an amount that pushes drift past
#      30%. For an xyk pool with reserves (R, R'), a swap of x in
#      moves the spot price by roughly (1 + x/R) factor; >30% drift
#      needs x ≈ 0.3 * R on the offer side.
#
#   4. bash test_swing.sh swap <pool_addr> <amount>
#      Submits the SimpleSwap. Pool's spread cap will permit it
#      because we pass allow_high_max_spread=true.
#
#   5. bash test_swing.sh observe <pool_addr>
#      Captures post-swap state. Then wait UPDATE_INTERVAL (300s) and
#      drive the next oracle update:
#        bash test_twap_advance.sh update
#      Then re-observe + check rotation state. The breaker either:
#        - rejected the new TWAP (branch (a) on steady-state); the
#          oracle's last_price stays put and the bad observation is
#          dropped from the rolling window. Look for
#          `breaker_branch_a_rejected_drift` style attributes in the
#          tx events.
#        - buffered the drift as a candidate (branches (b)/(c)/(d)
#          during warm-up windows). pending_first_price gets set;
#          confirm with test_bootstrap.sh query.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/_osmo_tx_helpers.sh"
require_state

usage() {
    cat >&2 <<EOF
usage:
  $0 query <pool_addr>
  $0 swap <pool_addr> <amount-micro-bluechip>
  $0 observe <pool_addr>

example:
  $0 query osmo1...pool_addr
  $0 swap osmo1...pool_addr 2000000000
  $0 observe osmo1...pool_addr
EOF
    exit 1
}

MODE="${1:-}"
POOL_ADDR="${2:-}"
[ -z "$MODE" ] && usage
[ -z "$POOL_ADDR" ] && usage

# Resolve the bluechip-side reserve (which side is bluechip depends on
# pool creation order). pool.pair returns the asset list in canonical
# order; pool.pool_state returns reserves in matching order.
resolve_pool_state() {
    local pair pool_state
    pair="$(query_smart "$POOL_ADDR" '{"pair":{}}')"
    pool_state="$(query_smart "$POOL_ADDR" '{"pool_state":{}}')"

    # Find which index holds the canonical bluechip denom.
    local bluechip_idx
    bluechip_idx="$(echo "$pair" | jq -r --arg d "$BLUECHIP_DENOM" '
        (.pool_token_info // .asset_infos // []) |
        to_entries[] | select(.value.bluechip.denom == $d) | .key' 2>/dev/null | head -n 1)"
    if [ -z "$bluechip_idx" ]; then
        echo "error: could not find $BLUECHIP_DENOM in pool's pair info" >&2
        echo "$pair" | jq .
        return 1
    fi

    echo "=== pool.pair ==="
    echo "$pair" | jq .
    echo ""
    echo "=== pool.pool_state ==="
    echo "$pool_state" | jq .
    echo ""
    echo "bluechip side index: $bluechip_idx"

    BLUECHIP_RESERVE="$(echo "$pool_state" | jq -r --argjson i "$bluechip_idx" '
        if $i == 0 then .reserve0 else .reserve1 end' 2>/dev/null)"
    echo "bluechip reserve:    $BLUECHIP_RESERVE"
}

case "$MODE" in
    query)
        resolve_pool_state
        echo ""
        # Suggest a swap size: ~35% of the bluechip-side reserve so
        # post-swap drift comfortably clears the 30% breaker threshold.
        # 35% gives 5pp of slack against the keeper sampling exactly
        # the pre-swap or post-swap state.
        if [ -n "${BLUECHIP_RESERVE:-}" ] && [ "$BLUECHIP_RESERVE" -gt 0 ]; then
            SUGGEST=$(( BLUECHIP_RESERVE * 35 / 100 ))
            echo "=== suggested swap (35% of bluechip reserve) ==="
            echo "  bash $0 swap $POOL_ADDR $SUGGEST"
            echo ""
            echo "  This pushes spot price by ~35% which exceeds the"
            echo "  MAX_TWAP_DRIFT_BPS = 3000 (30%) breaker threshold"
            echo "  on the next UpdateOraclePrice round."
        fi
        ;;
    swap)
        AMOUNT="${3:-}"
        [ -z "$AMOUNT" ] && usage
        echo "executing simple_swap with allow_high_max_spread=true"
        echo "offer: $AMOUNT $BLUECHIP_DENOM → $POOL_ADDR"
        echo ""
        SWAP_MSG="$(jq -nc \
            --arg blue "$BLUECHIP_DENOM" \
            --arg amt  "$AMOUNT" \
            '{simple_swap:{
                offer_asset:{
                    info:{bluechip:{denom:$blue}},
                    amount:$amt
                },
                belief_price:null,
                max_spread:"0.10",
                allow_high_max_spread:true,
                to:null,
                transaction_deadline:null
            }}')"
        RESULT="$(submit_tx wasm execute "$POOL_ADDR" "$SWAP_MSG" \
            --amount "${AMOUNT}${BLUECHIP_DENOM}")"
        echo "OK — tx $(echo "$RESULT" | jq -r '.txhash')"
        RETURN_AMOUNT="$(extract_attr "$RESULT" wasm return_amount)"
        SPREAD_AMOUNT="$(extract_attr "$RESULT" wasm spread_amount)"
        echo "received: ${RETURN_AMOUNT:-?} (spread: ${SPREAD_AMOUNT:-?})"
        echo ""
        echo "NEXT:"
        echo "  1. wait UPDATE_INTERVAL (300s) for the keeper-update cooldown"
        echo "  2. fire it manually:  bash test_twap_advance.sh update"
        echo "  3. observe breaker:   bash $0 observe $POOL_ADDR"
        ;;
    observe)
        resolve_pool_state
        echo ""
        echo "=== factory.GetBluechipUsdPrice ==="
        if PRICE_RESP="$(query_smart "$FACTORY_ADDR" \
            '{"internal_blue_chip_oracle_query":{"get_bluechip_usd_price":{}}}' 2>&1)"; then
            echo "$PRICE_RESP" | jq .
        else
            echo "(query failed: $PRICE_RESP)"
        fi
        echo ""
        echo "=== internal_oracle.bluechip_price_cache (raw storage) ==="
        ORACLE="$(query_raw_storage "$FACTORY_ADDR" 'internal_oracle')"
        if [ -n "$ORACLE" ]; then
            echo "$ORACLE" | jq '{
                last_price: .bluechip_price_cache.last_price,
                last_update: .bluechip_price_cache.last_update,
                observation_count: (.bluechip_price_cache.twap_observations | length),
                pending_first_price,
                warmup_remaining,
                consecutive_failures
            }'
        fi
        echo ""
        echo "=== pending_bootstrap_price (raw storage) ==="
        PENDING="$(query_raw_storage "$FACTORY_ADDR" 'pending_bootstrap_price')"
        if [ -n "$PENDING" ]; then
            echo "$PENDING" | jq .
        else
            echo "(no pending bootstrap candidate — already published or never reached)"
        fi
        ;;
    *)
        usage
        ;;
esac
