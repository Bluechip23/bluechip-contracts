use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Uint128};

use cw20::{Cw20Coin, MinterResponse};

use crate::asset::TokenType;
use crate::pool_struct::{CommitFeeInfo, CreatePool, PoolConfigUpdate, RecoveryType};
use crate::state::FactoryInstantiate;

//triggers inside factory reply, used to complete the pool creation process.
#[cw_serde]
pub struct CreatePoolReplyMsg {
    pub pool_id: u64,
    pub pool_token_info: [TokenType; 2],
    // The token contract code ID used for the tokens in the pool
    pub cw20_token_contract_id: u64,
    pub used_factory_addr: Addr,
    //gets populated inside reply
    pub threshold_payout: Option<Binary>,
    //fees to bluechip and creator
    pub commit_fee_info: CommitFeeInfo,
    pub commit_threshold_limit_usd: Uint128,
    pub token_address: Addr,
    //address called by the pool to mint new liquidity position NFTs.
    pub position_nft_address: Addr,
    pub max_bluechip_lock_per_pool: Uint128,
    pub creator_excess_liquidity_lock_days: u64,
}

#[cw_serde]
pub enum ExecuteMsg {
    ProposeConfigUpdate {
        config: FactoryInstantiate,
    },
    UpdateConfig {},
    Create {
        pool_msg: CreatePool,
        token_info: CreatorTokenInfo,
    },
    UpdateOraclePrice {},
    // 2-step rotation: admin proposes, waits 48h, then calls
    // ForceRotateOraclePools to execute. Cancel with
    // CancelForceRotateOraclePools before the timelock elapses.
    ProposeForceRotateOraclePools {},
    CancelForceRotateOraclePools {},
    ForceRotateOraclePools {},
    UpgradePools {
        new_code_id: u64,
        pool_ids: Option<Vec<u64>>,
        migrate_msg: Binary,
    },
    CancelConfigUpdate {},
    ExecutePoolUpgrade {},
    CancelPoolUpgrade {},
    ContinuePoolUpgrade {},
    // 48-hour timelocked pool config changes.
    ProposePoolConfigUpdate {
        pool_id: u64,
        pool_config: PoolConfigUpdate,
    },
    ExecutePoolConfigUpdate {
        pool_id: u64,
    },
    CancelPoolConfigUpdate {
        pool_id: u64,
    },
    // Called by a pool contract when its commit threshold has been crossed.
    // Triggers the bluechip mint for this pool (only fires once per pool).
    NotifyThresholdCrossed {
        pool_id: u64,
    },

    // Admin-only pool admin forwards. The pool checks that info.sender ==
    // pool_info.factory_addr, so these must be routed through the factory
    // contract rather than called directly.
    PausePool {
        pool_id: u64,
    },
    UnpausePool {
        pool_id: u64,
    },
    // First call (no pending withdraw): initiates the 24h timelock and
    // pauses the pool. Second call (after the timelock): actually drains
    // pool reserves. The pool itself decides which phase based on state.
    EmergencyWithdrawPool {
        pool_id: u64,
    },
    CancelEmergencyWithdrawPool {
        pool_id: u64,
    },
    RecoverPoolStuckStates {
        pool_id: u64,
        recovery_type: RecoveryType,
    },
    // Admin sets the per-call bounty paid to anyone who successfully
    // invokes UpdateOraclePrice. Capped by MAX_ORACLE_UPDATE_BOUNTY.
    // Set to zero to disable the bounty entirely.
    SetOracleUpdateBounty {
        new_bounty: Uint128,
    },
    // Admin sets the per-batch bounty paid to keepers calling
    // pool.ContinueDistribution. Capped by MAX_DISTRIBUTION_BOUNTY.
    // Set to zero to disable the bounty entirely.
    SetDistributionBounty {
        new_bounty: Uint128,
    },
    // Admin tightens or relaxes the Pyth ATOM/USD confidence-interval
    // gate. `bps` is bounded to
    // `[PYTH_CONF_THRESHOLD_BPS_MIN, PYTH_CONF_THRESHOLD_BPS_MAX]`
    // (50–500 bps inclusive). The same value is applied immediately
    // to (a) the live Pyth read and (b) the cache-fallback re-read,
    // so tightening the gate forces stale-cached prices whose
    // sampling-time conf no longer satisfies the new gate to be
    // refused on the very next conversion. Effect is immediate
    // rather than timelocked: tightening is always conservative
    // (it can only make the protocol more cautious about Pyth
    // confidence) so the standard 48h window would only delay the
    // safer state. Loosening is bounded by the hardcoded ceiling so
    // even a compromised admin cannot disable the gate.
    SetPythConfThresholdBps {
        bps: u16,
    },
    // Pool-only. Forwarded by a pool's ContinueDistribution handler to
    // pay the keeper bounty out of the factory's reserve. The factory
    // verifies info.sender is a registered pool.
    PayDistributionBounty {
        recipient: String,
    },

    // ---- Standard pools ----
    //
    // Permissionless creator-of-its-own-pool entry point for plain xyk
    // pools around two pre-existing assets. Caller pays the configured
    // `standard_pool_creation_fee_usd` in ubluechip — the handler
    // converts USD → bluechip via the oracle (with hardcoded fallback
    // for the bootstrap case where the oracle has no data yet) and
    // forwards the fee to `bluechip_wallet_address`.
    //
    // Pair shape constraints (enforced in the handler): no self-pair;
    // any `Bluechip { denom }` entry must match the canonical
    // bluechip_denom; any `CreatorToken { contract_addr }` entry must
    // resolve as a real CW20 (validated via `TokenInfo {}` query at
    // creation time).
    //
    // `label` is the on-chain label string passed to the pool's
    // wasm instantiate — used by block explorers and operator tooling.
    CreateStandardPool {
        pool_token_info: [TokenType; 2],
        label: String,
    },
    // One-shot bootstrap: admin sets the ATOM/bluechip anchor pool
    // address to a previously-created standard pool. Only callable
    // ONCE per factory deployment (gated on the `INITIAL_ANCHOR_SET`
    // flag). All subsequent anchor changes require the standard 48h
    // `ProposeConfigUpdate` flow. Exists purely to break the launch-day
    // chicken-and-egg of "factory needs an anchor pool address at
    // instantiate but the anchor pool itself is created via the factory".
    SetAnchorPool {
        pool_id: u64,
    },
    // HIGH-4 audit fix: bootstrap-price candidate confirmation. Branch (d)
    // of `update_internal_oracle_price` writes the very-first published
    // TWAP to a pending candidate slot rather than directly into
    // `last_price`. The admin observes the candidate stabilize (≥ 1h
    // BOOTSTRAP_OBSERVATION_SECONDS) and then calls `ConfirmBootstrapPrice`
    // to publish it. Mitigates the single-block anchor-manipulation
    // window that would otherwise let an attacker anchor the breaker
    // for branch (a) to a chosen value.
    ConfirmBootstrapPrice {},
    CancelBootstrapPrice {},
    // MEDIUM-2 audit fix: permissionless storage hygiene. Iterates the
    // per-address rate-limit maps (commit-pool create, standard-pool
    // create) and removes entries older than 10× the cooldown window.
    // `batch_size` caps work per call so large maps don't exceed gas
    // limits; defaults to 100, hard-capped at 500. Anyone may call;
    // there is no bounty (the work is cheap and ops/keepers run it as
    // part of normal housekeeping).
    PruneRateLimits {
        batch_size: Option<u32>,
    },
}

#[cw_serde]
pub struct FactoryInstantiateResponse {
    pub factory: FactoryInstantiate,
}

#[cw_serde]
pub struct TokenInstantiateMsg {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub initial_balances: Vec<Cw20Coin>,
    pub mint: Option<MinterResponse>,
}

#[cw_serde]
pub struct CreatorTokenInfo {
    pub name: String,
    pub symbol: String,
    pub decimal: u8,
}
