use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, QuerierWrapper, StdResult,
    Uint128
};
use cw_storage_plus::Item;
use crate::{
    asset::{Asset, AssetInfo, PairType},
    msg::FeeInfo,
};
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
/// ## Description
/// Stores the config struct at the given key
pub const USD_RAISED: Item<Uint128> = Item::new("usd_raised");
pub const CONFIG: Item<Config> = Item::new("config");
pub const FEEINFO: Item<FeeInfo> = Item::new("fee_info");
pub const COMMITSTATUS: Item<Uint128> = Item::new("commit_status");
pub const NATIVE_RAISED: Item<Uint128> = Item::new("native_raised");
pub const THRESHOLD_HIT: Item<bool>    = Item::new("threshold_hit");
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> = cw_storage_plus::Map::new("commit_usd");


/// This structure stores the main parameters for an BETFI pair
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
