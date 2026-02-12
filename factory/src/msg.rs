use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Uint128};

use cw20::{Cw20Coin, MinterResponse};

use crate::asset::TokenType;
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolConfigUpdate};
use crate::state::FactoryInstantiate;

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
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
    pub is_standard_pool: Option<bool>,
}

#[cw_serde]
pub enum ExecuteMsg {
    ProposeConfigUpdate {
        config: FactoryInstantiate,
    },
    UpdateConfig {},
    Create {
        pool_msg: CreatePool,
        token_info: CreatorTokenInfo,
    },
    UpdateOraclePrice {},
    ForceRotateOraclePools {},
    UpgradePools {
        new_code_id: u64,
        pool_ids: Option<Vec<u64>>,
        migrate_msg: Binary,
    },
    CancelConfigUpdate {},
    ExecutePoolUpgrade {},
    CancelPoolUpgrade {},
    ContinuePoolUpgrade {},
    UpdatePoolConfig {
        pool_id: u64,
        pool_config: PoolConfigUpdate,
    },
    /// Called by a pool contract when its commit threshold has been crossed.
    /// Triggers the bluechip mint for this pool (only fires once per pool).
    NotifyThresholdCrossed {
        pool_id: u64,
    },
}

#[cw_serde]
pub struct FactoryInstantiateResponse {
    pub factory: FactoryInstantiate,
}

#[cw_serde]
pub struct TokenInstantiateMsg {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub initial_balances: Vec<Cw20Coin>,
    pub mint: Option<MinterResponse>,
}

#[cw_serde]
pub struct CreatorTokenInfo {
    pub name: String,
    pub symbol: String,
    pub decimal: u8,
}
