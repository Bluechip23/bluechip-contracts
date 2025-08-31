use crate::pool::CreatePool;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};

//states used during the pool and factory creation process. 
//there is TEMPdata to keep the creation process going, TEMPdata is eventually removed after sucessful pool creation
pub const FACTORYINSTANTIATEINFO: Item<FactoryInstantiate> = Item::new("config");
pub const TEMPPOOLINFO: Item<CreatePool> = Item::new("temp_pool_info");
pub const TEMPCREATORWALLETADDR: Item<Addr> = Item::new("temp_admin");
pub const TEMPPOOLID: Item<u64> = Item::new("temp_pool_id");
pub const TEMPCREATORTOKENADDR: Item<Addr> = Item::new("temp_token_addr");
pub const TEMPNFTADDR: Item<Addr> = Item::new("temp_nft_addr");
//setting the commit field inside the pool
pub const SETCOMMIT: Map<&str, CommitInfo> = Map::new("commit_info");
//tracking pool id for querys etc
pub const NEXT_POOL_ID: Item<u64> = Item::new("next_pool_id");
//used in querys to grab multiple pools
pub const POOLS_BY_ID: Map<u64, CommitInfo> = Map::new("pools_by_id");
//keep track of pool creation state in case any corruption or bad executes.
pub const CREATION_STATES: Map<u64, CreationState> = Map::new("creation_states");

#[cw_serde]
pub struct FactoryInstantiate {
    //admin of the factory - will be bluechip or some multisig or something along those lines. person who can edit effectively
    pub factory_admin_address: Addr,
    //amount of bluechip spent to cross the commit threshold
    pub commit_amount_for_threshold_bluechip: Uint128,
    //threshold needed to be crossed for pool to become fully active - priced in USD 
    pub commit_threshold_limit_usd: Uint128,
    //oracle contract address to track usd price
    pub oracle_contract_addr: Addr,
    //ticker the oracle is tracking for pricing
    pub oracle_ticker: String,
    //CW20 contract id that is store on the chain for the pool to use when minting new NFTs
    pub cw20_token_contract_id: u64,
    //nft contract id that is store on the chain for the pool to use when minting new NFTs
    pub cw721_nft_contract_id: u64,
    //the pool contract id used by the factory to replicate pools when the pool create function is called.
    pub create_pool_wasm_contract_id: u64,
    //bluechip wallet address whwere bluechip fees accumulate from commits
    pub bluechip_wallet_address: Addr,
    //fee distributed to bluechip every commit  - 1%
    pub commit_fee_bluechip: Decimal,
    //fee distributed to creator ever commit - 5%
    pub commit_fee_creator: Decimal,
}
//info about creator and pool for commit tracking
#[cw_serde]
pub struct CommitInfo {
    //id of pool - will be some positive integer
    pub pool_id: u64,
    pub creator: Addr,
    pub creator_token_addr: Addr,
    pub creator_pool_addr: Addr,
}

//used to track the state of the pool throughout creation. Will trigger different events upon partial or complete creation
#[cw_serde]
pub struct CreationState {
    //tracking pool by id
    pub pool_id: u64,
    //creator of the pool
    pub creator: Addr,
    //token address of the pool
    pub creator_token_address: Option<Addr>,
    //nft address used to mint new liquidity position nfts
    pub mint_new_position_nft_address: Option<Addr>,
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
