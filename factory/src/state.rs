use crate::pair::PairInstantiateMsg;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Uint128};
use cw_storage_plus::{Item, Map};

pub const CONFIG: Item<Config> = Item::new("config");
pub const TEMPPAIRINFO: Item<PairInstantiateMsg> = Item::new("temp_pair");
pub const TEMPCREATOR: Item<Addr> = Item::new("temp_admin");
pub const TEMPPOOLID: Item<u64> = Item::new("temp_pool_id");
pub const TEMPTOKENADDR: Item<Addr> = Item::new("temp_token_addr");
pub const TEMPNFTADDR: Item<Addr> = Item::new("temp_nft_addr");
pub const SUBSCRIBE: Map<&str, SubscribeInfo> = Map::new("subscription_info");
pub const NEXT_POOL_ID: Item<u64> = Item::new("next_pool_id");
pub const POOLS_BY_ID: Map<u64, SubscribeInfo> = Map::new("pools_by_id");

#[cw_serde]
pub struct Config {
    pub admin: Addr,
    pub commit_limit: Uint128,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_id: u64,
    pub position_nft_id: u64,
    pub pair_id: u64,
    pub bluechip_address: Addr,
    pub bluechipe_fee: Decimal,
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct SubscribeInfo {
    pub pool_id: u64,
    pub creator: Addr,
    pub token_addr: Addr,
    pub pool_addr: Addr,
}
