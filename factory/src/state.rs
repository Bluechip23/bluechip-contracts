use crate::pool_struct::{PoolDetails, TempPoolCreation};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::PoolStateResponseForFactory;

//states used during the pool and factory creation process.
//there is TEMPdata to keep the creation process going, TEMPdata is eventually removed after sucessful pool creation
pub const FACTORYINSTANTIATEINFO: Item<FactoryInstantiate> = Item::new("config");
/// Per-pool temporary creation state, keyed by pool_id to prevent concurrent pool
/// creations from overwriting each other (was previously a singleton Item).
pub const TEMP_POOL_CREATION: Map<u64, TempPoolCreation> = Map::new("temp_pool_creation_v2");
pub const PENDING_CONFIG: Item<PendingConfig> = Item::new("pending_config");
pub const POOL_COUNTER: Item<u64> = Item::new("pool_counter");
//setting the commit field inside the pool
pub const SETCOMMIT: Map<&str, CommitInfo> = Map::new("commit_info");
//tracking pool id for querys etc
//used in querys to grab multiple pools
pub const POOLS_BY_ID: Map<u64, PoolDetails> = Map::new("pools_by_id");
pub const POOLS_BY_CONTRACT_ADDRESS: Map<Addr, PoolStateResponseForFactory> =
    Map::new("pools_by_contract_address");
//keep track of pool creation state in case any corruption or bad executes.
pub const POOL_CONTRACT_ADDRESS: Item<Addr> = Item::new("pool_contract_addr");
pub const POOL_CREATION_STATES: Map<u64, PoolCreationState> = Map::new("creation_states");
//pyth info for conversions
pub const PYTH_CONTRACT_ADDR: &str =
    "neutron1m2emc93m9gpwgsrsf2vylv9xvgqh654630v7dfrhrkmr5slly53spg85wv";
//direct feed used from pyth contract that exposes ATOM/USD price
//pub const ATOM_USD_PRICE_FEED_ID: &str =
//  "0xb00b60f88b03a6a625a8d1c048c3f66653edf217439983d037e7222c4e612819";
pub const ATOM_USD_PRICE_FEED_ID: &str = "ATOM_USD";
pub const MAX_PRICE_AGE_SECONDS_BEFORE_STALE: u64 = 300; // 5 minutes
pub const ATOM_BLUECHIP_ANCHOR_POOL_ADDRESS: Item<Addr> = Item::new("atom_pool_address");
pub const POOL_REGISTRY: Map<u64, Addr> = Map::new("pool_registry");
pub const POOL_CODE_ID: Item<u64> = Item::new("pool_code_id");
pub const PENDING_POOL_UPGRADE: Item<PoolUpgrade> = Item::new("pending_upgrade");
pub const FIRST_POOL_TIMESTAMP: Item<Timestamp> = Item::new("first_pool_timestamp");
/// Tracks which pools have already triggered their bluechip mint on threshold crossing.
/// Prevents double-minting if NotifyThresholdCrossed is called multiple times.
pub const POOL_THRESHOLD_MINTED: Map<u64, bool> = Map::new("pool_threshold_minted");

#[cw_serde]
pub struct FactoryInstantiate {
    //admin of the factory - will be bluechip or some multisig or something along those lines. person who can edit effectively
    pub factory_admin_address: Addr,
    //amount of bluechip spent to cross the commit threshold
    pub commit_amount_for_threshold_bluechip: Uint128,
    //threshold needed to be crossed for pool to become fully active - priced in USD
    pub commit_threshold_limit_usd: Uint128,
    //pyth is used to obtain atom prices in dollar to eventually convert bluechip to dollars
    pub pyth_contract_addr_for_conversions: String,
    pub pyth_atom_usd_price_feed_id: String,
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
    //max bluechip that can be locked per pool - protects against pools locking to much bluechip in extreme market conditions
    pub max_bluechip_lock_per_pool: Uint128,
    //days until creator gains access to above max locked bluechip for their pool.
    pub creator_excess_liquidity_lock_days: u64,
    pub atom_bluechip_anchor_pool_address: Addr,
    pub bluechip_mint_contract_address: Option<Addr>,
}
#[cw_serde]
pub struct PendingConfig {
    pub new_config: FactoryInstantiate,
    pub effective_after: Timestamp,
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
pub struct PoolCreationState {
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

#[cw_serde]
pub struct PoolUpgrade {
    pub new_code_id: u64,
    pub migrate_msg: Binary,
    pub pools_to_upgrade: Vec<u64>,
    pub upgraded_count: u32,
    pub effective_after: Timestamp,
}
