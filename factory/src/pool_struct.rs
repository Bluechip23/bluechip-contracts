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

/// Canonical per-component values for the threshold-payout splits.
///
/// These MUST mirror `pool_core::state::THRESHOLD_PAYOUT_*_BASE_UNITS`
/// exactly. The pool side's `validate_pool_threshold_payments` rejects
/// any record that doesn't match these constants at pool-instantiate
/// time (`creator-pool::commit::threshold_payout::validate_pool_threshold_payments`),
/// so the factory-side validator must enforce the same canonical-only
/// rule — otherwise a `ProposeConfigUpdate` could install non-canonical
/// splits that the next commit-pool creation would silently fail to
/// instantiate against, burning a full 48h timelock cycle on a misconfig
/// the factory had no way to catch.
///
/// The factory crate intentionally has no compile-time dependency on
/// `pool-core` (they communicate only over wasm message boundaries), so
/// these are duplicated rather than re-exported. Any future change to
/// the pool-side constants MUST be mirrored here AND in a coordinated
/// migration that updates every deployed pool's expected values.
pub const THRESHOLD_PAYOUT_CREATOR_BASE_UNITS: u128 = 325_000_000_000;
pub const THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS: u128 = 25_000_000_000;
pub const THRESHOLD_PAYOUT_POOL_BASE_UNITS: u128 = 350_000_000_000;
pub const THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS: u128 = 500_000_000_000;
pub const THRESHOLD_PAYOUT_TOTAL_BASE_UNITS: u128 = 1_200_000_000_000;

/// Per-pool initial mint splits awarded when a commit pool crosses its
/// threshold. The four components MUST match the canonical
/// `THRESHOLD_PAYOUT_*_BASE_UNITS` constants — `validate()` enforces
/// exact equality and rejects anything else (M-7.2 audit fix). The sum
/// is also the `total_mint()` consumed by `mint_create_pool` as both
/// the CW20 mint cap and the threshold-payout payload.
///
/// Stored on [`FactoryInstantiate`] so the field can be re-validated on
/// the 48h `ProposeConfigUpdate` flow rather than requiring a contract
/// migration. With the strict-canonical validator the only "config
/// update" allowed for this field is "set it to the same canonical
/// values" — admin can still propose a config update touching OTHER
/// fields and leave this one unchanged. Changing the splits requires a
/// coordinated migration that updates both crates' constants and every
/// deployed pool simultaneously.
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
            creator_reward_amount: Uint128::new(THRESHOLD_PAYOUT_CREATOR_BASE_UNITS),
            bluechip_reward_amount: Uint128::new(THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS),
            pool_seed_amount: Uint128::new(THRESHOLD_PAYOUT_POOL_BASE_UNITS),
            commit_return_amount: Uint128::new(THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS),
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

    /// Validates that every component matches its canonical
    /// `THRESHOLD_PAYOUT_*_BASE_UNITS` value exactly AND that the sum
    /// equals `THRESHOLD_PAYOUT_TOTAL_BASE_UNITS` (M-7.2 audit fix).
    ///
    /// Mirrors the pool-side `validate_pool_threshold_payments` in
    /// `creator-pool::commit::threshold_payout`. Previously this
    /// validator only checked "every component non-zero + no overflow"
    /// — which let `ProposeConfigUpdate` install non-canonical splits
    /// that the pool-side validator would then silently reject at the
    /// next commit-pool instantiate, burning a 48h timelock cycle on a
    /// misconfig the factory should have caught at propose time. The
    /// strict-canonical gate keeps the two sides in lockstep.
    ///
    /// Changing the canonical values requires updating BOTH
    /// `factory::pool_struct::THRESHOLD_PAYOUT_*_BASE_UNITS` AND
    /// `pool_core::state::THRESHOLD_PAYOUT_*_BASE_UNITS` in a
    /// coordinated migration that also touches every deployed pool;
    /// there is no admin-tunable path for these splits today.
    pub fn validate(&self) -> StdResult<()> {
        if self.creator_reward_amount != Uint128::new(THRESHOLD_PAYOUT_CREATOR_BASE_UNITS) {
            return Err(StdError::generic_err(format!(
                "ThresholdPayoutAmounts.creator_reward_amount must be the canonical {} \
                 (got {}). Splits are protocol-canonical; changing them requires a \
                 coordinated migration of both factory and pool crates.",
                THRESHOLD_PAYOUT_CREATOR_BASE_UNITS, self.creator_reward_amount
            )));
        }
        if self.bluechip_reward_amount != Uint128::new(THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS) {
            return Err(StdError::generic_err(format!(
                "ThresholdPayoutAmounts.bluechip_reward_amount must be the canonical {} \
                 (got {}).",
                THRESHOLD_PAYOUT_BLUECHIP_BASE_UNITS, self.bluechip_reward_amount
            )));
        }
        if self.pool_seed_amount != Uint128::new(THRESHOLD_PAYOUT_POOL_BASE_UNITS) {
            return Err(StdError::generic_err(format!(
                "ThresholdPayoutAmounts.pool_seed_amount must be the canonical {} \
                 (got {}).",
                THRESHOLD_PAYOUT_POOL_BASE_UNITS, self.pool_seed_amount
            )));
        }
        if self.commit_return_amount != Uint128::new(THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS) {
            return Err(StdError::generic_err(format!(
                "ThresholdPayoutAmounts.commit_return_amount must be the canonical {} \
                 (got {}).",
                THRESHOLD_PAYOUT_COMMIT_RETURN_BASE_UNITS, self.commit_return_amount
            )));
        }
        // Defense-in-depth: re-check the total matches the canonical
        // sum. With the four equality checks above the sum is
        // structurally fixed at THRESHOLD_PAYOUT_TOTAL_BASE_UNITS; this
        // line just locks the constant in case a future edit drifts
        // either the per-component or total values.
        let total = self.total_mint()?;
        if total != Uint128::new(THRESHOLD_PAYOUT_TOTAL_BASE_UNITS) {
            return Err(StdError::generic_err(format!(
                "ThresholdPayoutAmounts total must equal {} (got {}); the per-component \
                 constants drifted from the total.",
                THRESHOLD_PAYOUT_TOTAL_BASE_UNITS, total
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod threshold_payout_validate_tests {
    //! M-7.2 audit-fix coverage: every non-canonical perturbation of the
    //! four threshold-payout components is rejected with an error message
    //! that names the offending field. The canonical Default() must pass.
    //! Closes the C-3 test-coverage gap from the meta-audit.
    use super::*;

    /// The canonical splits round-trip validate cleanly.
    #[test]
    fn canonical_default_passes() {
        let payout = ThresholdPayoutAmounts::default();
        assert!(payout.validate().is_ok(), "canonical default must pass validate");
    }

    /// Off-by-one on the creator share is rejected with the creator field
    /// named. Exercises the first per-component equality branch and pins
    /// the operator-facing error message to the canonical-value mismatch
    /// language so an off-chain dashboard parsing the message can extract
    /// which knob drifted.
    #[test]
    fn rejects_creator_reward_drift() {
        let mut payout = ThresholdPayoutAmounts::default();
        payout.creator_reward_amount = payout
            .creator_reward_amount
            .checked_add(Uint128::new(1))
            .unwrap();
        let err = payout.validate().expect_err("non-canonical creator must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("creator_reward_amount"),
            "error must name the creator field; got: {}",
            msg
        );
        assert!(
            msg.contains(&THRESHOLD_PAYOUT_CREATOR_BASE_UNITS.to_string()),
            "error must include the canonical creator value; got: {}",
            msg
        );
    }

    #[test]
    fn rejects_bluechip_reward_drift() {
        let mut payout = ThresholdPayoutAmounts::default();
        payout.bluechip_reward_amount = Uint128::new(1);
        let err = payout.validate().expect_err("non-canonical bluechip must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("bluechip_reward_amount"),
            "error must name the bluechip field; got: {}",
            msg
        );
    }

    #[test]
    fn rejects_pool_seed_drift() {
        let mut payout = ThresholdPayoutAmounts::default();
        payout.pool_seed_amount = payout
            .pool_seed_amount
            .checked_sub(Uint128::new(1))
            .unwrap();
        let err = payout.validate().expect_err("non-canonical pool_seed must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("pool_seed_amount"),
            "error must name the pool_seed field; got: {}",
            msg
        );
    }

    #[test]
    fn rejects_commit_return_drift() {
        let mut payout = ThresholdPayoutAmounts::default();
        payout.commit_return_amount = Uint128::zero();
        let err = payout
            .validate()
            .expect_err("non-canonical commit_return must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("commit_return_amount"),
            "error must name the commit_return field; got: {}",
            msg
        );
    }

    /// All-zero payload — pre-audit validator accepted this if every
    /// component happened to be zero (the old non-zero check) but now
    /// rejects on the first per-component equality. Pins the regression
    /// vector: a misconfig that sets every component to zero (e.g., a
    /// silent serde default at the wrong layer) is caught at propose
    /// time rather than landing as a no-op mint at threshold-cross.
    #[test]
    fn rejects_all_zero_payload() {
        let payout = ThresholdPayoutAmounts {
            creator_reward_amount: Uint128::zero(),
            bluechip_reward_amount: Uint128::zero(),
            pool_seed_amount: Uint128::zero(),
            commit_return_amount: Uint128::zero(),
        };
        assert!(payout.validate().is_err(), "all-zero payload must reject");
    }

    /// Sanity: `total_mint()` on the canonical default matches the
    /// total constant exactly. Ties the per-component constants to the
    /// total so a future edit that drifts one side without the other
    /// is caught here.
    #[test]
    fn canonical_total_matches_total_constant() {
        let payout = ThresholdPayoutAmounts::default();
        let total = payout.total_mint().expect("total_mint must succeed on canonical");
        assert_eq!(
            total,
            Uint128::new(THRESHOLD_PAYOUT_TOTAL_BASE_UNITS),
            "canonical per-component constants must sum to THRESHOLD_PAYOUT_TOTAL_BASE_UNITS"
        );
    }
}
