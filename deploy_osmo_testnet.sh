#!/usr/bin/env bash
# =====================================================================
# osmo-test-5 deploy — Step 1: upload wasms + instantiate factory
# =====================================================================
# Smallest viable slice of the testnet rollout. After this completes
# you have:
#   - Tokenfactory bluechip denom minted to the deployer
#   - All custom wasms uploaded with code IDs captured
#   - expand-economy instantiated + funded
#   - Factory instantiated, pointing at real Pyth + tokenfactory
#     bluechip + uosmo as the ATOM-side denom
#
# What this script intentionally does NOT do (next slices):
#   - Anchor standard pool creation + SetAnchorPool one-shot bootstrap
#   - VAA pusher loop (factory will register stale-Pyth errors until
#     the pusher is running, which is fine for step 1)
#   - Commit pool creation / threshold crossing
#   - Any test scenarios (TWAP, rotation, staleness, swing)
#
# Prerequisites:
#   1. osmosisd installed and on PATH
#   2. jq and curl installed
#   3. A key named per ${FROM} in the local keyring with osmo-test-5
#      gas balance (see osmo_testnet.env: MIN_GAS_BALANCE)
#   4. Custom wasms built via `make build` and present in ${ARTIFACTS}/
#      (creator_pool.wasm, standard_pool.wasm, factory.wasm,
#      expand_economy.wasm, cw20_base.wasm, cw721_base.wasm)
#
# Re-running: not idempotent. Each run uploads fresh wasms and
# instantiates fresh contracts; ${STATE_FILE} gets overwritten with
# the new addresses. Tokenfactory create-denom is the one step we
# guard against re-running (it errors if the denom already exists),
# but mint will run again — expected, since you may want to top up
# the deployer balance between runs.
# =====================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/osmo_testnet.env"

# ---- Sanity ---------------------------------------------------------
for cmd in osmosisd jq curl; do
    command -v "$cmd" >/dev/null \
        || { echo "error: $cmd not found on PATH" >&2; exit 1; }
done

if ! osmosisd keys show "$FROM" --keyring-backend "$KEYRING" >/dev/null 2>&1; then
    echo "error: key '$FROM' not found in keyring '$KEYRING'" >&2
    echo "       create one with: osmosisd keys add $FROM --keyring-backend $KEYRING" >&2
    exit 1
fi
ADDR="$(osmosisd keys show "$FROM" -a --keyring-backend "$KEYRING")"

# Ping the node + check gas balance.
BAL_JSON="$(osmosisd query bank balances "$ADDR" --node "$NODE" -o json 2>/dev/null \
    || { echo "error: cannot reach $NODE" >&2; exit 1; })"
GAS_BAL="$(echo "$BAL_JSON" | jq -r --arg d "$NATIVE_DENOM" \
    '.balances[]? | select(.denom == $d) | .amount' || echo 0)"
[ -z "$GAS_BAL" ] && GAS_BAL=0
if [ "$GAS_BAL" -lt "$MIN_GAS_BALANCE" ]; then
    echo "error: $ADDR has only $GAS_BAL u$NATIVE_DENOM, need >= $MIN_GAS_BALANCE" >&2
    echo "       faucet: https://faucet.testnet.osmosis.zone/" >&2
    exit 1
fi

echo "deployer:        $ADDR"
echo "gas balance:     $GAS_BAL u$NATIVE_DENOM (>= $MIN_GAS_BALANCE required)"
echo "chain:           $CHAIN_ID via $NODE"
echo ""

# ---- Tx helpers -----------------------------------------------------
# Common flags. -y skips confirmation; -o json keeps everything
# parseable. We don't pass --gas-prices to estimate (osmosisd uses it
# to convert gas-units → fee at submit time).
TX_FLAGS=(
    --chain-id "$CHAIN_ID"
    --node "$NODE"
    --keyring-backend "$KEYRING"
    --from "$FROM"
    --gas auto
    --gas-adjustment "$GAS_ADJUSTMENT"
    --gas-prices "$GAS_PRICES"
    -y -o json
)

# Submit a tx, wait for inclusion, fail on non-zero code.
# stdout: full tx-result JSON
# stderr: status messages
submit_tx() {
    local raw
    raw="$(osmosisd tx "$@" "${TX_FLAGS[@]}" 2>/dev/null)" \
        || { echo "error: tx submit (mempool admission) failed for: $*" >&2; return 1; }
    local tx_hash
    tx_hash="$(echo "$raw" | jq -r '.txhash // empty')"
    if [ -z "$tx_hash" ]; then
        echo "error: tx submit returned no hash. raw output:" >&2
        echo "$raw" >&2
        return 1
    fi
    # Poll for inclusion. 6s block time + indexing latency → try for ~30s.
    local i result code
    for i in 1 2 3 4 5 6; do
        sleep 5
        if result="$(osmosisd query tx "$tx_hash" --node "$NODE" -o json 2>/dev/null)"; then
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

# Extract the value of a typed event attribute from a tx-result JSON.
# usage: extract_attr <tx-json> <event-type> <attr-key>
extract_attr() {
    local json="$1" type="$2" key="$3"
    echo "$json" | jq -r --arg t "$type" --arg k "$key" '
        .events[] | select(.type == $t) | .attributes[]
        | select(.key == $k) | .value' | head -n 1
}

# ---- 1. Tokenfactory ubluechip denom -------------------------------
BLUECHIP_DENOM="factory/${ADDR}/${BLUECHIP_SUBDENOM}"
echo "[1/4] tokenfactory $BLUECHIP_DENOM"

# Idempotent guard: query existing denoms-from-creator, only create if
# this subdenom isn't already registered. Mint always runs (a re-run
# is treated as "top up the deployer balance").
EXISTING_DENOMS="$(osmosisd query tokenfactory denoms-from-creator "$ADDR" \
    --node "$NODE" -o json 2>/dev/null | jq -r '.denoms[]?' || true)"
if echo "$EXISTING_DENOMS" | grep -qx "$BLUECHIP_DENOM"; then
    echo "      denom exists, skipping create-denom"
else
    submit_tx tokenfactory create-denom "$BLUECHIP_SUBDENOM" >/dev/null
    echo "      created"
fi

echo "      minting $BLUECHIP_INITIAL_MINT $BLUECHIP_DENOM → $ADDR"
submit_tx tokenfactory mint "${BLUECHIP_INITIAL_MINT}${BLUECHIP_DENOM}" >/dev/null

# ---- 2. Upload wasms -----------------------------------------------
echo ""
echo "[2/4] uploading wasms"

upload_wasm() {
    local file="$1" label="$2"
    local path="$ARTIFACTS/$file"
    if [ ! -f "$path" ]; then
        echo "error: missing $path — run \`make build\` first" >&2
        return 1
    fi
    local result code_id
    result="$(submit_tx wasm store "$path")"
    code_id="$(extract_attr "$result" store_code code_id)"
    if [ -z "$code_id" ] || [ "$code_id" = "null" ]; then
        echo "error: could not extract code_id for $file" >&2
        return 1
    fi
    printf "      %-22s code_id=%s\n" "$label" "$code_id" >&2
    echo "$code_id"
}

CW20_CODE_ID="$(upload_wasm cw20_base.wasm 'CW20 base')"
CW721_CODE_ID="$(upload_wasm cw721_base.wasm 'CW721 base')"
FACTORY_CODE_ID="$(upload_wasm factory.wasm 'Factory')"
CREATOR_POOL_CODE_ID="$(upload_wasm creator_pool.wasm 'Creator pool')"
STANDARD_POOL_CODE_ID="$(upload_wasm standard_pool.wasm 'Standard pool')"
EXPAND_ECONOMY_CODE_ID="$(upload_wasm expand_economy.wasm 'Expand economy')"

# ---- 3. Instantiate expand-economy ---------------------------------
echo ""
echo "[3/4] instantiate expand-economy"

EXPAND_INIT="$(jq -nc \
    --arg factory "$ADDR" \
    --arg owner "$ADDR" \
    '{factory_address: $factory, owner: $owner}')"

# --no-admin: we never want to migrate expand-economy from a key.
EXPAND_RESULT="$(submit_tx wasm instantiate "$EXPAND_ECONOMY_CODE_ID" "$EXPAND_INIT" \
    --label "bluechip_expand_economy" --no-admin)"
EXPAND_ECONOMY_ADDR="$(extract_attr "$EXPAND_RESULT" instantiate _contract_address)"
echo "      expand-economy: $EXPAND_ECONOMY_ADDR"

echo "      funding with $EXPAND_ECONOMY_FUND $BLUECHIP_DENOM"
submit_tx bank send "$ADDR" "$EXPAND_ECONOMY_ADDR" \
    "${EXPAND_ECONOMY_FUND}${BLUECHIP_DENOM}" >/dev/null

# ---- 4. Instantiate factory ----------------------------------------
echo ""
echo "[4/4] instantiate factory"

# Build init message via jq. atom_bluechip_anchor_pool_address is a
# placeholder (deployer addr) until SetAnchorPool fires in the next
# script slice — the field type is Addr so it has to be a valid bech32,
# but it isn't read by anything until the anchor pool is created and
# the one-shot SetAnchorPool overwrites it.
FACTORY_INIT="$(jq -nc \
    --arg admin               "$ADDR" \
    --arg threshold_usd       "$COMMIT_THRESHOLD_LIMIT_USD" \
    --arg pyth_addr           "$PYTH_CONTRACT" \
    --arg pyth_feed           "$PYTH_OSMO_USD_FEED_ID" \
    --arg cw721_id            "$CW721_CODE_ID" \
    --arg cw20_id             "$CW20_CODE_ID" \
    --arg creator_pool_id     "$CREATOR_POOL_CODE_ID" \
    --arg standard_pool_id    "$STANDARD_POOL_CODE_ID" \
    --arg wallet              "$ADDR" \
    --arg fee_bc              "$COMMIT_FEE_BLUECHIP" \
    --arg fee_cr              "$COMMIT_FEE_CREATOR" \
    --arg max_lock            "$MAX_BLUECHIP_LOCK_PER_POOL" \
    --arg lock_days           "$CREATOR_EXCESS_LIQUIDITY_LOCK_DAYS" \
    --arg expand              "$EXPAND_ECONOMY_ADDR" \
    --arg blue_denom          "$BLUECHIP_DENOM" \
    --arg atom_denom          "$NATIVE_DENOM" \
    --arg std_fee             "$STANDARD_POOL_CREATION_FEE_USD" \
    --arg anchor_placeholder  "$ADDR" \
    '{
        factory_admin_address:               $admin,
        commit_threshold_limit_usd:          $threshold_usd,
        pyth_contract_addr_for_conversions:  $pyth_addr,
        pyth_atom_usd_price_feed_id:         $pyth_feed,
        cw721_nft_contract_id:               ($cw721_id            | tonumber),
        cw20_token_contract_id:              ($cw20_id             | tonumber),
        create_pool_wasm_contract_id:        ($creator_pool_id     | tonumber),
        standard_pool_wasm_contract_id:      ($standard_pool_id    | tonumber),
        bluechip_wallet_address:             $wallet,
        commit_fee_bluechip:                 $fee_bc,
        commit_fee_creator:                  $fee_cr,
        max_bluechip_lock_per_pool:          $max_lock,
        creator_excess_liquidity_lock_days:  ($lock_days           | tonumber),
        atom_bluechip_anchor_pool_address:   $anchor_placeholder,
        bluechip_mint_contract_address:      $expand,
        bluechip_denom:                      $blue_denom,
        atom_denom:                          $atom_denom,
        standard_pool_creation_fee_usd:      $std_fee
    }')"

# --admin: factory keeps an admin so we can migrate later (e.g. to
# pull in the ForceRefreshEligibleSnapshot addition before commit-pool
# rotation tests). Once we're on the test-final wasm we can clear it.
FACTORY_RESULT="$(submit_tx wasm instantiate "$FACTORY_CODE_ID" "$FACTORY_INIT" \
    --label "bluechip_factory" --admin "$ADDR")"
FACTORY_ADDR="$(extract_attr "$FACTORY_RESULT" instantiate _contract_address)"
echo "      factory:        $FACTORY_ADDR"

# ---- Write state file ----------------------------------------------
cat > "$SCRIPT_DIR/$STATE_FILE" <<EOF
# Auto-generated by deploy_osmo_testnet.sh — DO NOT EDIT MANUALLY
# Sourced by subsequent test-scenario scripts alongside osmo_testnet.env.
ADMIN_ADDR="$ADDR"
BLUECHIP_DENOM="$BLUECHIP_DENOM"
CW20_CODE_ID="$CW20_CODE_ID"
CW721_CODE_ID="$CW721_CODE_ID"
FACTORY_CODE_ID="$FACTORY_CODE_ID"
CREATOR_POOL_CODE_ID="$CREATOR_POOL_CODE_ID"
STANDARD_POOL_CODE_ID="$STANDARD_POOL_CODE_ID"
EXPAND_ECONOMY_CODE_ID="$EXPAND_ECONOMY_CODE_ID"
FACTORY_ADDR="$FACTORY_ADDR"
EXPAND_ECONOMY_ADDR="$EXPAND_ECONOMY_ADDR"
EOF

echo ""
echo "=================================================="
echo "deploy step 1 complete"
echo "=================================================="
echo "factory:         $FACTORY_ADDR"
echo "expand-economy:  $EXPAND_ECONOMY_ADDR"
echo "bluechip denom:  $BLUECHIP_DENOM"
echo ""
echo "state written:   $SCRIPT_DIR/$STATE_FILE"
echo ""
echo "NEXT: anchor standard pool creation + SetAnchorPool one-shot."
echo "      The factory's USD↔bluechip oracle path will fail until"
echo "      the anchor pool exists AND a Pyth update has landed."
