
#You will need to set cw_base.wasm, cw721_base.wasm, pool.wasm and store them on your local chain.cw20 and cw721 is very easy with just downloading it from the Cosmwasm github
#wget https://github.com/CosmWasm/cw-plus/releases/download/v1.0.1/cw20_base.wasm
#wget https://github.com/CosmWasm/cw-plus/releases/download/v0.18.0/cw721_base.wasm
#then store them on your chain
#bluechipChaind tx wasm store cw20_base.wasm --from alice --gas auto --gas-adjustment 1.3 -y
#bluechipChaind tx wasm store cw721_base.wasm --from alice --gas auto --gas-adjustment 1.3 -y
#pool is easy as well. You would do it the same way you got the factory.wasm 
#RUSTFLAGS="-C link-arg=-s" cargo build --release --target wasm32-unknown-unknown && 
#cp target/wasm32-unknown-unknown/release/pool.wasm pool.wasm && 
#cp target/wasm32-unknown-unknown/release/factory.wasm factory.wasm 
#bluechipChaind tx wasm store pool.wasm --from alice --gas auto --gas-adjustment 1.3 -y
#bluechipChaind tx wasm store factory.wasm --from alice --gas auto --gas-adjustment 1.3 -y
#just make sure you know what order you did these in so you can run bluechipChaind query list-code and know which one belongs to what.


set -e

# Configuration variables - modify these as needed
CHAIN_ID="bluechipChain"
NODE_URL="http://127.0.0.1:26657"
KEYRING_BACKEND="test"  
KEY_NAME="alice"
GAS="auto"
GAS_ADJUSTMENT="1.3"
FEES="5000stake"  # can use -y as well

# Instantiate message configuration
ADMIN_ADDRESS="" #use any wallet address created in chain creation
COMMIT_LIMIT_USD="25000"
ORACLE_ADDR="cosmos1hrpna9v7vs3stzyd4z3xf00676kf78zpe2u5ksvljswn2vnjp3ysawcmtt" #you can make this up for local testing of factory only. use this 
ORACLE_SYMBOL="stake"
TOKEN_ID=1 #must match what your code id for the cw20 above is.
POSITION_NFT_ID=2 #must match what your code id fort the cw20 above is.
PAIR_ID=4 #must match what your code id for the pool above is
BLUECHIP_ADDRESS="$ADMIN_ADDRESS"
BLUECHIP_FEE="0.01"
CREATOR_FEE="0.05"
FACTORY_CODE_ID=5 #must match what your code id for the factory above is


# Instantiate the contract
instantiate_contract() {
    print_status "Instantiating factory contract..."
    
    # Construct the instantiate message
    INSTANTIATE_MSG=$(cat <<EOF
{
    "admin": "$ADMIN_ADDRESS",
    "commit_limit_usd": "$COMMIT_LIMIT_USD",
    "oracle_addr": "$ORACLE_ADDR",
    "oracle_symbol": "$ORACLE_SYMBOL",
    "token_id": $TOKEN_ID,
    "position_nft_id": $POSITION_NFT_ID,
    "pair_id": $PAIR_ID,
    "bluechip_address": "$BLUECHIP_ADDRESS",
    "bluechip_fee": "$BLUECHIP_FEE",
    "creator_fee": "$CREATOR_FEE"
}
EOF
)

    print_status "Instantiate message:"
    echo "$INSTANTIATE_MSG" | jq .
    
    INSTANTIATE_RESULT=$(bluechipChaind tx wasm instantiate "$FACTORY_CODE_ID" "$INSTANTIATE_MSG" \
        --label "factory" \
        --admin "$ADMIN_ADDRESS" \
        --from "$KEY_NAME" \
        --chain-id "$CHAIN_ID" \
        --node "$NODE_URL" \
        --keyring-backend "$KEYRING_BACKEND" \
        --gas "$GAS" \
        --gas-adjustment "$GAS_ADJUSTMENT" \
        --fees "$FEES" \
        --output json \
        --yes)
    
    if [ $? -eq 0 ]; then
     TXHASH=$(echo "$INSTANTIATE_RESULT" | jq -r '.txhash')
        print_status "Contract instantiated successfully. Transaction hash: $TXHASH"
        
        # Wait for transaction to be included in block
        sleep 6
        
        # Query contracts by code ID to get the contract address
        CONTRACT_ADDRESS=$(bluechipChaind query wasm list-contract-by-code "$FACTORY_CODE_ID" --node "$NODE_URL" --output json | jq -r '.contracts[-1]')
        
        if [ "$CONTRACT_ADDRESS" != "null" ] && [ -n "$CONTRACT_ADDRESS" ]; then
            print_status "Contract Address: $CONTRACT_ADDRESS"
            echo "$CONTRACT_ADDRESS" > contract_address.txt

cat <<EOF > contract_address.json
{
    "contract_address": "$CONTRACT_ADDRESS"
}
EOF
            
        else
            print_error "Failed to extract contract address from transaction result"
            exit 1
        fi
    else
        print_error "Failed to instantiate contract"
        exit 1
    fi
}

# Query contract info to verify deployment
verify_deployment() {
    print_status "Verifying deployment..."
    
    # Query contract info
    CONTRACT_INFO=$(bluechipChaind query wasm contract "$CONTRACT_ADDRESS" --node "$NODE_URL" --output json)
    print_status "Contract Info:"
    echo "$CONTRACT_INFO" | jq .

}
