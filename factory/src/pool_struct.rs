use cosmwasm_schema::cw_serde;

use crate::asset::TokenType;

use cosmwasm_std::{Addr, Decimal, StdError, StdResult, Uint128};
use pool_factory_interfaces::PoolKind;

/// Caller-supplied portion of the commit-pool create message.
///
/// Only `pool_token_info` is honored end-to-end — the factory's stored
/// config is the authoritative source of truth for every other knob
/// (commit threshold, commit fee splits, threshold payout amounts, lock
/// caps, oracle config). The previous version of this struct included
/// caller-supplied versions of those fields, but `mint_create_pool`
/// silently overwrote them with `factory_config.*` values, so a caller
/// thinking they were tuning their pool was just being ignored.
///
/// Reduced to the single load-bearing field so the wire format matches
/// what the contract actually consumes; downstream tooling that used to
/// supply the dropped fields no longer has to construct sentinel zeros.
#[cw_serde]
pub struct CreatePool {
    pub pool_token_info: [TokenType; 2],
}

#[cw_serde]
#[derive(Default)]
pub struct PoolConfigUpdate {
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: Option<u64>,
    /// Per-pool override for the pre-threshold minimum commit value
    /// (USD, 6 decimals). Creator-pool only. Standard-pool proposals
    /// carrying this field are rejected at propose time
    /// (`execute_propose_pool_config_update` looks up `pool_kind` and
    /// rejects when `Standard` AND any commit-floor field is `Some`).
    /// Bounds: `0 < v <= POOL_CONFIG_MAX_MIN_COMMIT_USD`. Mirrors the
    /// pool-side `PoolConfigUpdate.min_commit_usd_pre_threshold`.
    /// `#[serde(default)]` keeps pre-this-field clients wire-compatible.
    #[serde(default)]
    pub min_commit_usd_pre_threshold: Option<Uint128>,
    /// Per-pool override for the post-threshold minimum commit value
    /// (USD, 6 decimals). Creator-pool only. Same shape and bounds as
    /// `min_commit_usd_pre_threshold` above.
    #[serde(default)]
    pub min_commit_usd_post_threshold: Option<Uint128>,
    // `oracle_address` removed (audit fix). Mirrors the same field's
    // removal from `pool_core::msg::PoolConfigUpdate`. Per-pool oracle
    // rotation was an admin-compromise vector — a malicious oracle could
    // return arbitrary USD valuations, letting a tiny commit register
    // as a full threshold cross. Future re-routing, if ever needed,
    // goes through a coordinated `UpgradePools` migration that writes
    // ORACLE_INFO directly.
}

/// Inclusive upper bound on `min_commit_interval` (seconds). Mirrors the pool
/// side's `86400` cap in `pool_core::admin`. Zero is allowed (disables the
/// per-address commit cooldown), matching pool-side acceptance.
pub const POOL_CONFIG_MIN_COMMIT_INTERVAL_MAX_SECONDS: u64 = 86_400;

/// Inclusive upper bound on either commit-floor knob ($1000, 6 decimals).
/// Mirrors the pool side's `MAX_MIN_COMMIT_USD` in
/// `creator-pool::state`. Both ends bounds-check; the propose-time
/// gate exists so an out-of-range value fails fast rather than after
/// 48h timelock.
pub const POOL_CONFIG_MAX_MIN_COMMIT_USD: Uint128 = Uint128::new(1_000_000_000);

impl PoolConfigUpdate {
    /// Validate the update at propose time so a misconfigured value fails
    /// fast rather than after the 48h timelock. The pool side enforces the
    /// same bounds again at apply (defense-in-depth across the trust
    /// boundary), but rejection there would force a Cancel + 48h re-propose
    /// cycle for the admin — surfacing the same error here saves 48h.
    ///
    /// Bounds mirror `pool_core`:
    ///   - `lp_fee`     : `MIN_LP_FEE` (0.1%) ..= `MAX_LP_FEE` (10%)
    ///   - `min_commit_interval` : 0 ..= 86400 seconds
    /// Constants are duplicated rather than imported from `pool-core` to keep
    /// the factory crate free of a `pool-core` dependency (pool-core already
    /// depends on the factory-interfaces crate).
    pub fn validate(&self) -> StdResult<()> {
        let lp_fee_min = Decimal::permille(1);
        let lp_fee_max = Decimal::percent(10);
        if let Some(fee) = self.lp_fee {
            if fee < lp_fee_min || fee > lp_fee_max {
                return Err(StdError::generic_err(format!(
                    "lp_fee {} out of allowed range [{}, {}]; pool will reject at apply time",
                    fee, lp_fee_min, lp_fee_max
                )));
            }
        }
        if let Some(interval) = self.min_commit_interval {
            if interval > POOL_CONFIG_MIN_COMMIT_INTERVAL_MAX_SECONDS {
                return Err(StdError::generic_err(format!(
                    "min_commit_interval {} exceeds maximum {} seconds; pool will reject at apply time",
                    interval, POOL_CONFIG_MIN_COMMIT_INTERVAL_MAX_SECONDS
                )));
            }
        }
        for (name, maybe) in [
            ("min_commit_usd_pre_threshold", self.min_commit_usd_pre_threshold),
            ("min_commit_usd_post_threshold", self.min_commit_usd_post_threshold),
        ] {
            if let Some(v) = maybe {
                if v.is_zero() {
                    return Err(StdError::generic_err(format!(
                        "{} must be non-zero; pool will reject at apply time",
                        name
                    )));
                }
                if v > POOL_CONFIG_MAX_MIN_COMMIT_USD {
                    return Err(StdError::generic_err(format!(
                        "{} {} exceeds maximum {}; pool will reject at apply time",
                        name, v, POOL_CONFIG_MAX_MIN_COMMIT_USD
                    )));
                }
            }
        }
        Ok(())
    }
}

// Mirrors pool::state::RecoveryType. Redefined here to avoid a pool -> factory
// dependency; serde serialization must stay in sync with the pool's variant.
#[cw_serde]
pub enum RecoveryType {
    StuckThreshold,
    StuckDistribution,
    StuckReentrancyGuard,
    Both,
}

#[cw_serde]
pub struct TempPoolCreation {
    pub temp_pool_info: CreatePool,
    pub temp_creator_wallet: Addr,
    pub pool_id: u64,
    pub creator_token_addr: Option<Addr>,
    pub nft_addr: Option<Addr>,
}

/// Per-pool initial mint splits awarded when a commit pool crosses its
/// threshold. The four components MUST be non-zero and their sum is the
/// `total_mint()` consumed by `mint_create_pool` as both the CW20 mint
/// cap and the threshold-payout payload.
///
/// Stored on [`FactoryInstantiate`] so the splits can be tuned through
/// the standard 48h `ProposeConfigUpdate` flow rather than requiring a
/// contract migration. In practice these values are not expected to
/// change post-launch, but routing them through factory config removes
/// the prior two-source-of-truth footgun where the four amounts were
/// hardcoded in `mint_create_pool` and then re-validated against a
/// duplicate `1_200_000_000_000` literal.
#[cw_serde]
pub struct ThresholdPayoutAmounts {
    pub creator_reward_amount: Uint128,
    pub bluechip_reward_amount: Uint128,
    pub pool_seed_amount: Uint128,
    pub commit_return_amount: Uint128,
}

impl Default for ThresholdPayoutAmounts {
    fn default() -> Self {
        Self {
            creator_reward_amount: Uint128::new(325_000_000_000),
            bluechip_reward_amount: Uint128::new(25_000_000_000),
            pool_seed_amount: Uint128::new(350_000_000_000),
            commit_return_amount: Uint128::new(500_000_000_000),
        }
    }
}

#[cw_serde]
pub struct CommitFeeInfo {
    pub bluechip_wallet_address: Addr,
    pub creator_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
}

#[cw_serde]
pub struct PoolDetails {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    pub creator_pool_addr: Addr,
    /// Distinguishes commit (two-phase) pools from standard (xyk) pools.
    /// `#[serde(default)]` makes old serialized records — written before
    /// this field existed — round-trip as `PoolKind::Commit`, which is
    /// the correct legacy classification since every pool created prior
    /// to standard-pool support was a commit pool.
    #[serde(default)]
    pub pool_kind: PoolKind,
    /// 1-indexed ordinal among commit pools at the time this pool was
    /// created. Always zero for standard pools. Consumed by the bluechip
    /// mint-decay formula in `calculate_and_mint_bluechip` so that
    /// permissionless standard-pool creation (which also bumps the
    /// global `POOL_COUNTER`) cannot inflate `x` in the decay polynomial
    /// and shrink legitimate commit pools' threshold-mint reward toward
    /// zero.
    ///
    /// Set non-zero at create time by `execute_create_creator_pool`
    /// (the counter is bumped to `current + 1` before save), so a
    /// commit pool with `commit_pool_ordinal == 0` indicates either
    /// storage corruption or a pre-v1 legacy record. v1 is the launch
    /// version — there is no legacy chain state to back-fill — so
    /// `calculate_and_mint_bluechip` fail-closes (`Err`) on a zero
    /// ordinal rather than falling back to a value that would
    /// inflate the mint relative to the intended schedule.
    ///
    /// `#[serde(default)]` is retained so a future migration that
    /// extends `PoolDetails` with another field continues to
    /// deserialize cleanly; it is NOT a back-compat shim for legacy
    /// records (none exist).
    #[serde(default)]
    pub commit_pool_ordinal: u64,
}

impl ThresholdPayoutAmounts {
    /// Sum of the four payout components. Used as both the CW20 mint cap
    /// (set when the creator token is instantiated) and the total minted
    /// at threshold-cross time. Returns `OverflowError` via `StdError` if
    /// any addition overflows `Uint128`.
    pub fn total_mint(&self) -> StdResult<Uint128> {
        self.creator_reward_amount
            .checked_add(self.bluechip_reward_amount)
            .and_then(|s| s.checked_add(self.pool_seed_amount))
            .and_then(|s| s.checked_add(self.commit_return_amount))
            .map_err(|e| StdError::generic_err(format!("threshold payout total overflow: {}", e)))
    }

    /// Validates that every component is non-zero and the sum does not
    /// overflow. A zero component would silently zero out one side of
    /// the threshold-payout (creator gets nothing, pool unfunded, etc.),
    /// so the propose-time gate rejects it explicitly.
    pub fn validate(&self) -> StdResult<()> {
        if self.creator_reward_amount.is_zero()
            || self.bluechip_reward_amount.is_zero()
            || self.pool_seed_amount.is_zero()
            || self.commit_return_amount.is_zero()
        {
            return Err(StdError::generic_err(
                "ThresholdPayoutAmounts: every component must be non-zero",
            ));
        }
        let _ = self.total_mint()?;
        Ok(())
    }
}
