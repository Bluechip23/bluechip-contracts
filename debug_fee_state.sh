#!/bin/bash
POOL_ADDR="cosmos1g3cgv3z2smd7vrjr3s639ewnwyd5dz4lg0uy9fadhkumca4suqcqzrdgdu"

echo "🔍 Querying Pool State..."
bluechipChaind query wasm contract-state smart $POOL_ADDR '{"pool_state":{}}'

echo "🔍 Querying Fee State..."
bluechipChaind query wasm contract-state smart $POOL_ADDR '{"fee_state":{}}'
