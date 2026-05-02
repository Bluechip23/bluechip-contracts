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

#[cw_serde]
pub struct ThresholdPayoutAmounts {
    pub creator_reward_amount: Uint128,
    pub bluechip_reward_amount: Uint128,
    pub pool_seed_amount: Uint128,
    pub commit_return_amount: Uint128,
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
    pub fn validate(&self, total_mint: Uint128) -> StdResult<()> {
        let sum = self.creator_reward_amount
            + self.bluechip_reward_amount
            + self.pool_seed_amount
            + self.commit_return_amount;

        if sum != total_mint {
            return Err(StdError::generic_err(
                "Payout amounts don't sum to total mint",
            ));
        }
        Ok(())
    }
}
