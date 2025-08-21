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

// Stores the config struct at the given key
//
//amount raised during the pool funding phase. tops out at commit_limit_usd
pub const USD_RAISED: Item<Uint128> = Item::new("usd_raised");
pub const COMMIT_INFO: Map<&Addr, Commiting> = Map::new("sub_info");
//5 minutes, oracle will expire due to stale prices
pub const MAX_ORACLE_AGE: u64 = 3000000;
//fee infor for commit transactions
pub const FEEINFO: Item<FeeInfo> = Item::new("fee_info");
//whether or the pool has crossed the threshold or not
pub const COMMITSTATUS: Item<Uint128> = Item::new("commit_status");
//amount of bluechips raised for the pool
pub const NATIVE_RAISED: Item<Uint128> = Item::new("native_raised");
//set to prohibit spam transactions from a single wallet
pub const RATE_LIMIT_GUARD: Item<bool> = Item::new("rate_limit_guard");
//Has the threshold been hit for this pool
pub const THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");
//store all the commiters and the amount they have commited to the pool prior to the threshold being hit
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
//the contract of the factory being used for pool creation
pub const EXPECTED_FACTORY: Item<ExpectedFactory> = Item::new("expected_factory");
pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");
//information for pool including factory, pool, and token addresses
pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
//liquidity and reserve amounts for the pool
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
//lp fee for liquidity pools
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
//used to handle races cases when the threshold is being crossed
pub const THRESHOLD_PROCESSING: Item<bool> = Item::new("threshold_processing");
//amounts that get sent to designated areas when threhsold is crossed
pub const THRESHOLD_PAYOUT: Item<ThresholdPayout> = Item::new("threshold_payout_amounts");
//pool identifier incriments by 1 every pool
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
//information liquiidty positions in pools
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
//commit limit and amount of bluechips that will be stored in pool
pub const COMMIT_CONFIG: Item<CommitInfo> = Item::new("commit_config");
//symbol orcale is trakcing along with its price
pub const ORACLE_INFO: Item<OracleInfo> = Item::new("oracle_info");
//tracking the global fee growth and total fees collected for the poolS
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");

#[cw_serde]
pub struct Commiting {
    pub pool_id: u64,
    //commit transaction executer
    pub commiter: Addr,
    //amount paid converted to USD
    pub total_paid_usd: Uint128,
    //amount of bluechips used to pay for the transaction
    pub total_paid_native: Uint128,
    //last time someone commited to pool
    pub last_commited: Timestamp,
    //last amount of bluechips commited
    pub last_payment_native: Uint128,  
    //last amount converted to USD
    pub last_payment_usd: Uint128, 
}
#[cw_serde]
pub struct PoolState {
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128, // native token
    pub reserve1: Uint128, // cw20 token
    //how many liquidity units are deposited in the pool
    pub total_liquidity: Uint128, 
    pub block_time_last: u64,
    //
    pub price0_cumulative_last: Uint128,
    pub price1_cumulative_last: Uint128,
}

#[cw_serde]
pub struct PoolFeeState {
    //the amount of fees per unit of liquidty - is used as a baseline to track fees for liquidity positions - asset 0
    pub fee_growth_global_0: Decimal,
    //the amount of fees per unit of liquidty - is used as a baseline to track fees for liquidity positions - asset 1
    pub fee_growth_global_1: Decimal,
    //the total amount of fees collected from pool for asset 0
    pub total_fees_collected_0: Uint128,
    //total amount fo fees collected from pool for asset 1
    pub total_fees_collected_1: Uint128,
}

#[cw_serde]
pub struct ExpectedFactory {
    pub expected_factory_address: Addr,
}

#[cw_serde]
pub struct PoolSpecs {
    pub lp_fee: Decimal,
    pub min_commit_interval: u64,
    pub usd_payment_tolerance_bps: u16,
}

#[cw_serde]
pub struct PoolInfo {
    pub pool_id: u64,
    pub pair_info: PairInfo,
    pub factory_addr: Addr,
    //token contract address
    pub token_address: Addr,
    //cw721 contract address for pool
    pub position_nft_address: Addr,
}

#[cw_serde]
pub struct PairInfo {
    // Asset information for the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    // Pair contract address
    pub contract_addr: Addr,
    // The pool type (xyk, stableswap etc) available in [`PairType`]
    pub pair_type: PairType,
}

#[cw_serde]
pub struct OracleInfo {
    //oracle contract addresss
    pub oracle_addr: Addr,
    //asset symbol being viewed
    pub oracle_symbol: String,
}


#[cw_serde]
//amount that gets paid out in creator token when threshold is crossed
pub struct ThresholdPayout {
    //amount goin to creators
    pub creator_amount: Uint128,
    //amount that goes to BlueChip
    pub bluechip_amount: Uint128,
    //amount that goes to pool for seeding
    pub pool_amount: Uint128,
    //amount that gets sent back to prethreshold commiters. 
    pub commit_amount: Uint128,
}

#[cw_serde]
pub struct RateLimitGuardParams {
    pub swap: Uint128,
    pub deposit_liquidity: Uint128,
    pub remove_partial_liquidity: Uint128,
}

#[cw_serde]
pub struct CommitInfo {
    pub commit_amount_for_threshold: Uint128,
    pub commit_limit_usd: Uint128,
}

#[cw_serde]
pub struct Position {
    //amount of liqudity units in the liquidity position
    pub liquidity: Uint128,
    //wallet address of lqiuidity owner
    pub owner: Addr,
    //last time the user collected fees in terms of fee accumulation fee_growth_global_0 - fee_growth_inside_0_last = fees owed for asset 0
    pub fee_growth_inside_0_last: Decimal,
    pub fee_growth_inside_1_last: Decimal,
    // when was position opened
    pub created_at: u64,
    pub last_fee_collection: u64,
    //when positions are too small, they take a liquidity accumulation penalty. 
    pub fee_multiplier: Decimal,
}

impl PairInfo {
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
