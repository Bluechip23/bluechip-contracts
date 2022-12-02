# WasmSwap

This contract is an automatic market maker (AMM) heavily inspired by Uniswap v1 for the cosmwasm smart contract engine.

# Instantiation

The contract can be instantiated with the following messages

```
{
    "token1_denom": {"native": "<DENOM>"},
    "token2_denom": {"cw20": "<CONTRACT_ADDRESS>"},
    "lp_token_code_id": '<CW20_CODE_ID>'
}
```

# Messages

### Add Liquidity

Allows a user to add liquidity to the pool.

### Remove Liquidity

Allows a user to remove liquidity from the pool.

### Swap

Swap one asset for the other

### Pass Through Swap

Execute a multi contract swap where A is swapped for B and then B is sent to another contract where it is swapped for C.

### Swap And Send To

Execute a swap and send the new asset to the given recipient. This is mostly used for `PassThroughSwaps`.
