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
pub struct PoolConfigUpdate {
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: Option<u64>,
    // `oracle_address` removed (audit fix). Mirrors the same field's
    // removal from `pool_core::msg::PoolConfigUpdate`. Per-pool oracle
    // rotation was an admin-compromise vector — a malicious oracle could
    // return arbitrary USD valuations, letting a tiny commit register
    // as a full threshold cross. Future re-routing, if ever needed,
    // goes through a coordinated `UpgradePools` migration that writes
    // ORACLE_INFO directly.
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
    /// zero. Legacy commit pools written before this field existed
    /// deserialize with `commit_pool_ordinal = 0` via `#[serde(default)]`;
    /// `calculate_and_mint_bluechip` falls back to `pool_id` in that case
    /// to preserve their original mint amount.
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
