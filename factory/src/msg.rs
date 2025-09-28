use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Uint128};

use cw20::{Cw20Coin, MinterResponse};

use crate::asset::TokenType;
use crate::pool_struct::{CommitFeeInfo, CreatePool};
use crate::state::{FactoryInstantiate};


//triggers inside factory reply, used to complete the pool creation process. 
#[cw_serde]
pub struct CreatePoolReplyMsg {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    // The token contract code ID used for the tokens in the pool
    pub cw20_token_contract_id: u64,
    pub used_factory_addr: Addr,
    //gets populated inside reply
    pub threshold_payout: Option<Binary>,
    //fees to bluechip and creator
    pub commit_fee_info: CommitFeeInfo,
    pub commit_threshold_limit_usd: Uint128,
    pub commit_amount_for_threshold: Uint128,
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
        token_info: CreatorTokenInfo,
    },
}

#[cw_serde]
pub struct FactoryInstantiateResponse {
    pub factory: FactoryInstantiate,
}

#[cw_serde]
pub struct TokenInstantiateMsg {
    pub token_name: String,
    pub ticker: String,
    pub decimals: u8,
    pub initial_balances: Vec<Cw20Coin>,
    pub mint: Option<MinterResponse>,
}

#[cw_serde]
pub struct CreatorTokenInfo {
    pub token_name: String,
    pub ticker: String,
    pub decimal: u8,
}
