#!/bin/bash
set -e

echo "🔨 Rebuilding pool contract..."
cd /home/jeremy/snap/smartcontracts/bluechip-contracts
RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/pool.wasm artifacts/pool.wasm

echo "✅ Pool contract rebuilt!"
echo ""
echo "Now run: ./deploy_pool_threshold_crossed.sh"
