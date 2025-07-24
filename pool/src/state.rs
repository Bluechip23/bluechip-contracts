use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, QuerierWrapper, StdResult,
    Uint128, Timestamp, Decimal,
};
use cw_storage_plus::Item;
use crate::{
    asset::{Asset, AssetInfo, PairType},
    msg::FeeInfo,
};
use cw_storage_plus::Map;
/// ## Description
/// This structure stores the main config parameters for a constant product pair contract.
#[cw_serde]
pub struct Config {
    /// General pair information (e.g pair type)
    pub pair_info: PairInfo,
    /// The factory contract address
    pub factory_addr: Addr,
    /// The last timestamp when the pair contract update the asset cumulative prices
    pub block_time_last: u64,
    /// The last cumulative price for asset 0
    pub price0_cumulative_last: Uint128,
    /// The last cumulative price for asset 1
    pub price1_cumulative_last: Uint128,
    pub subscription_period: u64,
    pub lp_fee: Decimal,
    pub commit_limit: Uint128,
    pub commit_amount: Uint128,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
    pub creator_amount: Uint128,
    pub bluechip_amount: Uint128,
    pub pool_amount: Uint128,
    pub available_payment: Vec<Uint128>,
}

#[cw_serde]
pub struct TokenMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// ## Description
/// Stores the config struct at the given key
pub const USD_RAISED: Item<Uint128> = Item::new("usd_raised");
pub const CONFIG: Item<Config> = Item::new("c   onfig");
pub const FEEINFO: Item<FeeInfo> = Item::new("fee_info");
pub const COMMITSTATUS: Item<Uint128> = Item::new("commit_status");
pub const NATIVE_RAISED: Item<Uint128> = Item::new("native_raised");
pub const THRESHOLD_HIT: Item<bool>    = Item::new("threshold_hit");
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> = cw_storage_plus::Map::new("commit_usd");
pub const SUB_INFO: Map<&Addr, Subscription> = Map::new("sub_info");
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
pub const POSITIONS: Map<&str, Position> = Map::new("positions");
pub const POOLS: Map<u64, Pool> = Map::new("pools");


#[cw_serde]
pub struct Subscription {
    pub expires: Timestamp,   
    pub total_paid: Uint128,  
}
#[cw_serde]
/// This structure stores the main parameters for an BETFI pair
pub struct Pool {
    pub pool_id: u64,
    pub reserve0: Uint128,  // native token
    pub reserve1: Uint128,  // cw20 token
    pub total_liquidity: Uint128,
    // Global fee trackers (fees per unit of liquidity)
    pub fee_growth_global_0: Uint128,
    pub fee_growth_global_1: Uint128,
    pub total_fees_collected_0: Uint128,
    pub total_fees_collected_1: Uint128,
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
pub struct Position {
    pub pool_id: u64,
    pub liquidity: Decimal,
    pub owner: Addr,
    // optionally: fee‐growth snapshots, etc.
    pub fee_growth_inside_0_last: Uint128,
    pub fee_growth_inside_1_last: Uint128,
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
