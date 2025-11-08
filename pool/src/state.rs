use crate::{
    asset::{PoolPairType, TokenInfo, TokenType},
    msg::CommitFeeInfo,
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

//amount raised during the pool funding phase. tops out at commit_limit_usd
pub const USD_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("usd_raised");
pub const COMMIT_INFO: Map<&Addr, Commiting> = Map::new("sub_info");
//5 minutes, oracle will expire due to stale prices
pub const MAX_ORACLE_AGE: u64 = 3000000;
//fee infor for commit transactions
pub const COMMITFEEINFO: Item<CommitFeeInfo> = Item::new("fee_info");
//whether or the pool has crossed the threshold or not
pub const COMMITSTATUS: Item<Uint128> = Item::new("commit_status");
//amount of bluechips raised for the pool
pub const NATIVE_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("bluechip_raised");
//set to prohibit spam transactions from a single wallet
pub const RATE_LIMIT_GUARD: Item<bool> = Item::new("rate_limit_guard");
//Has the threshold been hit for this pool
pub const IS_THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");
//store all the commiters and the amount they have commited to the pool prior to the threshold being hit
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
//the contract of the factory being used for pool creation
pub const EXPECTED_FACTORY: Item<ExpectedFactory> = Item::new("expected_factory");
pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");
//information for pool including factory, pool, and token addresses
pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
//liquidity, reserve, and prices
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
pub const MAX_DISTRIBUTIONS_PER_TX: u32 = 40;
//lp fee for liquidity pools
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
//used to handle races cases when the threshold is being crossed
pub const THRESHOLD_PROCESSING: Item<bool> = Item::new("threshold_processing");
//amounts that get sent to designated areas when threhsold is crossed
pub const THRESHOLD_PAYOUT_AMOUNTS: Item<ThresholdPayoutAmounts> =
    Item::new("threshold_payout_amounts");
//pool identifier incriments by 1 every pool
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
pub const DISTRIBUTION_STATE: Item<DistributionState> = Item::new("distribution_state");
//information liquiidty positions in pools
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
//commit limit and amount of bluechips that will be stored in pool
pub const COMMIT_LIMIT_INFO: Item<CommitLimitInfo> = Item::new("commit_config");
//symbol orcale is trakcing along with its price
pub const ORACLE_INFO: Item<OracleInfo> = Item::new("oracle_info");
//tracking the global fee growth and total fees collected for the poolS
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");
pub const POOLS: Map<&str, PoolState> = Map::new("pools");
pub const CREATOR_EXCESS_POSITION: Item<CreatorExcessLiquidity> = Item::new("creator_excess");

#[cw_serde]
pub struct DistributionState {
    pub is_distributing: bool,
    pub total_to_distribute: Uint128,
    pub total_committed_usd: Uint128,
    pub last_processed_key: Option<Addr>,
    pub distributions_remaining: u32,
}

#[cw_serde]
pub struct Commiting {
    pub pool_contract_address: Addr,
    //commit transaction executer
    pub commiter: Addr,
    //amount paid converted to USD
    pub total_paid_usd: Uint128,
    //amount of bluechips used to pay for the transaction
    pub total_paid_bluechip: Uint128,
    //last time someone commited to pool
    pub last_commited: Timestamp,
    //last amount of bluechips commited
    pub last_payment_bluechip: Uint128,
    //last amount converted to USD
    pub last_payment_usd: Uint128,
}
#[cw_serde]
pub struct PoolState {
    pub pool_contract_address: Addr,
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128, // bluechip token
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
    pub pool_info: PoolDetails,
    pub factory_addr: Addr,
    //cw20 token contract address used for creation
    pub token_address: Addr,
    //cw721 contract address for pool used for creation
    pub position_nft_address: Addr,
}

#[cw_serde]
pub struct PoolDetails {
    // information for the two tokens in the pool
    pub asset_infos: [TokenType; 2],
    // pool contract address
    pub contract_addr: Addr,
    pub pool_type: PoolPairType,
}

#[cw_serde]
pub struct OracleInfo {
    //oracle contract addresss
    pub oracle_addr: Addr,
}

#[cw_serde]
pub struct ThresholdPayoutAmounts {
    // once the threshold is crossed, the amount distributed directly to the creator
    pub creator_reward_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the BlueChip
    pub bluechip_reward_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the newly formed creator pool
    pub pool_seed_amount: Uint128,
    //the total sum of tokens sent back to commiters who commited prior to the threshold being crossed
    pub commit_return_amount: Uint128,
}

#[cw_serde]
pub struct CommitLimitInfo {
    pub commit_amount_for_threshold: Uint128,
    pub commit_amount_for_threshold_usd: Uint128,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
}
#[cw_serde]
pub struct CreatorExcessLiquidity {
    pub creator: Addr,
    pub bluechip_amount: Uint128, // Excess bluechip amount
    pub token_amount: Uint128,    // Proportional creator tokens
    pub unlock_time: Timestamp,
    pub excess_nft_id: Option<String>, 
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
    pub fee_size_multiplier: Decimal,
}

impl PoolDetails {
    pub fn query_pools(
        &self,
        querier: &QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[TokenInfo; 2]> {
        Ok([
            TokenInfo {
                amount: self.asset_infos[0].query_pool(querier, contract_addr.clone())?,
                info: self.asset_infos[0].clone(),
            },
            TokenInfo {
                amount: self.asset_infos[1].query_pool(querier, contract_addr)?,
                info: self.asset_infos[1].clone(),
            },
        ])
    }
}
