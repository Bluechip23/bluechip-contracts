use crate::pair::CreatePool;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};


//states used during the pool and factory creation process. 
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
    //amount of bluechip used to seed the creator pool when threshold is crossed
    pub commit_amount_for_threshold: Uint128,
    //threshold limit priced in USD 
    pub commit_limit_usd: Uint128,
    //oracle contract address to track usd price
    pub oracle_addr: Addr,
    //symbol used to track pricing
    pub oracle_symbol: String,
    //CW20 contract id that is store on the chain for the pool to use when minting new NFTs
    pub token_id: u64,
    //nft contract id that is store on the chain for the pool to use when minting new NFTs
    pub position_nft_id: u64,
    //id for the token pair that exists in the pool. Used for queries mostly.
    pub pair_id: u64,
    //bluechip wallet address
    pub bluechip_address: Addr,
    //fee distributed to bluechip every commit
    pub bluechip_fee: Decimal,
    //fee distributed to creator ever transaction
    pub creator_fee: Decimal,
}
#[cw_serde]
pub struct CommitInfo {
    //id of pool - will be some positive integer
    pub pool_id: u64,
    //
    pub creator: Addr,
    //address of the creator token itself
    pub token_addr: Addr,
    //creator pool address
    pub pool_addr: Addr,
}

//used to track the state of the pool throughout creation. Will trigger different events upon partial or complete creation
#[cw_serde]
pub struct CreationState {
    //tracking pool by id
    pub pool_id: u64,
    //creator of the pool
    pub creator: Addr,
    //token address of the pool
    pub token_address: Option<Addr>,
    pub nft_address: Option<Addr>,
    //pool address
    pub pool_address: Option<Addr>,
    pub creation_time: Timestamp,
    //creation status triggers different outcomes based on completion of different status updates
    pub status: CreationStatus,
    pub retry_count: u8, // Track retries
}

//different "stages" of the creation process
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
