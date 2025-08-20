use cosmwasm_schema::{cw_serde,};

use crate::asset::{ AssetInfo, };
use cosmwasm_std::{Addr, Binary, Decimal, Uint128};

#[cw_serde]
pub struct PoolInstantiateMsg {
    pub pool_id: u64,
    // Information about the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    // The token contract code ID used for the tokens in the pool
    pub token_code_id: u64,
    // The factory contract address
    pub factory_addr: Addr,
    // gets set in reply function - amounts that go to each payout party
    pub threshold_payout: Option<Binary>,
    pub fee_info: FeeInfo,
    pub commit_limit_usd: Uint128,
    pub commit_amount_for_threshold: Uint128,
    pub position_nft_address: Addr,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
}

#[cw_serde]
pub struct FeeInfo {
    //BlueChip wallet
    pub bluechip_address: Addr,
    //pool creatpr wallet
    pub creator_address: Addr,
    //amount of commit that goes to BlueChip
    pub bluechip_fee: Decimal,
    //amount of commit taht goes to pool creator
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct ConfigResponse {
    // Last timestamp when the cumulative prices in the pool were updated
    pub block_time_last: u64,
    // The pool's parameters
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct MigrateMsg {}
