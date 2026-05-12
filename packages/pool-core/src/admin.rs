//! Shared admin handlers: pause/unpause, cancel-emergency-withdraw,
//! factory config updates, and the two-phase emergency withdraw split.
//!
//! `execute_emergency_withdraw` is factored into
//! `execute_emergency_withdraw_initiate` (Phase 1: pause + arm the 24h
//! timelock) and `execute_emergency_withdraw_core_drain` (Phase 2: drain
//! reserves+fee_reserves+CREATOR_FEE_POT, write the audit record, flip
//! EMERGENCY_DRAINED). The creator-pool crate wraps these with its
//! commit-only bookkeeping (pre-threshold rejection, CREATOR_EXCESS_POSITION
//! sweep, DISTRIBUTION_STATE halt); standard-pool calls them directly
//! with no extras.

use crate::asset::{TokenInfo, TokenInfoPoolExt};
use crate::error::ContractError;
use crate::msg::PoolConfigUpdate;
use crate::liquidity_helpers::{sync_position_on_transfer, verify_position_ownership};
use crate::state::{
    EmergencyDrainSnapshot, EmergencyWithdrawalInfo, COMMITFEEINFO, CREATOR_FEE_POT,
    EMERGENCY_CLAIM_DORMANCY_SECONDS, EMERGENCY_DRAINED, EMERGENCY_DRAIN_SNAPSHOT,
    EMERGENCY_WITHDRAWAL, LIQUIDITY_POSITIONS,
    PENDING_EMERGENCY_WITHDRAW, POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_PAUSED_AUTO,
    POOL_SPECS, POOL_STATE,
};
use pool_factory_interfaces::{EmergencyWithdrawDelayResponse, FactoryQueryMsg};
use cosmwasm_std::{
    Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo, Response, StdError, Storage, Uint128,
};

/// Bundle returned by `execute_emergency_withdraw_core_drain`. Callers
/// turn it into a `Response` — either directly (standard-pool) or after
/// adding commit-only bookkeeping (creator-pool).
pub struct CoreDrainResult {
    pub messages: Vec<CosmosMsg>,
    pub total_0: Uint128,
    pub total_1: Uint128,
    pub recipient: Addr,
    pub total_liquidity_at_withdrawal: Uint128,
}

/// Checks that the pool has not been permanently drained. Returns
/// `ContractError::EmergencyDrained` if it has.
pub fn ensure_not_drained(storage: &dyn Storage) -> Result<(), ContractError> {
    if EMERGENCY_DRAINED.may_load(storage)?.unwrap_or(false) {
        return Err(ContractError::EmergencyDrained {});
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pause / Unpause
// ---------------------------------------------------------------------------

pub fn execute_pause(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    let pool_contract = pool_info.pool_info.contract_addr.to_string();
    POOL_PAUSED.save(deps.storage, &true)?;
    // Explicit admin pause is "hard" — clear any prior auto-pause
    // state so a deposit-driven auto-unpause can't override the admin's
    // intent. If reserves happen to be low at admin-pause time, recovery
    // requires explicit Unpause, not an opportunistic deposit.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "pause")
        .add_attribute("pool_contract", pool_contract)
        .add_attribute("paused_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

pub fn execute_unpause(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    let pool_contract = pool_info.pool_info.contract_addr.to_string();
    POOL_PAUSED.save(deps.storage, &false)?;
    // Clearing admin pause also clears the auto-flag. The pool is
    // now unpaused regardless of reason — the next swap/remove that
    // drains reserves below MIN will re-arm the auto-pause cleanly.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "unpause")
        .add_attribute("pool_contract", pool_contract)
        .add_attribute("unpaused_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — Phase 1: initiate (pause + 24h timelock)
// ---------------------------------------------------------------------------

pub fn execute_emergency_withdraw_initiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    ensure_not_drained(deps.storage)?;

    // Re-initiating while a timelock is already armed is a caller error —
    // the ExecuteMsg::EmergencyWithdraw handler in each pool kind is what
    // decides whether to dispatch here (Phase 1) or to core_drain (Phase 2).
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(
            "Emergency withdraw already initiated; wait for the timelock to elapse or cancel.",
        )));
    }

    let now = env.block.time;
    POOL_PAUSED.save(deps.storage, &true)?;
    // emergency_withdraw_initiate is a "hard" pause — must not be
    // recoverable via opportunistic deposit. Override any prior auto-flag
    // so the timelock can't be circumvented by a low-liquidity
    // bystander pushing reserves above MIN.
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    // Pull the delay from the factory at runtime so it always reflects the
    // current `factory_config.emergency_withdraw_delay_seconds` (admin-tunable
    // via the standard 48h `ProposeConfigUpdate` flow). A snapshot taken at
    // pool instantiate would silently freeze pre-existing pools at the
    // delay value present when they were spawned, defeating the
    // tunability guarantee.
    let delay: EmergencyWithdrawDelayResponse = deps.querier.query_wasm_smart(
        pool_info.factory_addr.to_string(),
        &FactoryQueryMsg::EmergencyWithdrawDelaySeconds {},
    )?;
    let effective_after = now.plus_seconds(delay.delay_seconds);
    PENDING_EMERGENCY_WITHDRAW.save(deps.storage, &effective_after)?;

    Ok(Response::new()
        .add_attribute("action", "emergency_withdraw_initiated")
        .add_attribute("effective_after", effective_after.to_string())
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("initiated_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — Phase 2: core drain
// ---------------------------------------------------------------------------

/// Drains the pool-held balances that this module can see, splitting
/// the funds into two streams (H-NFT-4 audit fix):
///
///   - **LP-owned funds** (`pool_state.reserve0/1` + `fee_reserve_0/1`):
///     stay in the pool's bank balance under an
///     `EMERGENCY_DRAIN_SNAPSHOT` escrow. Each LP position can call
///     `ClaimEmergencyShare` for its pro-rata portion at any time
///     during the `EMERGENCY_CLAIM_DORMANCY_SECONDS` (1-year) window.
///     After the window, the unclaimed remainder is sweepable to the
///     bluechip wallet by the factory admin via
///     `SweepUnclaimedEmergencyShares`.
///
///   - **Non-LP funds** (`CREATOR_FEE_POT` + caller-supplied
///     `accumulation_drain_*`): swept to the bluechip wallet
///     immediately, matching the pre-fix economics for those buckets.
///     `CREATOR_FEE_POT` and `accumulation_drain_*` are not part of
///     any LP's claim — `accumulation_drain_*` is the creator-pool's
///     `CREATOR_EXCESS_POSITION` (creator-owned), and
///     `CREATOR_FEE_POT` is the protocol's clip-slice accumulator.
///
/// Pre-fix, the function swept ALL pool funds (including LP reserves
/// and pending fees) to `bluechip_wallet_address` after a 24-hour
/// timelock. The 24h window allowed active LPs to exit, but
/// set-and-forget LPs lost their funds entirely. The escrow pattern
/// preserves the 24h pause-with-LP-exits semantics AND gives passive
/// LPs a year to surface and claim their share — substantially closing
/// the "passive LP loses everything to treasury" gap.
///
/// Writes `EMERGENCY_WITHDRAWAL` with the swept (non-LP) totals,
/// writes `EMERGENCY_DRAIN_SNAPSHOT` with the LP-owned snapshot,
/// zeroes `pool_state.reserve_*`, `pool_state.total_liquidity`, and
/// `fee_reserve_*`, removes `CREATOR_FEE_POT`, and flips
/// `EMERGENCY_DRAINED` to true. After a successful call, the pool is
/// permanently drained — any subsequent `ensure_not_drained()` check
/// rejects further admin / liquidity / trading actions, but
/// `ClaimEmergencyShare` and `SweepUnclaimedEmergencyShares` remain
/// callable.
pub fn execute_emergency_withdraw_core_drain(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    accumulation_drain_0: Uint128,
    accumulation_drain_1: Uint128,
) -> Result<CoreDrainResult, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    ensure_not_drained(deps.storage)?;

    let effective_after = PENDING_EMERGENCY_WITHDRAW
        .may_load(deps.storage)?
        .ok_or_else(|| {
            ContractError::Std(StdError::generic_err(
                "Emergency withdraw has not been initiated.",
            ))
        })?;

    if env.block.time < effective_after {
        return Err(ContractError::EmergencyTimelockPending { effective_after });
    }

    PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    let mut pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;

    // LP-owned funds — stay in the pool under the escrow snapshot.
    let lp_reserve_0 = pool_state.reserve0;
    let lp_reserve_1 = pool_state.reserve1;
    let lp_fee_reserve_0 = pool_fee_state.fee_reserve_0;
    let lp_fee_reserve_1 = pool_fee_state.fee_reserve_1;
    let total_liquidity_at_drain = pool_state.total_liquidity;

    // Non-LP funds — swept to bluechip wallet now.
    let mut sweep_0 = Uint128::zero();
    let mut sweep_1 = Uint128::zero();
    if let Some(pot) = CREATOR_FEE_POT.may_load(deps.storage)? {
        sweep_0 = sweep_0.checked_add(pot.amount_0)?;
        sweep_1 = sweep_1.checked_add(pot.amount_1)?;
        CREATOR_FEE_POT.remove(deps.storage);
    }
    sweep_0 = sweep_0.checked_add(accumulation_drain_0)?;
    sweep_1 = sweep_1.checked_add(accumulation_drain_1)?;

    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let recipient = fee_info.bluechip_wallet_address.clone();

    // Audit record reflects ONLY the funds actually swept to the
    // bluechip wallet at drain time. LP-claimable shares are recorded
    // separately on the EMERGENCY_DRAIN_SNAPSHOT and are NOT counted
    // here — that's the load-bearing semantic of the H-NFT-4 fix.
    let withdrawal_info = EmergencyWithdrawalInfo {
        withdrawn_at: env.block.time.seconds(),
        recipient: recipient.clone(),
        amount0: sweep_0,
        amount1: sweep_1,
        total_liquidity_at_withdrawal: total_liquidity_at_drain,
    };
    EMERGENCY_WITHDRAWAL.save(deps.storage, &withdrawal_info)?;

    // Snapshot the LP-owned funds for per-position claims. Funds stay
    // in the pool's bank balance until claimed (or until the dormancy
    // window expires and treasury sweeps the unclaimed remainder).
    let drained_at = env.block.time;
    let dormancy_expires_at = drained_at.plus_seconds(EMERGENCY_CLAIM_DORMANCY_SECONDS);
    EMERGENCY_DRAIN_SNAPSHOT.save(
        deps.storage,
        &EmergencyDrainSnapshot {
            drained_at,
            dormancy_expires_at,
            reserve0_at_drain: lp_reserve_0,
            reserve1_at_drain: lp_reserve_1,
            fee_reserve_0_at_drain: lp_fee_reserve_0,
            fee_reserve_1_at_drain: lp_fee_reserve_1,
            total_liquidity_at_drain,
            total_claimed_0: Uint128::zero(),
            total_claimed_1: Uint128::zero(),
            residual_swept: false,
        },
    )?;

    pool_state.reserve0 = Uint128::zero();
    pool_state.reserve1 = Uint128::zero();
    pool_state.total_liquidity = Uint128::zero();
    POOL_STATE.save(deps.storage, &pool_state)?;

    pool_fee_state.fee_reserve_0 = Uint128::zero();
    pool_fee_state.fee_reserve_1 = Uint128::zero();
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;

    EMERGENCY_DRAINED.save(deps.storage, &true)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    if !sweep_0.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[0].clone(),
                amount: sweep_0,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }
    if !sweep_1.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[1].clone(),
                amount: sweep_1,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }

    Ok(CoreDrainResult {
        messages,
        total_0: sweep_0,
        total_1: sweep_1,
        recipient,
        total_liquidity_at_withdrawal: total_liquidity_at_drain,
    })
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — per-position claim escrow (H-NFT-4 audit fix)
// ---------------------------------------------------------------------------

/// Per-position pro-rata claim against the post-drain
/// `EMERGENCY_DRAIN_SNAPSHOT`. Permissionless (gated only by CW721
/// ownership of `position_id`); a position holder calls
/// `ClaimEmergencyShare` to retrieve their share of the LP-owned
/// reserves and pending fees that were captured in the snapshot.
///
/// Math: pure liquidity-weighted pro-rata against
/// `total_liquidity_at_drain`. The snapshot's `reserve_*_at_drain +
/// fee_reserve_*_at_drain` is split among positions in proportion to
/// each position's liquidity at drain time. A more "fair"
/// alternative would walk fee-growth checkpoints to give long-time
/// LPs a larger fee share, but the emergency-drain context (pool is
/// being shut down, funds are leaving the protocol regardless) makes
/// equal-by-liquidity the right tradeoff for code simplicity.
///
/// Floor-division dust: `Σ shares ≤ lp_drainable` by integer-division
/// rounding. Whatever doesn't get claimed (dust + truly abandoned
/// positions) flows to `SweepUnclaimedEmergencyShares` after the
/// 1-year dormancy.
///
/// Double-claim prevention: a successful claim sets
/// `position.liquidity = 0` and zeroes `unclaimed_fees_*`. A second
/// claim attempt computes `share = 0 × X / total_liquidity = 0` and
/// rejects with `NoClaimableEmergencyShare`. Storage row stays alive
/// (consistent with H-NFT-1 empty-position semantics) but is
/// economically spent.
pub fn execute_claim_emergency_share(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: String,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;

    // Pool must actually be drained — otherwise there's nothing to
    // claim against. Mirrors the inverse semantics of
    // `ensure_not_drained` used everywhere else.
    if !EMERGENCY_DRAINED
        .may_load(deps.storage)?
        .unwrap_or(false)
    {
        return Err(ContractError::NoEmergencyDrainSnapshot);
    }

    let mut snapshot = EMERGENCY_DRAIN_SNAPSHOT
        .may_load(deps.storage)?
        .ok_or(ContractError::NoEmergencyDrainSnapshot)?;

    // Hard-close per-position claims once `SweepUnclaimedEmergencyShares`
    // has fired. Pre-fix, late claims were tolerated in principle (bank
    // module would reject if balance insufficient), but the snapshot's
    // `total_claimed_*` tally would still get bumped, producing an
    // inconsistent record where cumulative claims exceeded drainable.
    // Matches the documented design intent ("after 1 year, abandoned
    // funds are gone") and gives off-chain observers a clean signal
    // that the claim window has closed.
    if snapshot.residual_swept {
        return Err(ContractError::EmergencyClaimsClosedPostSweep);
    }

    // CW721 ownership gate. Mirrors every other position-mutating
    // handler — current NFT holder is the only authorized claimant.
    // sync_position_on_transfer aligns the storage `position.owner`
    // with the CW721 owner if the NFT changed hands post-drain.
    verify_position_ownership(
        deps.as_ref(),
        &pool_info.position_nft_address,
        &position_id,
        &info.sender,
    )?;
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let mut position = LIQUIDITY_POSITIONS.load(deps.storage, &position_id)?;
    sync_position_on_transfer(
        deps.storage,
        &mut position,
        &position_id,
        &info.sender,
        &pool_fee_state,
    )?;

    if position.liquidity.is_zero() || snapshot.total_liquidity_at_drain.is_zero() {
        return Err(ContractError::NoClaimableEmergencyShare {
            position_id: position_id.clone(),
        });
    }

    // Pro-rata math: principal share + fee share, both weighted by
    // `position.liquidity / total_liquidity_at_drain`.
    let principal_0 = snapshot.reserve0_at_drain.multiply_ratio(
        position.liquidity,
        snapshot.total_liquidity_at_drain,
    );
    let principal_1 = snapshot.reserve1_at_drain.multiply_ratio(
        position.liquidity,
        snapshot.total_liquidity_at_drain,
    );
    let fee_share_0 = snapshot.fee_reserve_0_at_drain.multiply_ratio(
        position.liquidity,
        snapshot.total_liquidity_at_drain,
    );
    let fee_share_1 = snapshot.fee_reserve_1_at_drain.multiply_ratio(
        position.liquidity,
        snapshot.total_liquidity_at_drain,
    );
    let total_0 = principal_0.checked_add(fee_share_0)?;
    let total_1 = principal_1.checked_add(fee_share_1)?;

    if total_0.is_zero() && total_1.is_zero() {
        // Position too small to round up to even one base unit on
        // either side — treat as nothing to claim rather than
        // emitting empty-amount transfer messages.
        return Err(ContractError::NoClaimableEmergencyShare {
            position_id: position_id.clone(),
        });
    }

    // Mark position as economically spent. Storage row stays so the
    // H-NFT-1 empty-position invariant holds; subsequent claims see
    // `position.liquidity == 0` and reject.
    let claimed_liquidity = position.liquidity;
    position.liquidity = Uint128::zero();
    position.unclaimed_fees_0 = Uint128::zero();
    position.unclaimed_fees_1 = Uint128::zero();
    LIQUIDITY_POSITIONS.save(deps.storage, &position_id, &position)?;

    // Bump the running tally on the snapshot. Saturate-add for
    // defense-in-depth — the sum of all claims cannot exceed
    // `lp_drainable` mathematically, but use checked_add to
    // surface any future bug as an explicit overflow rather than
    // silently overflowing.
    snapshot.total_claimed_0 = snapshot.total_claimed_0.checked_add(total_0)?;
    snapshot.total_claimed_1 = snapshot.total_claimed_1.checked_add(total_1)?;
    EMERGENCY_DRAIN_SNAPSHOT.save(deps.storage, &snapshot)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    if !total_0.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[0].clone(),
                amount: total_0,
            }
            .into_msg(&deps.querier, info.sender.clone())?,
        );
    }
    if !total_1.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[1].clone(),
                amount: total_1,
            }
            .into_msg(&deps.querier, info.sender.clone())?,
        );
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "claim_emergency_share")
        .add_attribute("position_id", position_id)
        .add_attribute("claimant", info.sender.to_string())
        .add_attribute("claimed_liquidity", claimed_liquidity.to_string())
        .add_attribute("principal_0", principal_0.to_string())
        .add_attribute("principal_1", principal_1.to_string())
        .add_attribute("fee_share_0", fee_share_0.to_string())
        .add_attribute("fee_share_1", fee_share_1.to_string())
        .add_attribute("total_0", total_0.to_string())
        .add_attribute("total_1", total_1.to_string())
        .add_attribute(
            "snapshot_total_claimed_0",
            snapshot.total_claimed_0.to_string(),
        )
        .add_attribute(
            "snapshot_total_claimed_1",
            snapshot.total_claimed_1.to_string(),
        )
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

/// Factory-only post-dormancy sweep of the unclaimed residual.
///
/// After `EMERGENCY_CLAIM_DORMANCY_SECONDS` (1 year) elapses from the
/// drain timestamp, the factory admin may invoke this to send the
/// still-unclaimed remainder of the LP escrow to `bluechip_wallet`.
/// The remainder is `(reserve_*_at_drain + fee_reserve_*_at_drain) -
/// total_claimed_*` per asset side — both floor-division dust and
/// truly abandoned positions whose owners never returned.
///
/// `residual_swept` flag prevents double-sweeps; a second call after
/// the first succeeded fails with `NoUnclaimedEmergencyResidual`.
/// `execute_claim_emergency_share` also gates on the same flag and
/// rejects with `EmergencyClaimsClosedPostSweep` once it flips — the
/// claim window is hard-closed at sweep time, matching the documented
/// "after 1 year, abandoned funds are gone" design intent.
pub fn execute_sweep_unclaimed_emergency_shares(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    if !EMERGENCY_DRAINED
        .may_load(deps.storage)?
        .unwrap_or(false)
    {
        return Err(ContractError::NoEmergencyDrainSnapshot);
    }

    let mut snapshot = EMERGENCY_DRAIN_SNAPSHOT
        .may_load(deps.storage)?
        .ok_or(ContractError::NoEmergencyDrainSnapshot)?;

    if env.block.time < snapshot.dormancy_expires_at {
        return Err(ContractError::EmergencyClaimDormancyNotElapsed {
            drained_at: snapshot.drained_at.seconds(),
            dormancy_expires_at: snapshot.dormancy_expires_at.seconds(),
            now: env.block.time.seconds(),
        });
    }

    if snapshot.residual_swept {
        return Err(ContractError::NoUnclaimedEmergencyResidual);
    }

    // Total LP-drainable per side at drain time.
    let total_0 = snapshot
        .reserve0_at_drain
        .checked_add(snapshot.fee_reserve_0_at_drain)?;
    let total_1 = snapshot
        .reserve1_at_drain
        .checked_add(snapshot.fee_reserve_1_at_drain)?;

    // Residual = drained - already_claimed. saturating_sub is
    // defensive; total_claimed_* should never exceed drainable by
    // construction, but a future math change shouldn't underflow
    // here.
    let residual_0 = total_0.saturating_sub(snapshot.total_claimed_0);
    let residual_1 = total_1.saturating_sub(snapshot.total_claimed_1);

    if residual_0.is_zero() && residual_1.is_zero() {
        return Err(ContractError::NoUnclaimedEmergencyResidual);
    }

    snapshot.residual_swept = true;
    EMERGENCY_DRAIN_SNAPSHOT.save(deps.storage, &snapshot)?;

    let fee_info = COMMITFEEINFO.load(deps.storage)?;
    let recipient = fee_info.bluechip_wallet_address;

    let mut messages: Vec<CosmosMsg> = vec![];
    if !residual_0.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[0].clone(),
                amount: residual_0,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }
    if !residual_1.is_zero() {
        messages.push(
            TokenInfo {
                info: pool_info.pool_info.asset_infos[1].clone(),
                amount: residual_1,
            }
            .into_msg(&deps.querier, recipient.clone())?,
        );
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "sweep_unclaimed_emergency_shares")
        .add_attribute("recipient", recipient.to_string())
        .add_attribute("residual_0", residual_0.to_string())
        .add_attribute("residual_1", residual_1.to_string())
        .add_attribute("dormancy_expired_at", snapshot.dormancy_expires_at.to_string())
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Emergency Withdraw — cancel (pre-drain only)
// ---------------------------------------------------------------------------

pub fn execute_cancel_emergency_withdraw(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_none() {
        return Err(ContractError::NoPendingEmergencyWithdraw {});
    }
    PENDING_EMERGENCY_WITHDRAW.remove(deps.storage);
    POOL_PAUSED.save(deps.storage, &false)?;
    // Emergency cancel clears any auto-flag (the cancel returns
    // the pool to fully open state).
    POOL_PAUSED_AUTO.save(deps.storage, &false)?;
    Ok(Response::new()
        .add_attribute("action", "emergency_withdraw_cancelled")
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("cancelled_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

// ---------------------------------------------------------------------------
// Config update (factory-only)
// ---------------------------------------------------------------------------

pub fn execute_update_config_from_factory(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    update: PoolConfigUpdate,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let mut attributes = vec![("action", "update_config")];
    let mut specs = POOL_SPECS.load(deps.storage)?;
    let mut specs_changed = false;

    if let Some(fee) = update.lp_fee {
        let max_lp_fee = Decimal::percent(10);
        let min_lp_fee = Decimal::permille(1); // 0.1%
        if fee > max_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must not exceed 10% (0.1)",
            )));
        }
        if fee < min_lp_fee {
            return Err(ContractError::Std(StdError::generic_err(
                "lp_fee must be at least 0.1% (0.001)",
            )));
        }
        specs.lp_fee = fee;
        specs_changed = true;
        attributes.push(("lp_fee", "updated"));
    }

    if let Some(interval) = update.min_commit_interval {
        const MAX_COMMIT_INTERVAL: u64 = 86_400; // 24 hours
        if interval > MAX_COMMIT_INTERVAL {
            return Err(ContractError::Std(StdError::generic_err(
                "min_commit_interval must not exceed 86400 seconds (1 day)",
            )));
        }
        specs.min_commit_interval = interval;
        specs_changed = true;
        attributes.push(("min_commit_interval", "updated"));
    }

    if specs_changed {
        POOL_SPECS.save(deps.storage, &specs)?;
    }

    // `update.min_commit_usd_pre_threshold` and
    // `update.min_commit_usd_post_threshold` are intentionally NOT applied
    // here. They live on `creator-pool::CommitLimitInfo`, which is
    // creator-pool-only state — pool-core has no compile-time access to
    // it. The creator-pool dispatch in `creator-pool::contract.rs::execute`
    // wraps this handler: it reads the commit-floor fields off `update`,
    // applies them to `COMMIT_LIMIT_INFO`, and only then delegates to
    // this function for the shared knobs (lp_fee + min_commit_interval).
    // Standard-pool's dispatch calls this handler directly and ignores
    // the commit-floor fields entirely (standard pools have no commit
    // phase); the factory-side `validate()` rejects standard-pool
    // proposals carrying those fields at propose time, so a standard-pool
    // apply that reaches here can only have `None` for both.

    // Per-pool `oracle_address` rotation removed (audit fix). The oracle
    // endpoint is pinned at instantiate to the factory address and is
    // no longer mutable through the per-pool config flow. If the
    // protocol ever splits the oracle off the factory, the rerouting
    // path is a coordinated `UpgradePools` migration that writes
    // ORACLE_INFO directly — not a runtime config knob.

    Ok(Response::new()
        .add_attributes(attributes)
        .add_attribute(
            "pool_contract",
            pool_info.pool_info.contract_addr.to_string(),
        )
        .add_attribute("updated_by", info.sender.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}

/// Two-phase emergency-withdraw dispatcher shared by `creator-pool` and
/// `standard-pool`. Picks Phase 1 (initiate) or Phase 2 (core drain)
/// based on whether `PENDING_EMERGENCY_WITHDRAW` is already set, and
/// builds the canonical `action="emergency_withdraw"` response on the
/// drain side. Each pool wasm wraps this helper to layer in any
/// pool-kind-specific bookkeeping (creator-pool sweeps
/// `CREATOR_EXCESS_POSITION` and halts `DISTRIBUTION_STATE` between
/// the dispatch and the drain; standard-pool just calls through with
/// zero `accumulation_drain_*`).
///
/// `accumulation_drain_0` / `_1` are the additional reserve-0 / reserve-1
/// amounts the caller's pool-kind-specific bookkeeping wants folded
/// into the drain transfer messages (creator-pool passes its excess
/// position; standard-pool passes zero). They are forwarded to the
/// core drain so the pool-core audit record captures the grand total.
pub fn execute_emergency_withdraw_dispatch(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    accumulation_drain_0: Uint128,
    accumulation_drain_1: Uint128,
) -> Result<Response, ContractError> {
    if PENDING_EMERGENCY_WITHDRAW.may_load(deps.storage)?.is_none() {
        return execute_emergency_withdraw_initiate(deps, env, info);
    }
    let drain = execute_emergency_withdraw_core_drain(
        deps,
        env.clone(),
        info,
        accumulation_drain_0,
        accumulation_drain_1,
    )?;
    Ok(Response::new()
        .add_messages(drain.messages)
        .add_attribute("action", "emergency_withdraw")
        .add_attribute("recipient", drain.recipient)
        .add_attribute("amount0", drain.total_0)
        .add_attribute("amount1", drain.total_1)
        .add_attribute("total_liquidity", drain.total_liquidity_at_withdrawal)
        .add_attribute("pool_contract", env.contract.address.to_string())
        .add_attribute("block_height", env.block.height.to_string())
        .add_attribute("block_time", env.block.time.seconds().to_string()))
}
