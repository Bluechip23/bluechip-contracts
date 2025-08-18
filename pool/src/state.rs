use crate::{
    asset::{Asset, AssetInfo, PairType},
    msg::{FeeInfo,},
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, QuerierWrapper, StdResult, Timestamp, Uint128};
use cw_storage_plus::Item;
use cw_storage_plus::Map;

#[cw_serde]
pub struct TokenMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// ## Description
/// Stores the config struct at the given key
pub const USD_RAISED: Item<Uint128> = Item::new("usd_raised");

pub const FEEINFO: Item<FeeInfo> = Item::new("fee_info");
pub const COMMITSTATUS: Item<Uint128> = Item::new("commit_status");
pub const NATIVE_RAISED: Item<Uint128> = Item::new("native_raised");
pub const REENTRANCY_GUARD: Item<bool> = Item::new("reentrancy_guard");
pub const THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
pub const SUB_INFO: Map<&Addr, Subscription> = Map::new("sub_info");
pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");
pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
pub const THRESHOLD_PAYOUT: Item<ThresholdPayout> = Item::new("threshold_payout_amounts");
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
pub const COMMIT_CONFIG: Item<CommitInfo> = Item::new("commit_config");
pub const ORACLE_INFO: Item<OracleInfo> = Item::new("oracle_info");
pub const POOL_PARAMS: Item<PoolSpecs> = Item::new("pool_params");
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");
pub const POOL_COMMITS: Map<(u64, &Addr), bool> = Map::new("pool_subs");

#[cw_serde]
pub struct Subscription {
    pub pool_id: u64,
    pub commiter: Addr,
    pub total_paid_usd: Uint128,
    pub total_paid_native: Uint128,
    pub last_commiter: Timestamp,
    pub last_payment_native: Uint128,  
    pub last_payment_usd: Uint128, 
}
#[cw_serde]
pub struct PoolState {
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128, // native token
    pub reserve1: Uint128, // cw20 token
    pub total_liquidity: Uint128,
    pub block_time_last: u64,
    pub price0_cumulative_last: Uint128,
    pub price1_cumulative_last: Uint128,
}

#[cw_serde]
pub struct PoolFeeState {
    pub fee_growth_global_0: Decimal,
    pub fee_growth_global_1: Decimal,
    pub total_fees_collected_0: Uint128,
    pub total_fees_collected_1: Uint128,
}

#[cw_serde]
pub struct PoolSpecs {
    pub subscription_period: u64,
    pub lp_fee: Decimal,
    pub min_commit_interval: u64,
    pub usd_payment_tolerance_bps: u16,
}

#[cw_serde]
pub struct PoolInfo {
    pub pool_id: u64,
    pub pair_info: PairInfo,
    pub factory_addr: Addr,
    pub token_address: Addr,
    pub position_nft_code_id: Addr,
}

#[cw_serde]
pub struct PairInfo {
    /// Asset information for the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    /// Pair contract address
    pub contract_addr: Addr,
    /// Pair LP token address
    pub liquidity_token: Addr,
    /// The pool type (xyk, stableswap etc) available in [`PairType`]
    pub pair_type: PairType,
}
#[cw_serde]
pub struct CachedOracleRate {
    pub rate: Uint128,  // Rate in micro units (1 NATIVE = rate USD)
    pub timestamp: u64, // Block timestamp when cached
}
#[cw_serde]
pub struct OracleInfo {
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
}

#[cw_serde]
pub struct ThresholdPayout {
    pub creator_amount: Uint128,
    pub bluechip_amount: Uint128,
    pub pool_amount: Uint128,
    pub commit_amount: Uint128,
}

#[cw_serde]
pub struct CommitInfo {
    pub commit_limit_usd: Uint128,
}

#[cw_serde]
pub struct Position {
    pub liquidity: Uint128,
    pub owner: Addr,
    // optionally: fee‐growth snapshots, etc.
    pub fee_growth_inside_0_last: Decimal,
    pub fee_growth_inside_1_last: Decimal,
    // Timestamps for better tracking
    pub created_at: u64,
    pub last_fee_collection: u64,
}

impl PairInfo {
    /// Returns the balance for each asset in the pool.
    /// ## Params
    /// * **self** is the type of the caller object
    ///
    /// * **querier** is an object of type [`QuerierWrapper`]
    ///
    /// * **contract_addr** is pair's pool address.
    pub fn query_pools(
        &self,
        querier: &QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[Asset; 2]> {
        Ok([
            Asset {
                amount: self.asset_infos[0].query_pool(querier, contract_addr.clone())?,
                info: self.asset_infos[0].clone(),
            },
            Asset {
                amount: self.asset_infos[1].query_pool(querier, contract_addr)?,
                info: self.asset_infos[1].clone(),
            },
        ])
    }
}
