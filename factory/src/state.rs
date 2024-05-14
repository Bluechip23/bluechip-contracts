use crate::pair::InstantiateMsg as PairInstantiateMsg;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Uint128};
use cw_storage_plus::{Item, Map};

pub const CONFIG: Item<Config> = Item::new("config");
pub const TEMPPAIRINFO: Item<PairInstantiateMsg> = Item::new("temp_pair");
pub const TEMPCREATOR: Item<Addr> = Item::new("temp_admin");
pub const TEMPTOKENADDR: Item<Addr> = Item::new("temp_token_addr");
pub const SUBSCRIBE: Map<&str, SubscribeInfo> = Map::new("subscription_info");
#[cw_serde]
pub struct Config {
    pub admin: Addr,
    pub total_token_amount: Uint128,
    pub creator_amount: Uint128,
    pub pool_amount: Uint128,
    pub commit_amount: Uint128,
    pub bluechip_amount: Uint128,
    pub token_id: u64,
    pub pair_id: u64,
    pub bluechip_address: Addr,
    pub bluechipe_fee: Decimal,
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct SubscribeInfo {
    pub creator: Addr,
    pub token_addr: Addr,
    pub pool_addr: Addr,
}
