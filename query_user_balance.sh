#!/bin/bash
TOKEN_ADDR="cosmos1quce89l8clsn8s5tmq5sylg370h58xfnkwadx72crjv90jmetp4sjv3h8m"
USER_ADDR="cosmos1zdnw3vdn4ekaxyjg29tq5djl7la6l3432c4pm6"

echo "🔍 Querying User Balance on CW20 Contract..."
bluechipChaind query wasm contract-state smart $TOKEN_ADDR '{"balance":{"address":"'$USER_ADDR'"}}'
