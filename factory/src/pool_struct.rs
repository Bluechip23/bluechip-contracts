use cosmwasm_schema::cw_serde;

use crate::asset::TokenType;

use cosmwasm_std::{Addr, Binary, Decimal, StdError, StdResult, Uint128};
use pool_factory_interfaces::PoolKind;

#[cw_serde]
pub struct CreatePool {
    pub pool_token_info: [TokenType; 2],
    pub cw20_token_contract_id: u64,
    pub factory_to_create_pool_addr: Addr,
    pub threshold_payout: Option<Binary>,
    pub commit_fee_info: CommitFeeInfo,
    pub creator_token_address: Addr,
    pub commit_amount_for_threshold: Uint128,
    pub commit_limit_usd: Uint128,
    pub pyth_contract_addr_for_conversions: String,
    pub pyth_atom_usd_price_feed_id: String,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
}

#[cw_serde]
pub struct PoolConfigUpdate {
    pub lp_fee: Option<Decimal>,
    pub min_commit_interval: Option<u64>,
    pub usd_payment_tolerance_bps: Option<u16>,
    pub oracle_address: Option<String>,
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
    /// to H14 was a commit pool.
    #[serde(default)]
    pub pool_kind: PoolKind,
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
