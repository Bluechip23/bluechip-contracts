use crate::{
    asset::{PoolPairType, TokenInfo, TokenType},
    msg::CommitFeeInfo,
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, StdResult, Timestamp, Uint128};
use cw_storage_plus::Item;
use cw_storage_plus::Map;

#[cw_serde]
pub struct TokenMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

pub const USD_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("usd_raised");
pub const COMMIT_INFO: Map<&Addr, Committing> = Map::new("sub_info");
pub const COMMITFEEINFO: Item<CommitFeeInfo> = Item::new("fee_info");
pub const NATIVE_RAISED_FROM_COMMIT: Item<Uint128> = Item::new("bluechip_raised");
// Reentrancy lock acquired by `commit` and `simple_swap` to reject
// re-entry within the same tx (e.g. via a malicious cw20 hook). Storage
// key is `"rate_limit_guard"` for backward compatibility with already-
// deployed pools — the Rust binding was renamed from `REENTRANCY_GUARD`
// because its previous name had nothing to do with rate limiting (which
// is handled separately by USER_LAST_COMMIT) and confused liquidity-op
// authors into adding spurious "reset on error" calls that paired with
// no acquisition.
pub const REENTRANCY_LOCK: Item<bool> = Item::new("rate_limit_guard");
pub const IS_THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");
pub const COMMIT_LEDGER: cw_storage_plus::Map<&Addr, Uint128> =
    cw_storage_plus::Map::new("commit_usd");
pub const EXPECTED_FACTORY: Item<ExpectedFactory> = Item::new("expected_factory");
pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");
pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
pub const THRESHOLD_PROCESSING: Item<bool> = Item::new("threshold_processing");
pub const THRESHOLD_PAYOUT_AMOUNTS: Item<ThresholdPayoutAmounts> =
    Item::new("threshold_payout_amounts");
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
pub const DISTRIBUTION_STATE: Item<DistributionState> = Item::new("distribution_state");
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
pub const OWNER_POSITIONS: Map<(&Addr, &str), bool> = Map::new("owner_positions");
pub const COMMIT_LIMIT_INFO: Item<CommitLimitInfo> = Item::new("commit_config");
pub const ORACLE_INFO: Item<OracleInfo> = Item::new("oracle_info");
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");
pub const CREATOR_EXCESS_POSITION: Item<CreatorExcessLiquidity> = Item::new("creator_excess");

// Creator-claimable pot that receives the portion of LP fees "clipped"
// away from small positions by `calculate_fee_size_multiplier`. Without
// this, the clipped fees would sit forever in POOL_FEE_STATE.fee_reserve_*
// unreachable to any position. Routing them into a dedicated pot that
// the pool creator can claim (a) prevents the orphan-fee buildup, and
// (b) gives creators an ongoing incentive tied to small-trade activity.
pub const CREATOR_FEE_POT: Item<CreatorFeePot> = Item::new("creator_fee_pot");

#[cw_serde]
pub struct CreatorFeePot {
    pub amount_0: Uint128,
    pub amount_1: Uint128,
}

impl Default for CreatorFeePot {
    fn default() -> Self {
        Self {
            amount_0: Uint128::zero(),
            amount_1: Uint128::zero(),
        }
    }
}

pub const POOL_PAUSED: Item<bool> = Item::new("pool_paused");
pub const POOL_ANALYTICS: Item<PoolAnalytics> = Item::new("pool_analytics");
pub const EMERGENCY_WITHDRAWAL: Item<EmergencyWithdrawalInfo> = Item::new("emergency_withdrawal");
pub const PENDING_EMERGENCY_WITHDRAW: Item<Timestamp> = Item::new("pending_emergency_withdraw");
pub const EMERGENCY_DRAINED: Item<bool> = Item::new("emergency_drained");
pub const EMERGENCY_WITHDRAW_DELAY_SECONDS: u64 = 86_400;

// Set to `true` when NotifyThresholdCrossed to the factory failed via the
// reply_on_error path during a threshold-crossing commit. All pool-side
// threshold state (IS_THRESHOLD_HIT, reserves, committer distribution)
// still succeeds — only the factory's POOL_THRESHOLD_MINTED flag and the
// per-pool Bluechip mint reward are pending. Any caller can invoke
// ExecuteMsg::RetryFactoryNotify to re-send the notification; on success
// this flag is cleared by the reply handler. Absence or `false` means
// "either never crossed threshold, or factory notification already
// succeeded" — the pool's IS_THRESHOLD_HIT disambiguates.
pub const PENDING_FACTORY_NOTIFY: Item<bool> = Item::new("pending_factory_notify");

// Reply IDs for SubMsg dispatches. Kept sparse so future features can
// slot in without renumbering. The pool has only one caller of reply()
// today (factory-notify), so this is mostly forward-looking.
pub const REPLY_ID_FACTORY_NOTIFY_INITIAL: u64 = 1;
pub const REPLY_ID_FACTORY_NOTIFY_RETRY: u64 = 2;

#[cw_serde]
pub struct PoolAnalytics {
    /// Total number of swaps executed on this pool.
    pub total_swap_count: u64,
    /// Total number of commits (pre- and post-threshold).
    pub total_commit_count: u64,
    /// Cumulative volume of token0 (bluechip) that flowed through swaps.
    pub total_volume_0: Uint128,
    /// Cumulative volume of token1 (creator token) that flowed through swaps.
    pub total_volume_1: Uint128,
    /// Total number of liquidity deposit/add operations.
    pub total_lp_deposit_count: u64,
    /// Total number of liquidity removal operations.
    pub total_lp_withdrawal_count: u64,
    /// Block height of the last trade (swap or post-threshold commit).
    pub last_trade_block: u64,
    /// Block timestamp of the last trade.
    pub last_trade_timestamp: u64,
}

impl Default for PoolAnalytics {
    fn default() -> Self {
        Self {
            total_swap_count: 0,
            total_commit_count: 0,
            total_volume_0: Uint128::zero(),
            total_volume_1: Uint128::zero(),
            total_lp_deposit_count: 0,
            total_lp_withdrawal_count: 0,
            last_trade_block: 0,
            last_trade_timestamp: 0,
        }
    }
}

#[cw_serde]
pub struct EmergencyWithdrawalInfo {
    pub withdrawn_at: u64,
    pub recipient: Addr,
    pub amount0: Uint128,
    pub amount1: Uint128,
    pub total_liquidity_at_withdrawal: Uint128,
}
pub const LAST_THRESHOLD_ATTEMPT: Item<Timestamp> = Item::new("last_threshold_attempt");
pub const DEFAULT_ESTIMATED_GAS_PER_DISTRIBUTION: u64 = 50_000;
pub const DEFAULT_MAX_GAS_PER_TX: u64 = 2_000_000;
pub const MAX_DISTRIBUTIONS_PER_TX: u32 = 40;
pub const MINIMUM_LIQUIDITY: Uint128 = Uint128::new(1000);

// Maximum wall-clock time between successful distribution batches before the
// pool declares the distribution stalled and requires admin recovery via
// `RecoverPoolStuckStates`. Sized for the worst case where the distribution
// keeper is offline: at the default 30-min poll interval the previous 2h
// window left almost no margin and risked bricking a pool on a brief
// keeper outage. 24h gives operators a full day to react.
pub const DISTRIBUTION_STALL_TIMEOUT_SECONDS: u64 = 86_400;

// Distribution keeper bounty is paid by the factory, not the pool.
// See factory::execute_pay_distribution_bounty. The pool just emits a
// WasmMsg to the factory and the factory pays from its own native reserve.
// This keeps LP funds isolated from keeper infrastructure costs.

#[cw_serde]
pub struct DistributionState {
    pub is_distributing: bool,
    pub total_to_distribute: Uint128,
    pub total_committed_usd: Uint128,
    pub last_processed_key: Option<Addr>,
    pub distributions_remaining: u32,
    pub estimated_gas_per_distribution: u64,
    pub max_gas_per_tx: u64,
    pub last_successful_batch_size: Option<u32>,
    pub consecutive_failures: u32,
    pub started_at: Timestamp,
    pub last_updated: Timestamp,
}
#[cw_serde]
pub enum RecoveryType {
    StuckThreshold,
    StuckDistribution,
    StuckReentrancyGuard,
    Both,
}

#[cw_serde]
pub struct Committing {
    pub pool_contract_address: Addr,
    pub committer: Addr,
    pub total_paid_usd: Uint128,
    pub total_paid_bluechip: Uint128,
    pub last_committed: Timestamp,
    pub last_payment_bluechip: Uint128,
    pub last_payment_usd: Uint128,
}
#[cw_serde]
pub struct PoolState {
    pub pool_contract_address: Addr,
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128,
    pub reserve1: Uint128,
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
    pub fee_reserve_0: Uint128,
    pub fee_reserve_1: Uint128,
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
    pub token_address: Addr,
    pub position_nft_address: Addr,
}

#[cw_serde]
pub struct PoolDetails {
    pub asset_infos: [TokenType; 2],
    pub contract_addr: Addr,
    pub pool_type: PoolPairType,
}

#[cw_serde]
pub struct OracleInfo {
    pub oracle_addr: Addr,
}

#[cw_serde]
pub struct ThresholdPayoutAmounts {
    pub creator_reward_amount: Uint128,
    pub bluechip_reward_amount: Uint128,
    pub pool_seed_amount: Uint128,
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
    pub bluechip_amount: Uint128,
    pub token_amount: Uint128,
    pub unlock_time: Timestamp,
    pub excess_nft_id: Option<String>,
}

#[cw_serde]
pub struct Position {
    pub liquidity: Uint128,
    pub owner: Addr,
    pub fee_growth_inside_0_last: Decimal,
    pub fee_growth_inside_1_last: Decimal,
    pub created_at: u64,
    pub last_fee_collection: u64,
    pub fee_size_multiplier: Decimal,
    /// Fees preserved from past partial removals so they can be collected later.
    #[serde(default)]
    pub unclaimed_fees_0: Uint128,
    #[serde(default)]
    pub unclaimed_fees_1: Uint128,
}

impl PoolDetails {
    pub fn query_pools(
        &self,
        querier: &cosmwasm_std::QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[TokenInfo; 2]> {
        pool_factory_interfaces::asset::query_pools(&self.asset_infos, querier, contract_addr)
    }
}
