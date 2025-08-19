use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Uint128};

use cw20::{Cw20Coin, MinterResponse};

use crate::asset::AssetInfo;
use crate::pair::{FeeInfo, CreatePool};
use crate::state::{FactoryInstantiate};

#[cw_serde]
pub struct CreatePoolReplyMsg {
    pub pool_id: u64,
    // Information about the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    // The token contract code ID used for the tokens in the pool
    pub token_code_id: u64,
    // The factory contract address
    pub factory_addr: Addr,
    pub threshold_payout: Option<Binary>,
    //fees to bluechip and creator
    pub fee_info: FeeInfo,
    pub commit_limit_usd: Uint128,
    pub commit_amount_for_threshold: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
    //address called by the pool to mint new liquidity position NFTs.
    pub position_nft_address: Addr,
}

#[cw_serde]
pub enum ExecuteMsg {
    UpdateConfig {
        config: FactoryInstantiate,
    },
    Create {
        pool_msg: CreatePool,
        token_info: TokenInfo,
    },
}

#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct ConfigResponse {
    pub config: FactoryInstantiate,
}



#[cw_serde]
pub struct TokenInstantiateMsg {
   // Token name
    pub name: String,
    // Token symbol
    pub symbol: String,
    // The amount of decimals the token has
    pub decimals: u8,
    //Initial token balances
    pub initial_balances: Vec<Cw20Coin>,
    //token minting information
    pub mint: Option<MinterResponse>,
}

#[cw_serde]
pub struct TokenInfo {
    //name of creator token - set by creator
    pub name: String,
    //symbol of creator token - set by
    pub symbol: String,
    // number of decimals 100000000 =$1 for 8 decimals
    pub decimal: u8,
}
