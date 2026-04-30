//! Shared state — every storage Item, struct, and constant that both
//! pool kinds read or write.
//!
//! Commit-phase-only storage (COMMIT_LEDGER, DISTRIBUTION_STATE, etc.)
//! stays in the creator-pool crate's own `state.rs`; this module only
//! contains what the shared hot-path code in `pool_core::liquidity`,
//! `pool_core::swap`, `pool_core::admin`, and `pool_core::query`
//! actually touches.
//!
//! The creator-pool crate glob-re-exports this module from its own
//! `state.rs` so existing `use crate::state::X;` call sites keep
//! resolving unchanged.

use crate::msg::CommitFeeInfo;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, StdResult, Storage, Timestamp, Uint128};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::asset::{PoolPairType, TokenInfo, TokenType};

// -- Structs --------------------------------------------------------------

#[cw_serde]
pub struct TokenMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

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
    // `usd_payment_tolerance_bps` removed: it was admin-tunable but never
    // read by any execution path, leaving operators with the impression
    // they were configuring a USD-payment slippage knob that had zero
    // effect on commit valuation. cw_serde tolerates unknown fields on
    // deserialize, so already-stored PoolSpecs records carrying it
    // round-trip cleanly into the new shape and the field is dropped on
    // the next save. If a USD-vs-oracle slippage protection is ever
    // wanted, wire it up explicitly with a fresh field rather than
    // resurrecting this stale config.
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
    /// Subset of `liquidity` that the owner cannot remove. Set to
    /// `MINIMUM_LIQUIDITY` (1000) on the first depositor's position so the
    /// classic Uniswap-V2 inflation-attack lock is genuinely enforced
    /// here rather than being a cosmetic accounting trick. Fees still
    /// accrue against the FULL `liquidity` (including the locked slice),
    /// so the depositor keeps fee rights on the locked principal — they
    /// just can never withdraw the principal itself.
    /// `#[serde(default)]` keeps existing positions deserializing as zero
    /// (no lock) for backward compatibility with already-deployed pools.
    #[serde(default)]
    pub locked_liquidity: Uint128,
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

/// The four state items read by every swap / commit / liquidity hot path.
/// Bundled so handlers that touch more than one can `load` once and let the
/// borrow checker enforce mutation vs read-only access on each field.
///
/// Only `state` and `fees` are ever mutated on the swap path; `info` and
/// `specs` stay read-only. Callers still save the dirty items themselves —
/// this struct is a loader, not a write-back cache.
pub struct PoolCtx {
    pub info: PoolInfo,
    pub state: PoolState,
    pub fees: PoolFeeState,
    pub specs: PoolSpecs,
}

impl PoolCtx {
    /// Single-shot load of the four core state items in one place. Keeps
    /// the four `.load()` calls in one spot so every new state item added
    /// to the hot path lands here exactly once.
    pub fn load(storage: &dyn Storage) -> StdResult<Self> {
        Ok(Self {
            info: POOL_INFO.load(storage)?,
            state: POOL_STATE.load(storage)?,
            fees: POOL_FEE_STATE.load(storage)?,
            specs: POOL_SPECS.load(storage)?,
        })
    }
}

// -- Storage Items & Maps -------------------------------------------------

pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
pub const POOL_ANALYTICS: Item<PoolAnalytics> = Item::new("pool_analytics");
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
pub const OWNER_POSITIONS: Map<(&Addr, &str), bool> = Map::new("owner_positions");
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
pub const POOL_PAUSED: Item<bool> = Item::new("pool_paused");
pub const EMERGENCY_WITHDRAWAL: Item<EmergencyWithdrawalInfo> = Item::new("emergency_withdrawal");
pub const PENDING_EMERGENCY_WITHDRAW: Item<Timestamp> = Item::new("pending_emergency_withdraw");
pub const EMERGENCY_DRAINED: Item<bool> = Item::new("emergency_drained");
pub const EXPECTED_FACTORY: Item<ExpectedFactory> = Item::new("expected_factory");

// Reentrancy lock acquired by `commit` and `simple_swap` to reject
// re-entry within the same tx (e.g. via a malicious cw20 hook). Storage
// key is `"rate_limit_guard"` for backward compatibility with already-
// deployed pools — the Rust binding was renamed from `REENTRANCY_GUARD`
// because its previous name had nothing to do with rate limiting (which
// is handled separately by USER_LAST_COMMIT) and confused liquidity-op
// authors into adding spurious "reset on error" calls that paired with
// no acquisition.
pub const REENTRANCY_LOCK: Item<bool> = Item::new("rate_limit_guard");

pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");

// Standard pool writes `true` at instantiate (no threshold gate); creator
// pool flips it in the threshold-crossing commit path. Shared handlers
// read via `query_check_commit`.
pub const IS_THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");

// Creator-claimable pot that receives the portion of LP fees "clipped"
// away from small positions by `calculate_fee_size_multiplier`. Standard
// pool's stays empty; emergency_withdraw sweeps it unconditionally.
pub const CREATOR_FEE_POT: Item<CreatorFeePot> = Item::new("creator_fee_pot");

// emergency_withdraw reads `bluechip_wallet_address` for the drain
// recipient; standard pool instantiate saves a zero-valued placeholder.
pub const COMMITFEEINFO: Item<CommitFeeInfo> = Item::new("fee_info");

// Oracle endpoint the pool queries for `ConvertBluechipToUsd`. Initialized
// at instantiate to `msg.used_factory_addr` (the factory contract hosts
// the internal price oracle today, so by default oracle == factory) and
// rotatable via `UpdateConfigFromFactory { oracle_address }`. Read by
// `creator-pool::swap_helper::get_oracle_conversion_with_staleness`,
// which is the only oracle-query call site in the pool.
//
// Forward-compat: pointing this at a different wasm address lets an
// operator decouple "where pricing comes from" from "what's the trusted
// factory" without redeploying every pool — useful for future oracle
// designs (separate oracle wasm, multi-source averaging, randomized
// source selection) and as a recovery lever if the factory's internal
// oracle is ever found to misbehave. The target wasm must respond to
// `FactoryQueryWrapper::InternalBlueChipOracleQuery(ConvertBluechipToUsd)`
// with a `ConversionResponse`.
pub const ORACLE_INFO: Item<OracleInfo> = Item::new("oracle_info");

// Block at which post-threshold trading is allowed to resume after a
// commit pool crosses its threshold. Set inside the threshold-crossing
// commit handler to `env.block.height + POST_THRESHOLD_COOLDOWN_BLOCKS + 1`,
// so the crossing block plus the next `POST_THRESHOLD_COOLDOWN_BLOCKS`
// blocks are gated. Same-block follower trades and the next-N-blocks
// trades are rejected. Eliminates the atomic same-block sandwich on the
// freshly-seeded pool. The threshold-crosser's own bounded excess swap
// (3%-of-reserve cap) still executes in the crossing tx itself, since
// that swap runs before this storage item is read by any other path.
//
// Standard pools never cross a threshold; this item is never set on
// them, and `may_load(...).unwrap_or(0)` makes the gate a no-op.
//
// Read by: simple_swap, execute_swap_cw20, process_post_threshold_commit.
// Written by: process_threshold_crossing_with_excess and the
// "threshold hit exact" branch of execute_commit_logic.
pub const POST_THRESHOLD_COOLDOWN_UNTIL_BLOCK: Item<u64> =
    Item::new("post_threshold_cooldown_until_block");

// -- Constants ------------------------------------------------------------

pub const MINIMUM_LIQUIDITY: Uint128 = Uint128::new(1000);
pub const EMERGENCY_WITHDRAW_DELAY_SECONDS: u64 = 86_400;

/// Blocks of trading freeze applied immediately after a commit pool's
/// threshold crosses. With ~6s block time on typical Cosmos chains, 2
/// blocks ≈ 12s — long enough to break atomic same-block sandwiches
/// targeting the freshly seeded pool, short enough to not meaningfully
/// hurt UX for legitimate first traders.
pub const POST_THRESHOLD_COOLDOWN_BLOCKS: u64 = 2;
