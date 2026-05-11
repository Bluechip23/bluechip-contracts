use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Uint128};

pub mod asset;
pub mod cw721_msgs;
pub mod routing;

use crate::asset::TokenType;

#[cw_serde]
pub enum PoolQueryMsg {
    /// Returns this pool's `PoolStateResponseForFactory` (its own state — the
    /// pool is the implicit subject of the query). Previously took a
    /// `pool_contract_address: String` argument that was never read by any
    /// implementor; the dispatch always replied with the queried pool's own
    /// state. Removed to prevent future readers from assuming the parameter
    /// changed which pool's state was returned.
    GetPoolState {},
    GetAllPools {},
    IsPaused {},
}

#[cw_serde]
pub struct IsPausedResponse {
    pub paused: bool,
}

/// Distinguishes the two pool flavors registered in the factory:
///
/// - `Commit`  — the original two-phase pool. Starts in a commit phase, mints
///   a fresh creator CW20 at creation, only opens to swaps/liquidity after
///   USD commits cross the configured threshold. Eligible for oracle
///   sampling once threshold-crossed.
///
/// - `Standard` — a plain xyk pool around two pre-existing assets (any
///   combination of native-denom and CW20). No threshold, no commit phase,
///   no distribution; immediately ready for deposits and swaps at pool
///   creation. Excluded from oracle sampling (its price is not meaningful
///   for bluechip/USD derivation unless the admin explicitly designates
///   it as the anchor pool).
///
/// Default is `Commit` so that old serialized `PoolDetails` records that
/// lack a `pool_kind` field round-trip cleanly as commit pools.
#[cw_serde]
pub enum PoolKind {
    Commit,
    Standard,
}

impl Default for PoolKind {
    fn default() -> Self {
        Self::Commit
    }
}
#[cw_serde]
#[derive(QueryResponses)]
pub enum FactoryQueryMsg {
    #[returns(BluechipPriceResponse)]
    GetBluechipUsdPrice {},

    #[returns(ConversionResponse)]
    ConvertBluechipToUsd { amount: Uint128 },

    #[returns(ConversionResponse)]
    ConvertUsdToBluechip { amount: Uint128 },

    /// Returns the chain-side emergency-withdraw delay (seconds between
    /// `Phase 1: initiate` and `Phase 2: drain` on each pool's
    /// `EmergencyWithdraw` flow). Pools query this at initiate time so
    /// the value tracks `factory_config.emergency_withdraw_delay_seconds`,
    /// which is admin-tunable through the standard 48h
    /// `ProposeConfigUpdate` flow.
    #[returns(EmergencyWithdrawDelayResponse)]
    EmergencyWithdrawDelaySeconds {},
}

#[cw_serde]
pub struct EmergencyWithdrawDelayResponse {
    pub delay_seconds: u64,
}

#[cw_serde]
pub struct BluechipPriceResponse {
    pub price: Uint128,
    pub timestamp: u64,
    pub is_cached: bool,
}

#[cw_serde]
pub struct ConversionResponse {
    pub amount: Uint128,
    pub rate_used: Uint128,
    pub timestamp: u64,
}

#[cw_serde]
pub struct PoolStateResponseForFactory {
    pub pool_contract_address: Addr,
    pub nft_ownership_accepted: bool,
    pub reserve0: Uint128,
    pub reserve1: Uint128,
    pub total_liquidity: Uint128,
    pub block_time_last: u64,
    pub price0_cumulative_last: Uint128,
    pub price1_cumulative_last: Uint128,
    pub assets: Vec<String>,
}

#[cw_serde]
pub struct AllPoolsResponse {
    pub pools: Vec<(String, PoolStateResponseForFactory)>,
}

// Messages that a pool contract can send to the factory contract.
#[cw_serde]
pub enum FactoryExecuteMsg {
    // Called by a pool when its commit threshold has been crossed.
    NotifyThresholdCrossed { pool_id: u64 },
    // Called by a pool's ContinueDistribution handler to ask the factory
    // to pay the keeper bounty out of the factory's native reserve.
    // The factory verifies the caller is a registered pool via
    // POOLS_BY_CONTRACT_ADDRESS, so unregistered contracts cannot drain
    // the reserve by pretending to be a pool.
    PayDistributionBounty { recipient: String },
}

#[cw_serde]
pub enum ExpandEconomyMsg {
    RequestExpansion { recipient: String, amount: Uint128 },
}

#[cw_serde]
pub enum ExpandEconomyExecuteMsg {
    ExpandEconomy(ExpandEconomyMsg),
}

/// Wire-format instantiate message sent by the factory's CreateStandardPool
/// reply chain to a freshly instantiated standard pool wasm.
///
/// Standard pools are plain xyk pools around two pre-existing assets:
/// they do not have a commit phase, do not mint a fresh CW20, and do not
/// participate in oracle sampling. Compared to the commit-pool
/// instantiate shape (`pool::msg::PoolInstantiateMsg`), the only inputs
/// the pool needs are: which two assets it wraps, which CW721 contract
/// to mint position NFTs on, and which factory it belongs to.
///
/// Lives in `pool_factory_interfaces` (not the factory or pool crate)
/// because both sides need to agree on the layout exactly. The pool's
/// instantiate handler dispatches between this struct and the existing
/// commit-pool instantiate shape based on which one deserializes
/// successfully.
#[cw_serde]
pub struct StandardPoolInstantiateMsg {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    pub used_factory_addr: Addr,
    pub position_nft_address: Addr,
    /// Wallet that receives drained funds when an emergency withdraw
    /// completes. Sourced from the factory's `bluechip_wallet_address`
    /// at instantiate time. Must NOT default to the factory address —
    /// the factory has no withdrawal mechanism, so funds drained to it
    /// would be permanently locked.
    pub bluechip_wallet_address: Addr,
}
