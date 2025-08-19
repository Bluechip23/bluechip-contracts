use crate::pair::CreatePool;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Uint128};
use cw_storage_plus::{Item};

pub const CONFIG: Item<FactoryInstantiate> = Item::new("config");
pub const TEMPPOOLINFO: Item<CreatePool> = Item::new("temp_pair");
pub const TEMPCREATOR: Item<Addr> = Item::new("temp_admin");
pub const TEMPPOOLID: Item<u64> = Item::new("temp_pool_id");
pub const TEMPTOKENADDR: Item<Addr> = Item::new("temp_token_addr");
pub const TEMPNFTADDR: Item<Addr> = Item::new("temp_nft_addr");
pub const NEXT_POOL_ID: Item<u64> = Item::new("next_pool_id");

#[cw_serde]
pub struct FactoryInstantiate {
    //admin of the factory - will be bluechip or some multisig or something along those lines. person who can edit effectively
    pub admin: Addr,
    //amount of bluechips that get stored in the pool with newly minted creator token
    pub commit_amount_for_threshold: Uint128,
    //the threshold in dollars that needs to be crossed for the pool to become activated.
    pub commit_limit_usd: Uint128,
    //address of the oracle being used
    pub oracle_addr: Addr,
    //symbol of the token
    pub oracle_symbol: String,
    //CW20 contract id that is store on the chain for the pool to use when minting new NFTs
    pub token_id: u64,
    //nft contract id that is store on the chain for the pool to use when minting new NFTs
    pub position_nft_id: u64,
    //the pool contract id used by the factory to replicate pools when the pool create function is called.
    pub pair_id: u64,
    //BlueChips wallet address
    pub bluechip_address: Addr,
    //fee that goes to bluechip per commit 1%
    pub bluechipe_fee: Decimal,
    //fee that goes to the creator per commit 5%
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct CommitInfo {
    //id of the creator pool (will be some positive integer incrimented every pool creation)
    pub pool_id: u64,
    pub creator: Addr,
    //address of the creator token itself
    pub token_addr: Addr,
    //address of the creator pool.
    pub pool_addr: Addr,
}
