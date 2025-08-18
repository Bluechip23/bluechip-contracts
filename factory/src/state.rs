use crate::pair::CreatePool;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};

pub const CONFIG: Item<FactoryInstantiate> = Item::new("config");
pub const TEMPPAIRINFO: Item<CreatePool> = Item::new("temp_pair");
pub const TEMPCREATOR: Item<Addr> = Item::new("temp_admin");
pub const TEMPPOOLID: Item<u64> = Item::new("temp_pool_id");
pub const TEMPTOKENADDR: Item<Addr> = Item::new("temp_token_addr");
pub const TEMPNFTADDR: Item<Addr> = Item::new("temp_nft_addr");
pub const COMMIT: Map<&str, CommitInfo> = Map::new("commit_info");
pub const NEXT_POOL_ID: Item<u64> = Item::new("next_pool_id");
pub const POOLS_BY_ID: Map<u64, CommitInfo> = Map::new("pools_by_id");
pub const CREATION_STATES: Map<u64, CreationState> = Map::new("creation_states");

#[cw_serde]
pub struct FactoryInstantiate {
    //admin of the factory - will be bluechip or some multisig or something along those lines. person who can edit effectively
    pub admin: Addr,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    //CW20 contract id that is store on the chain for the pool to use when minting new NFTs
    pub token_id: u64,
    //nft contract id that is store on the chain for the pool to use when minting new NFTs
    pub position_nft_id: u64,
    //id for the token pair that exists in the pool. Used for queries mostly.
    pub pair_id: u64,
    pub bluechip_address: Addr,
    pub bluechipe_fee: Decimal,
    pub creator_fee: Decimal,
}
#[cw_serde]
pub struct CommitInfo {
    pub pool_id: u64,
    pub creator: Addr,
    pub token_addr: Addr,
    pub pool_addr: Addr,
}

#[cw_serde]
pub struct CreationState {
    pub pool_id: u64,
    pub creator: Addr,
    pub token_address: Option<Addr>,
    pub nft_address: Option<Addr>,
    pub pool_address: Option<Addr>,
    pub creation_time: Timestamp,
    pub status: CreationStatus,
    pub retry_count: u8, // Track retries
}

#[cw_serde]
pub enum CreationStatus {
    Started,
    TokenCreated,
    NftCreated,
    PoolCreated,
    Completed,
    Failed,
    CleaningUp,
}
