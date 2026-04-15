use crate::pool_struct::{PoolDetails, TempPoolCreation};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::PoolStateResponseForFactory;

pub const FACTORYINSTANTIATEINFO: Item<FactoryInstantiate> = Item::new("config");
pub const TEMP_POOL_CREATION: Map<u64, TempPoolCreation> = Map::new("temp_pool_creation_v2");
pub const PENDING_CONFIG: Item<PendingConfig> = Item::new("pending_config");
pub const POOL_COUNTER: Item<u64> = Item::new("pool_counter");
// Keyed by pool_id (not creator address) to avoid collisions when the
// same creator makes multiple pools.
pub const SETCOMMIT: Map<u64, CommitInfo> = Map::new("commit_info");
pub const POOLS_BY_ID: Map<u64, PoolDetails> = Map::new("pools_by_id");
pub const POOLS_BY_CONTRACT_ADDRESS: Map<Addr, PoolStateResponseForFactory> =
    Map::new("pools_by_contract_address");
pub const POOL_CREATION_STATES: Map<u64, PoolCreationState> = Map::new("creation_states");
pub const MAX_PRICE_AGE_SECONDS_BEFORE_STALE: u64 = 300;
pub const POOL_REGISTRY: Map<u64, Addr> = Map::new("pool_registry");
pub const PENDING_POOL_UPGRADE: Item<PoolUpgrade> = Item::new("pending_upgrade");
pub const FIRST_POOL_TIMESTAMP: Item<Timestamp> = Item::new("first_pool_timestamp");
pub const POOL_THRESHOLD_MINTED: Map<u64, bool> = Map::new("pool_threshold_minted");
pub const PENDING_POOL_CONFIG: Map<u64, PendingPoolConfig> = Map::new("pending_pool_config");

// Keeper bounty paid out of the factory's native balance to whoever
// successfully calls UpdateOraclePrice. The existing UPDATE_INTERVAL
// cooldown in update_internal_oracle_price gates the frequency, so the
// payout can happen at most once per window and cannot be spammed.
// The admin can adjust this value up to MAX_ORACLE_UPDATE_BOUNTY via
// SetOracleUpdateBounty; setting it to zero disables the bounty.
pub const ORACLE_UPDATE_BOUNTY: Item<Uint128> = Item::new("oracle_update_bounty");

// Hard cap to protect the factory's reserve if the admin key is
// compromised. 1000 bluechip per successful update (6 decimals).
pub const MAX_ORACLE_UPDATE_BOUNTY: Uint128 = Uint128::new(1_000_000_000);

// Native denom the bounty is paid in. The factory must be pre-funded
// with this denom by the bluechip main wallet.
pub const ORACLE_BOUNTY_DENOM: &str = "ubluechip";

// Keeper bounty paid to whoever calls a pool's ContinueDistribution and
// successfully processes a batch. Paid out of the factory's native
// reserve (same pocket as the oracle bounty) so pool LP reserves are
// never tapped for keeper infrastructure. Admin configurable via
// SetDistributionBounty up to MAX_DISTRIBUTION_BOUNTY. Zero disables.
pub const DISTRIBUTION_BOUNTY_AMOUNT: Item<Uint128> = Item::new("distribution_bounty_amount");

// Hard cap for the distribution bounty. 10 bluechip per batch (6 decimals).
pub const MAX_DISTRIBUTION_BOUNTY: Uint128 = Uint128::new(10_000_000);

// ForceRotateOraclePools is a 2-step action: admin proposes a rotation,
// the timelock elapses, then admin invokes ForceRotateOraclePools to
// execute. Prevents a compromised admin from instantly rotating the
// oracle's pool sample set without a 48h observability window for the
// community to notice and respond.
pub const PENDING_ORACLE_ROTATION: Item<Timestamp> = Item::new("pending_oracle_rotation");

#[cw_serde]
pub struct PendingPoolConfig {
    pub pool_id: u64,
    pub update: crate::pool_struct::PoolConfigUpdate,
    pub effective_after: Timestamp,
}

#[cw_serde]
pub struct FactoryInstantiate {
    pub factory_admin_address: Addr,
    pub commit_amount_for_threshold_bluechip: Uint128,
    pub commit_threshold_limit_usd: Uint128,
    pub pyth_contract_addr_for_conversions: String,
    pub pyth_atom_usd_price_feed_id: String,
    pub cw20_token_contract_id: u64,
    pub cw721_nft_contract_id: u64,
    pub create_pool_wasm_contract_id: u64,
    pub bluechip_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
    pub atom_bluechip_anchor_pool_address: Addr,
    pub bluechip_mint_contract_address: Option<Addr>,
}

#[cw_serde]
pub struct PendingConfig {
    pub new_config: FactoryInstantiate,
    pub effective_after: Timestamp,
}

#[cw_serde]
pub struct CommitInfo {
    pub pool_id: u64,
    pub creator: Addr,
    pub creator_token_addr: Addr,
    pub creator_pool_addr: Addr,
}

#[cw_serde]
pub struct PoolCreationState {
    pub pool_id: u64,
    pub creator: Addr,
    pub creator_token_address: Option<Addr>,
    pub mint_new_position_nft_address: Option<Addr>,
    pub pool_address: Option<Addr>,
    pub creation_time: Timestamp,
    pub status: CreationStatus,
    pub retry_count: u8,
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

#[cw_serde]
pub struct PoolUpgrade {
    pub new_code_id: u64,
    pub migrate_msg: Binary,
    pub pools_to_upgrade: Vec<u64>,
    pub upgraded_count: u32,
    pub effective_after: Timestamp,
}
