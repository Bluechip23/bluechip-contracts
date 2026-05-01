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

/// Pool identity and addresses (factory, token, position NFT).
pub const POOL_INFO: Item<PoolInfo> = Item::new("pool_info");
/// Mutable pool state: reserves, total_liquidity, price accumulators.
pub const POOL_STATE: Item<PoolState> = Item::new("pool_state");
/// Fee accounting: global fee growth, fee reserves, totals collected.
pub const POOL_FEE_STATE: Item<PoolFeeState> = Item::new("pool_fee_state");
/// Tunable pool parameters (lp_fee, min_commit_interval).
pub const POOL_SPECS: Item<PoolSpecs> = Item::new("pool_specs");
/// Cumulative counters for swaps, commits, deposits, withdrawals.
pub const POOL_ANALYTICS: Item<PoolAnalytics> = Item::new("pool_analytics");
/// All LP positions keyed by string position id.
pub const LIQUIDITY_POSITIONS: Map<&str, Position> = Map::new("positions");
/// Reverse index: positions owned by a given address.
pub const OWNER_POSITIONS: Map<(&Addr, &str), bool> = Map::new("owner_positions");
/// Monotonic counter used to mint the next Position NFT id.
pub const NEXT_POSITION_ID: Item<u64> = Item::new("next_position_id");
/// Top-level pause flag — true if the pool is paused for any reason.
pub const POOL_PAUSED: Item<bool> = Item::new("pool_paused");
/// Distinguishes "admin/emergency paused" (false) from "auto-paused
/// because reserves dropped below MINIMUM_LIQUIDITY" (true). Only meaningful
/// when `POOL_PAUSED == true`.
///
/// Wire-up:
///   - Auto-set: after a swap or remove leaves reserves < MIN, the handler
///     sets POOL_PAUSED + POOL_PAUSED_AUTO = true (only if no harder pause
///     is already in place).
///   - Auto-clear: after a deposit pushes reserves back >= MIN AND the
///     pool was auto-paused, the deposit clears both flags.
///   - Hard pauses (admin Pause, emergency_withdraw_initiate) explicitly
///     set POOL_PAUSED_AUTO = false to override any prior auto-state.
///
/// Gating semantics:
///   - Auto-paused (true & true): deposits allowed (recovery path);
///     swaps / removes / collects rejected.
///   - Hard-paused (true & false): everything rejected, including
///     deposits — admin must Unpause or cancel emergency to resume.
///
/// `#[serde(default)]` keeps deployed pools that predate this flag
/// deserializing as false; legacy paused pools therefore behave as
/// hard-paused (the safe default), and admin Pause / Unpause continues
/// to work unchanged.
pub const POOL_PAUSED_AUTO: Item<bool> = Item::new("pool_paused_auto");
/// Audit record written on completed emergency withdraw (Phase 2 drain).
pub const EMERGENCY_WITHDRAWAL: Item<EmergencyWithdrawalInfo> = Item::new("emergency_withdrawal");
/// Effective-after timestamp armed by Phase 1 (initiate); cleared by
/// Phase 2 (drain) or by cancel.
pub const PENDING_EMERGENCY_WITHDRAW: Item<Timestamp> = Item::new("pending_emergency_withdraw");
/// Permanent flag set after a successful emergency drain.
pub const EMERGENCY_DRAINED: Item<bool> = Item::new("emergency_drained");
/// Expected factory address pinned at instantiate for sanity checks.
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

/// Transient context for SubMsg-based CW20 balance verification on
/// deposits. The deposit handler snapshots the pool's pre-balance for
/// every CW20 side, saves this context, and dispatches the last
/// CW20-side `TransferFrom` as a `SubMsg::reply_on_success`. The reply
/// handler queries the post-balance, confirms the delta matches the
/// expected `actual_amount`, and either clears the context (success)
/// or errors (causing the entire transaction to roll back).
///
/// Only set / read when `verify_balances == true` is passed into the
/// shared deposit/add helpers — i.e. by standard-pool, where the CW20
/// can be any third-party contract. Creator-pool's freshly minted
/// `cw20-base` is trusted (no transfer fee, no rebase) and never
/// triggers this path.
///
/// `cw20_side*_addr == None` for non-CW20 sides; balances on those
/// sides are not snapshotted (native bank transfers are exact).
#[cw_serde]
pub struct DepositVerifyContext {
    pub pool_addr: Addr,
    pub cw20_side0_addr: Option<Addr>,
    pub cw20_side1_addr: Option<Addr>,
    pub pre_balance0: Uint128,
    pub pre_balance1: Uint128,
    pub expected_delta0: Uint128,
    pub expected_delta1: Uint128,
}

/// Storage for the transient `DepositVerifyContext` used between deposit
/// dispatch and the balance-verification reply.
pub const DEPOSIT_VERIFY_CTX: Item<DepositVerifyContext> = Item::new("deposit_verify_ctx");

/// Reply ID for `DEPOSIT_VERIFY_CTX` — emitted by the
/// `verify_balances == true` deposit/add path on its final SubMsg, and
/// dispatched in standard-pool's `reply` entry point to
/// `pool_core::balance_verify::handle_deposit_verify_reply`. Numeric
/// value is high enough to not collide with any factory or creator-pool
/// reply ID conventions.
pub const DEPOSIT_VERIFY_REPLY_ID: u64 = 0xD550_0000;

/// Per-user timestamp of last commit, used by rate limiting.
pub const USER_LAST_COMMIT: Map<&Addr, u64> = Map::new("user_last_commit");

/// Standard pool writes `true` at instantiate (no threshold gate); creator
/// pool flips it in the threshold-crossing commit path. Shared handlers
/// read via `query_check_commit`.
pub const IS_THRESHOLD_HIT: Item<bool> = Item::new("threshold_hit");

/// Creator-claimable pot that receives the portion of LP fees "clipped"
/// away from small positions by `calculate_fee_size_multiplier`. Standard
/// pool's stays empty; emergency_withdraw sweeps it unconditionally.
pub const CREATOR_FEE_POT: Item<CreatorFeePot> = Item::new("creator_fee_pot");

/// emergency_withdraw reads `bluechip_wallet_address` for the drain
/// recipient; standard pool instantiate saves a zero-valued placeholder.
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

/// Uniswap-V2-style minimum-liquidity floor permanently locked on the
/// first deposit, and the per-reserve floor used by auto-pause checks.
pub const MINIMUM_LIQUIDITY: Uint128 = Uint128::new(1000);
/// 24 hours, in seconds — timelock between emergency_withdraw_initiate
/// and emergency_withdraw_core_drain.
pub const EMERGENCY_WITHDRAW_DELAY_SECONDS: u64 = 86_400;

/// Blocks of trading freeze applied immediately after a commit pool's
/// threshold crosses. With ~6s block time on typical Cosmos chains, 2
/// blocks ≈ 12s — long enough to break atomic same-block sandwiches
/// targeting the freshly seeded pool, short enough to not meaningfully
/// hurt UX for legitimate first traders.
pub const POST_THRESHOLD_COOLDOWN_BLOCKS: u64 = 2;

/// Classify the pool's current pause state. Used by the dispatch
/// gates to allow deposits during auto-pause (recovery) but reject them
/// during admin / emergency pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseKind {
    /// Pool is open. POOL_PAUSED == false.
    None,
    /// Reserves fell below MINIMUM_LIQUIDITY after a swap or remove.
    /// Deposits are allowed (recovery path); other ops reject.
    AutoLowLiquidity,
    /// Admin or emergency-withdraw pending. Everything rejects.
    Hard,
}

/// Resolve POOL_PAUSED + POOL_PAUSED_AUTO into a `PauseKind`. Reads
/// only — does not mutate. `may_load.unwrap_or(false)` means absent
/// storage decodes as "not set", preserving backward-compat with
/// pools deployed before POOL_PAUSED_AUTO existed.
pub fn pause_kind(storage: &dyn Storage) -> StdResult<PauseKind> {
    if !POOL_PAUSED.may_load(storage)?.unwrap_or(false) {
        return Ok(PauseKind::None);
    }
    if POOL_PAUSED_AUTO.may_load(storage)?.unwrap_or(false) {
        Ok(PauseKind::AutoLowLiquidity)
    } else {
        Ok(PauseKind::Hard)
    }
}

/// Arm the auto-pause flag after a liquidity-out operation if
/// post-state reserves dropped below `MINIMUM_LIQUIDITY`. No-op when
/// reserves are still healthy or when the pool is already hard-paused
/// (admin / emergency-pending) — overriding a hard pause with an auto
/// flag would let the next deposit unintentionally clear the admin's
/// intent. Auto-pause only over a "None" pause state.
///
/// Called from `remove_all_liquidity` and `remove_partial_liquidity`
/// after the post-remove POOL_STATE save. `swap` and `commit` paths
/// don't need this — their own MINIMUM_LIQUIDITY checks reject any
/// trade that would leave reserves below the floor, so post-trade
/// reserves stay ≥ MIN by construction.
pub fn maybe_auto_pause_on_low_liquidity(
    storage: &mut dyn Storage,
    pool_state: &PoolState,
) -> StdResult<bool> {
    let drained =
        pool_state.reserve0 < MINIMUM_LIQUIDITY || pool_state.reserve1 < MINIMUM_LIQUIDITY;
    if !drained {
        return Ok(false);
    }
    // Don't override hard pauses. Only arm auto when the pool is
    // currently considered "open" (not paused for any reason).
    let already_paused = POOL_PAUSED.may_load(storage)?.unwrap_or(false);
    if already_paused {
        return Ok(false);
    }
    POOL_PAUSED.save(storage, &true)?;
    POOL_PAUSED_AUTO.save(storage, &true)?;
    Ok(true)
}
