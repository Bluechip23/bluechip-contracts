//! Pool contract entry points: instantiate, execute dispatch, swap, migrate.
//!
//! Commit logic lives in [`crate::commit`], admin operations in [`crate::admin`].

use crate::admin::{
    ensure_not_drained, execute_cancel_emergency_withdraw, execute_claim_emergency_share,
    execute_claim_failed_distribution, execute_emergency_withdraw, execute_pause,
    execute_recover_stuck_states, execute_self_recover_distribution,
    execute_sweep_unclaimed_emergency_shares, execute_unpause,
    execute_update_config_from_factory,
};
use crate::asset::{PoolPairType, TokenInfoPoolExt, TokenType};
use crate::commit::{commit, execute_continue_distribution};
use crate::error::ContractError;
use crate::generic_helpers::validate_pool_threshold_payments;
use crate::liquidity::{
    execute_add_to_position_with_verify, execute_collect_fees,
    execute_deposit_liquidity_with_verify, execute_remove_all_liquidity,
    execute_remove_partial_liquidity, execute_remove_partial_liquidity_by_percent,
};
use crate::liquidity_helpers::{execute_claim_creator_excess, execute_claim_creator_fees};
use crate::msg::{ExecuteMsg, MigrateMsg, PoolInstantiateMsg};
use crate::query::query_check_commit;
use crate::state::{
    CommitLimitInfo, DEFAULT_LP_FEE, DEFAULT_SWAP_RATE_LIMIT_SECS, ExpectedFactory, MAX_LP_FEE,
    MIN_LP_FEE, OracleInfo, PoolAnalytics,
    PoolDetails, PoolFeeState, PoolInfo, PoolSpecs, PoolState, Position, ThresholdPayoutAmounts,
    COMMITFEEINFO, COMMIT_LIMIT_INFO, EXPECTED_FACTORY, IS_THRESHOLD_HIT, LIQUIDITY_POSITIONS,
    DEPOSIT_VERIFY_REPLY_ID, FAILED_MINTS, NATIVE_RAISED_FROM_COMMIT, NEXT_POSITION_ID,
    ORACLE_INFO, OWNER_POSITIONS, PENDING_FACTORY_NOTIFY, PENDING_MINT_REPLIES, POOL_ANALYTICS,
    POOL_FEE_STATE, POOL_INFO, POOL_PAUSED, POOL_SPECS, POOL_STATE,
    REPLY_ID_DISTRIBUTION_MINT_BASE, REPLY_ID_FACTORY_NOTIFY_INITIAL,
    REPLY_ID_FACTORY_NOTIFY_RETRY, THRESHOLD_PAYOUT_AMOUNTS, USD_RAISED_FROM_COMMIT,
};
// Swap orchestration moved to pool_core::swap; re-exported via swap_helper.
use crate::swap_helper::{execute_swap_cw20, simple_swap};
use pool_core::balance_verify::handle_deposit_verify_reply;
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo,
    Reply, Response, StdError, StdResult, Storage, SubMsg, SubMsgResult, Uint128, WasmMsg,
};
use cw2::set_contract_version;

/// cw2 contract name. Includes the `creator` discriminator so a
/// migration tool inspecting cw2 names can distinguish a creator-pool
/// from a `standard-pool` (`bluechip-contracts-standard-pool`).
/// Pre-rename pools migrating up will fail any cw2-name check; that's
/// the desired behaviour — name drift across pool kinds is exactly
/// the foot-gun this rename closes.
const CONTRACT_NAME: &str = "bluechip-contracts-creator-pool";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Instantiate
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    let cfg = ExpectedFactory {
        expected_factory_address: msg.used_factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;
    if info.sender != cfg.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }

    msg.pool_token_info[0].check(deps.api)?;
    msg.pool_token_info[1].check(deps.api)?;
    if msg.pool_token_info[0] == msg.pool_token_info[1] {
        return Err(ContractError::DoublingAssets {});
    }

    // Enforce strict pair shape AND ordering: index 0 = Bluechip
    // (Native), index 1 = CreatorToken matching `msg.token_address`.
    // Every downstream piece of commit/swap/threshold-payout code
    // hard-codes `reserve0 == bluechip, reserve1 == creator-token`,
    // so a reversed pair would silently produce wrong-direction swaps.
    // Defense-in-depth — the factory's validate_pool_token_info enforces
    // the same invariant — but rejecting again here means a buggy
    // factory migration or a directly-instantiated pool (e.g. via a raw
    // Wasm instantiate bypassing the factory entirely) can't silently
    // produce a pool whose reserve accounting disagrees with its
    // holdings.
    match (&msg.pool_token_info[0], &msg.pool_token_info[1]) {
        (TokenType::Native { denom }, TokenType::CreatorToken { contract_addr }) => {
            if denom.trim().is_empty() {
                return Err(ContractError::InvalidPairShape {
                    reason: "Bluechip denom must be non-empty".to_string(),
                });
            }
            if contract_addr != &msg.token_address {
                return Err(ContractError::InvalidPairShape {
                    reason: "CreatorToken.contract_addr in pool_token_info must equal \
                             msg.token_address"
                        .to_string(),
                });
            }
        }
        _ => {
            return Err(ContractError::InvalidPairShape {
                reason: "pool_token_info must be [Bluechip(Native), CreatorToken] — order \
                         matters: bluechip at index 0, creator-token at index 1."
                    .to_string(),
            });
        }
    }
    if (msg.commit_fee_info.commit_fee_bluechip + msg.commit_fee_info.commit_fee_creator)
        > Decimal::one()
    {
        return Err(ContractError::InvalidFee {});
    }

    let threshold_payout_amounts = if let Some(params_binary) = msg.threshold_payout {
        let params: ThresholdPayoutAmounts = from_json(params_binary)?;
        validate_pool_threshold_payments(&params)?;
        params
    } else {
        return Err(ContractError::InvalidThresholdParams {
            msg: "Your params could not be validated during pool instantiation.".to_string(),
        });
    };

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pool_info: PoolDetails {
            contract_addr: env.contract.address.clone(),
            asset_infos: msg.pool_token_info.clone(),
            pool_type: PoolPairType::Xyk {},
        },
        factory_addr: msg.used_factory_addr.clone(),
        token_address: msg.token_address.clone(),
        position_nft_address: msg.position_nft_address.clone(),
    };

    let liquidity_position = Position {
        liquidity: Uint128::zero(),
        owner: env.contract.address.clone(),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
        fee_size_multiplier: Decimal::one(),
        unclaimed_fees_0: Uint128::zero(),
        unclaimed_fees_1: Uint128::zero(),
        // Sentinel position at id "0" — no actual liquidity, no lock.
        locked_liquidity: Uint128::zero(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: DEFAULT_LP_FEE,
        min_commit_interval: DEFAULT_SWAP_RATE_LIMIT_SECS,
    };

    let commit_config = CommitLimitInfo {
        commit_amount_for_threshold_usd: msg.commit_threshold_limit_usd,
        max_bluechip_lock_per_pool: msg.max_bluechip_lock_per_pool,
        creator_excess_liquidity_lock_days: msg.creator_excess_liquidity_lock_days,
        min_commit_usd_pre_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_PRE_THRESHOLD,
        min_commit_usd_post_threshold: crate::state::DEFAULT_MIN_COMMIT_USD_POST_THRESHOLD,
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.used_factory_addr.clone(),
    };

    let pool_state = PoolState {
        pool_contract_address: env.contract.address.clone(),
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(),
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        nft_ownership_accepted: false,
    };

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
        fee_reserve_0: Uint128::zero(),
        fee_reserve_1: Uint128::zero(),
    };

    USD_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    COMMITFEEINFO.save(deps.storage, &msg.commit_fee_info)?;
    NATIVE_RAISED_FROM_COMMIT.save(deps.storage, &Uint128::zero())?;
    // Creator pools start pre-threshold — swap / liquidity entry points
    // gate on this until `process_threshold_crossing_with_excess` flips
    // it to `true` during the threshold-crossing commit.
    IS_THRESHOLD_HIT.save(deps.storage, &false)?;
    // Creator pools use the legacy `fee_size_multiplier` dust-griefing
    // deterrent — small positions accrue with a clipped multiplier whose
    // clipped slice flows to CREATOR_FEE_POT, claimable by the creator.
    // Standard pools instantiate with this flag set to `false`; see the
    // doc-comment on `APPLY_DUST_MULTIPLIER` in pool-core::state.
    pool_core::state::APPLY_DUST_MULTIPLIER.save(deps.storage, &true)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT_AMOUNTS.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_LIMIT_INFO.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    OWNER_POSITIONS.save(deps.storage, (&env.contract.address, "0"), &true)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    POOL_ANALYTICS.save(deps.storage, &PoolAnalytics::default())?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool_kind", crate::state::POOL_KIND_COMMIT)
        .add_attribute("pool_contract", env.contract.address.to_string()))
}


// ---------------------------------------------------------------------------
// Execute dispatch
// ---------------------------------------------------------------------------

/// Pause-only gate. Used by `Commit` (pre-threshold path) where the pool
/// has no reserves yet, so the drain check from `check_pool_writable`
/// doesn't apply. Identical inlined check was repeated 9× across the
/// dispatch arms before this extraction.
fn check_pool_not_paused(storage: &dyn Storage) -> Result<(), ContractError> {
    if POOL_PAUSED.may_load(storage)?.unwrap_or(false) {
        return Err(ContractError::PoolPausedLowLiquidity {});
    }
    Ok(())
}

/// Strict liquidity gate: hard-rejects whenever the pool is paused
/// for ANY reason — admin Pause, emergency-pending, or auto-pause due
/// to low liquidity — and rejects when the pool is permanently
/// drained.
///
/// Used by the creator-only claim paths (`ClaimCreatorExcessLiquidity`,
/// `ClaimCreatorFees`). LP-mutating paths use the more permissive
/// gates instead:
/// - `check_pool_writable_for_remove` permits LP exits during the
/// emergency-withdraw timelock window so LPs can race the drain.
/// - `check_pool_writable_for_deposit` permits deposits during
/// auto-pause so reserves can be restored.
///
/// Creator claims do not benefit from those relaxations: the creator
/// excess + fee pot are admin-controlled fund flows, and the safer
/// default during any non-None pause is to wait for the explicit
/// resume signal (admin Unpause / emergency cancel).
fn check_pool_writable(storage: &dyn Storage) -> Result<(), ContractError> {
    ensure_not_drained(storage)?;
    check_pool_not_paused(storage)
}

/// Deposit-side gate: rejects hard pauses (admin / emergency-pending)
/// but PERMITS auto-pause-on-low-liquidity. The deposit handler then
/// either (a) restores reserves above MIN and auto-clears both flags,
/// or (b) leaves the pool still auto-paused if the deposit didn't
/// restore enough. Either way the deposit completes and adds
/// liquidity.
///
/// Auto-pause vs. hard pause distinction:
/// - `PauseKind::AutoLowLiquidity`: this gate accepts. The deposit's
/// post-state branch in `execute_deposit_liquidity_inner` clears the
/// flags if reserves recover.
/// - `PauseKind::EmergencyPending` / `PauseKind::Hard`: rejected —
/// emergency-pending pool must wait for the 24h timelock or admin
/// cancel; admin-paused pool must wait for explicit Unpause.
/// Letting fresh capital into a pool that is about to be drained
/// would funnel new deposits into the emergency-drain recipient.
fn check_pool_writable_for_deposit(storage: &dyn Storage) -> Result<(), ContractError> {
    use crate::state::{pause_kind, PauseKind};
    ensure_not_drained(storage)?;
    match pause_kind(storage)? {
        PauseKind::None | PauseKind::AutoLowLiquidity => Ok(()),
        PauseKind::EmergencyPending | PauseKind::Hard => {
            Err(ContractError::PoolPausedLowLiquidity {})
        }
    }
}

/// LP-exit gate. Permits `Remove*Liquidity` and `CollectFees` while
/// the pool is open OR while it's in the 24h emergency-withdraw
/// timelock window (PauseKind::EmergencyPending). Auto-pause and
/// admin Hard pause still reject — same rationale as standard-pool's
/// equivalent helper.
///
/// Closes the LP-trap window surfaced: without
/// this, post-threshold LPs whose pool is emergency-withdrawn cannot
/// exit during the timelock and lose their entire principal on the
/// Phase-2 drain.
fn check_pool_writable_for_remove(storage: &dyn Storage) -> Result<(), ContractError> {
    use crate::state::{pause_kind, PauseKind};
    ensure_not_drained(storage)?;
    match pause_kind(storage)? {
        PauseKind::None | PauseKind::EmergencyPending => Ok(()),
        PauseKind::AutoLowLiquidity | PauseKind::Hard => {
            Err(ContractError::PoolPausedLowLiquidity {})
        }
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        // --- Admin ---
        ExecuteMsg::UpdateConfigFromFactory { update } => {
            execute_update_creator_config_from_factory(deps, env, info, update)
        }
        ExecuteMsg::Pause {} => execute_pause(deps, env, info),
        ExecuteMsg::Unpause {} => execute_unpause(deps, env, info),
        ExecuteMsg::EmergencyWithdraw {} => execute_emergency_withdraw(deps, env, info),
        ExecuteMsg::CancelEmergencyWithdraw {} => {
            execute_cancel_emergency_withdraw(deps, env, info)
        }
        ExecuteMsg::RecoverStuckStates { recovery_type } => {
            execute_recover_stuck_states(deps, env, info, recovery_type)
        }

        // --- Commit & Distribution (commit-pool only) ---
        ExecuteMsg::Commit {
            asset,
            transaction_deadline,
            belief_price,
            max_spread,
        } => {
            // Block ALL commits while paused — pre-threshold AND post-threshold.
            // Previously only process_post_threshold_commit checked POOL_PAUSED,
            // so admin pauses failed to stop pre-threshold deposits, letting
            // users trap funds in the COMMIT_LEDGER of a paused pool.
            check_pool_not_paused(deps.storage)?;
            commit(
                deps,
                env,
                info,
                asset,
                transaction_deadline,
                belief_price,
                max_spread,
            )
        }
        ExecuteMsg::ContinueDistribution {} => {
            execute_continue_distribution(deps, env, info)
        }

        // --- Swap ---
        ExecuteMsg::SimpleSwap {
            offer_asset,
            belief_price,
            max_spread,
            allow_high_max_spread,
            to,
            transaction_deadline,
        } => {
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            offer_asset.confirm_sent_native_balance(&info)?;
            let sender_addr = info.sender.clone();
            let to_addr: Option<Addr> = to
                .map(|to_str| deps.api.addr_validate(&to_str))
                .transpose()?;
            simple_swap(
                deps,
                env,
                info,
                sender_addr,
                offer_asset,
                belief_price,
                max_spread,
                allow_high_max_spread,
                to_addr,
                transaction_deadline,
            )
        }
        ExecuteMsg::Receive(cw20_msg) => execute_swap_cw20(deps, env, info, cw20_msg),

        // --- Liquidity ---
        // Pause checks are now applied to EVERY liquidity-touching path.
        // Previously only CollectFees honored POOL_PAUSED; deposits and
        // removes could run unchecked while the pool was paused (e.g. mid
        // emergency-withdraw window), which could either funnel fresh LP
        // capital into a pending drain or let LPs race the drain. The
        // drain-initiated path already flips POOL_PAUSED on, so a single
        // check blocks both admin-pause and emergency-pending states.
        ExecuteMsg::DepositLiquidity {
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            // Deposits use the auto-pause-tolerant gate so they can
            // restore liquidity in an auto-paused pool. Hard pauses still
            // reject.
            check_pool_writable_for_deposit(deps.storage)?;
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            // route every CW20-bearing deposit through the
            // balance-verify variant. The pre-fix path skipped the
            // pre/post snapshot under the assumption that the pool's
            // CW20 is always a vanilla cw20-base freshly minted by the
            // factory — true today, but a single careless future
            // `update_pool_token_address` admin path or factory upgrade
            // permitting third-party CW20s would let reserves drift
            // from on-chain balances silently. Two extra balance
            // queries per deposit is negligible relative to the
            // strength of the invariant; the verify reply rolls the
            // entire tx back on any delta mismatch (see
            // pool_core::balance_verify::handle_deposit_verify_reply).
            execute_deposit_liquidity_with_verify(
                deps,
                env,
                info,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            min_amount0,
            min_amount1,
            transaction_deadline,
        } => {
            // Same recovery semantics as DepositLiquidity.
            check_pool_writable_for_deposit(deps.storage)?;
            if !query_check_commit(deps.as_ref())? {
                return Err(ContractError::ShortOfThreshold {});
            }
            let sender = info.sender.clone();
            // same balance-verify rationale as
            // DepositLiquidity above. Also closes the implicit-trust
            // gap on the add-to-position path.
            execute_add_to_position_with_verify(
                deps,
                env,
                info,
                position_id,
                sender,
                amount0,
                amount1,
                min_amount0,
                min_amount1,
                transaction_deadline,
            )
        }
        ExecuteMsg::CollectFees {
            position_id,
            transaction_deadline,
        } => {
            // Permitted during EmergencyPending so an LP about to remove
            // can sweep their share of fee_reserve before the drain
            //.
            check_pool_writable_for_remove(deps.storage)?;
            execute_collect_fees(deps, env, info, position_id, transaction_deadline)
        }
        ExecuteMsg::RemovePartialLiquidity {
            position_id,
            liquidity_to_remove,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            // Defense-in-depth: currently a drained pool has
            // total_liquidity == 0 so the math inside remove_partial_liquidity
            // would error out on its own. That's a coincidence, not a
            // guarantee. If a future partial-drain or recovery path ever
            // leaves non-zero total_liquidity after EMERGENCY_DRAINED is set,
            // an explicit check here keeps users from pulling against
            // already-swept reserves with arbitrary math.
            //
            // EmergencyPending is permitted so LPs can race the 24h drain
            //. Hard / auto-pause / drained still reject.
            check_pool_writable_for_remove(deps.storage)?;
            execute_remove_partial_liquidity(
                deps,
                env,
                info,
                position_id,
                liquidity_to_remove,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
        ExecuteMsg::RemoveAllLiquidity {
            position_id,
            transaction_deadline,
            min_amount1,
            min_amount0,
            max_ratio_deviation_bps,
        } => {
            check_pool_writable_for_remove(deps.storage)?;
            execute_remove_all_liquidity(
                deps,
                env,
                info,
                position_id,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
        ExecuteMsg::RemovePartialLiquidityByPercent {
            position_id,
            percentage,
            transaction_deadline,
            min_amount0,
            min_amount1,
            max_ratio_deviation_bps,
        } => {
            check_pool_writable_for_remove(deps.storage)?;
            execute_remove_partial_liquidity_by_percent(
                deps,
                env,
                info,
                position_id,
                percentage,
                transaction_deadline,
                min_amount0,
                min_amount1,
                max_ratio_deviation_bps,
            )
        }
        ExecuteMsg::ClaimCreatorExcessLiquidity { transaction_deadline } => {
            // Creator excess exists only when a commit-pool threshold is crossed
            // with more raised-bluechip than `max_bluechip_lock_per_pool`
            // absorbs. Standard pools have no commit phase and no excess
            // position, so there's nothing to claim.
            check_pool_writable(deps.storage)?;
            execute_claim_creator_excess(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::ClaimCreatorFees { transaction_deadline } => {
            // The creator fee pot is seeded by the fee_size_multiplier
            // clip on commit-pool LP fees. Standard pools have no creator
            // concept, so the pot is always empty and this handler is N/A.
            check_pool_writable(deps.storage)?;
            execute_claim_creator_fees(deps, env, info, transaction_deadline)
        }
        ExecuteMsg::RetryFactoryNotify {} => {
            // Retries NotifyThresholdCrossed to the factory. Standard
            // pools never cross a threshold, so there's nothing to retry.
            execute_retry_factory_notify(deps, env, info)
        }
        // distribution-liveness primitives.
        ExecuteMsg::SelfRecoverDistribution {} => {
            // Permissionless 7-day distribution-stall recovery. The
            // handler enforces the elapsed-time gate and rejects calls
            // before the window expires.
            execute_self_recover_distribution(deps, env, info)
        }
        ExecuteMsg::ClaimFailedDistribution { recipient } => {
            // Committer-side claim of an isolated mint failure recorded
            // in FAILED_MINTS. Caller must be the original committer
            // address; an optional alternate `recipient` lets them
            // route the mint to a fresh wallet.
            execute_claim_failed_distribution(deps, env, info, recipient)
        }
        // per-position post-emergency-drain claim
        // escrow. Auth gate (CW721 ownership of position_id) lives in
        // the handler.
        ExecuteMsg::ClaimEmergencyShare { position_id } => {
            execute_claim_emergency_share(deps, env, info, position_id)
        }
        // factory-only post-1y-dormancy sweep of
        // the unclaimed residual.
        ExecuteMsg::SweepUnclaimedEmergencyShares {} => {
            execute_sweep_unclaimed_emergency_shares(deps, env, info)
        }
        ExecuteMsg::AcceptNftOwnership {} => execute_accept_nft_ownership(deps, info),
    }
}

/// Creator-pool wrapper around pool-core's
/// `execute_update_config_from_factory`.
///
/// Pool-core's shared handler updates `PoolSpecs` (lp_fee +
/// min_commit_interval) but has no compile-time access to creator-pool
/// state and so leaves `update.min_commit_usd_pre_threshold` and
/// `update.min_commit_usd_post_threshold` untouched. This wrapper
/// applies those two creator-pool-only floors to `COMMIT_LIMIT_INFO`
/// first, then delegates the shared knobs to the inner handler.
///
/// Bounds re-enforced here (defense-in-depth — factory's
/// `PoolConfigUpdate::validate()` already rejects out-of-range values
/// at propose time, but locking the apply path too means a future
/// migration that ever inserts a `PendingPoolConfig` directly cannot
/// land an out-of-range value):
/// - non-zero
/// - <= `MAX_MIN_COMMIT_USD` ($1000, 6 decimals)
fn execute_update_creator_config_from_factory(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    update: crate::msg::PoolConfigUpdate,
) -> Result<Response, ContractError> {
    use crate::state::MAX_MIN_COMMIT_USD;

    // Auth gate is duplicated from pool-core's handler so we don't load
    // and write COMMIT_LIMIT_INFO under an unauthorised caller. The
    // inner call enforces it again (defense-in-depth).
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let pre = update.min_commit_usd_pre_threshold;
    let post = update.min_commit_usd_post_threshold;

    if pre.is_some() || post.is_some() {
        let mut commit_config = COMMIT_LIMIT_INFO.load(deps.storage)?;
        if let Some(v) = pre {
            if v.is_zero() || v > MAX_MIN_COMMIT_USD {
                return Err(ContractError::InvalidCommitFloor {
                    field: "min_commit_usd_pre_threshold",
                    got: v,
                    max: MAX_MIN_COMMIT_USD,
                });
            }
            commit_config.min_commit_usd_pre_threshold = v;
        }
        if let Some(v) = post {
            if v.is_zero() || v > MAX_MIN_COMMIT_USD {
                return Err(ContractError::InvalidCommitFloor {
                    field: "min_commit_usd_post_threshold",
                    got: v,
                    max: MAX_MIN_COMMIT_USD,
                });
            }
            commit_config.min_commit_usd_post_threshold = v;
        }
        COMMIT_LIMIT_INFO.save(deps.storage, &commit_config)?;
    }

    // Delegate shared knobs to the pool-core handler (which builds the
    // canonical response attributes).
    execute_update_config_from_factory(deps.branch(), env, info, update)
}

/// Factory-only callback dispatched immediately after `register_pool` in
/// `factory::finalize_pool`. Completes the two-phase CW721 ownership
/// handoff begun by the factory's `TransferOwnership` to the position
/// NFT: sends the matching `AcceptOwnership` back to the NFT and flips
/// `pool_state.nft_ownership_accepted`.
///
/// Mirrors `standard-pool`'s handler of the same name. Pre-this-handler
/// the commit pool relied on a lazy `AcceptOwnership` emitted by
/// `trigger_threshold_payout` (the first time threshold crossed), which
/// left the factory as the NFT contract's actual owner for the entire
/// pre-threshold window. The synchronous accept at finalize closes
/// that window.
///
/// Authorisation: `info.sender` must equal `pool_info.factory_addr`.
/// Idempotent: a second call (or a call after the deposit-side lazy
/// fallback in pool-core has already flipped the flag) returns Ok
/// without emitting a second `AcceptOwnership` — the CW721 would
/// reject the duplicate with `NoPendingOwner`, which would revert the
/// entire create tx if dispatched.
fn execute_accept_nft_ownership(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    if info.sender != pool_info.factory_addr {
        return Err(ContractError::Unauthorized {});
    }

    let mut pool_state = POOL_STATE.load(deps.storage)?;
    if pool_state.nft_ownership_accepted {
        return Ok(Response::new()
            .add_attribute("action", "accept_nft_ownership_noop")
            .add_attribute("pool_contract", pool_info.pool_info.contract_addr.to_string()));
    }

    let accept_msg = WasmMsg::Execute {
        contract_addr: pool_info.position_nft_address.to_string(),
        msg: to_json_binary(
            &pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::<()>::UpdateOwnership(
                pool_factory_interfaces::cw721_msgs::Action::AcceptOwnership,
            ),
        )?,
        funds: vec![],
    };
    pool_state.nft_ownership_accepted = true;
    POOL_STATE.save(deps.storage, &pool_state)?;

    Ok(Response::new()
        .add_message(CosmosMsg::Wasm(accept_msg))
        .add_attribute("action", "accept_nft_ownership")
        .add_attribute("pool_contract", pool_info.pool_info.contract_addr.to_string())
        .add_attribute("nft", pool_info.position_nft_address.to_string()))
}

/// Re-sends `NotifyThresholdCrossed` to the factory when the initial
/// notification (dispatched via `reply_on_error` during threshold-crossing
/// commit) failed. This entrypoint is callable by ANYONE; the factory's
/// POOL_THRESHOLD_MINTED idempotency check prevents a successful mint from
/// firing twice. Reply handling clears PENDING_FACTORY_NOTIFY on success.
///
/// Why permissionless: recovery-path tx. If a factory misconfiguration or
/// expand-economy stall caused the initial notification to fail, we want
/// anyone — a keeper, a committer, Bluechip ops — to be able to nudge the
/// system back to consistent state once the root cause is fixed. The worst
/// an abusive caller can do is waste their own gas on a factory reject.
pub fn execute_retry_factory_notify(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
) -> Result<Response, ContractError> {
    let pending = PENDING_FACTORY_NOTIFY
        .may_load(deps.storage)?
        .unwrap_or(false);
    if !pending {
        return Err(ContractError::NoPendingFactoryNotify);
    }

    let pool_info = POOL_INFO.load(deps.storage)?;
    let notify = SubMsg::reply_always(
        CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pool_info.factory_addr.to_string(),
            msg: to_json_binary(
                &pool_factory_interfaces::FactoryExecuteMsg::NotifyThresholdCrossed {
                    pool_id: pool_info.pool_id,
                },
            )?,
            funds: vec![],
        }),
        REPLY_ID_FACTORY_NOTIFY_RETRY,
    );

    Ok(Response::new()
        .add_submessage(notify)
        .add_attribute("action", "retry_factory_notify")
        .add_attribute("pool_id", pool_info.pool_id.to_string()))
}

/// SubMsg reply handler.
///
/// Two reply IDs come through here:
/// - REPLY_ID_FACTORY_NOTIFY_INITIAL (from the threshold-crossing commit).
/// Fires only on error (reply_on_error). We set PENDING_FACTORY_NOTIFY
/// and return Ok so the parent commit tx survives.
/// - REPLY_ID_FACTORY_NOTIFY_RETRY (from execute_retry_factory_notify).
/// Fires always (reply_always). On success we clear the pending flag;
/// on failure we keep it so another retry can be attempted.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, env: Env, msg: Reply) -> StdResult<Response> {
    match msg.id {
        REPLY_ID_FACTORY_NOTIFY_INITIAL => {
            let err = match msg.result {
                SubMsgResult::Err(e) => e,
                // reply_on_error shouldn't produce Ok here; treat as no-op.
                SubMsgResult::Ok(_) => return Ok(Response::new()),
            };
            PENDING_FACTORY_NOTIFY.save(deps.storage, &true)?;
            Ok(Response::new()
                .add_attribute("action", "factory_notify_deferred")
                .add_attribute("reason", err)
                .add_attribute("block_time", env.block.time.seconds().to_string()))
        }
        REPLY_ID_FACTORY_NOTIFY_RETRY => match msg.result {
            SubMsgResult::Ok(_) => {
                PENDING_FACTORY_NOTIFY.save(deps.storage, &false)?;
                Ok(Response::new()
                    .add_attribute("action", "factory_notify_retry_succeeded")
                    .add_attribute("block_time", env.block.time.seconds().to_string()))
            }
            SubMsgResult::Err(e) => {
                // Keep the pending flag set so another retry can be attempted
                // later. Don't revert — that would propagate the error back
                // to the caller and could trap gas in a retry loop.
                Ok(Response::new()
                    .add_attribute("action", "factory_notify_retry_failed")
                    .add_attribute("reason", e)
                    .add_attribute("block_time", env.block.time.seconds().to_string()))
            }
        },
        // route the deposit balance-verify reply through
        // the shared pool-core handler. Listed BEFORE the H6 distribution-
        // mint guard arm because `DEPOSIT_VERIFY_REPLY_ID` (0xD550_0000)
        // is numerically above `REPLY_ID_DISTRIBUTION_MINT_BASE` and the
        // guard arm would otherwise match it; literal arms before guard
        // arms in a Rust `match` win regardless of relative numeric
        // ordering, so this is the canonical placement.
        //
        // The verify handler returns `Result<Response, ContractError>`;
        // creator-pool's `reply` returns `StdResult<Response>` to stay
        // symmetric with the other branches above. The error is mapped
        // into `StdError::generic_err(<message>)` here — variant typing
        // is lost on this one path, but the failure message is verbose
        // enough for off-chain monitoring to match against the
        // "balance delta does not match" substring that
        // `handle_deposit_verify_reply` emits on a fee-on-transfer or
        // rebasing CW20.
        DEPOSIT_VERIFY_REPLY_ID => {
            handle_deposit_verify_reply(deps, env, msg)
                .map_err(|e| StdError::generic_err(e.to_string()))
        }
        id if id >= REPLY_ID_DISTRIBUTION_MINT_BASE
            && PENDING_MINT_REPLIES.has(deps.storage, id) =>
        {
            // Distribution-mint reply. The parent
            // `process_distribution_batch` (or `ClaimFailedDistribution`)
            // dispatched a per-user CW20 mint as a `reply_always` SubMsg
            // and stashed `(user, amount)` in PENDING_MINT_REPLIES under
            // this id. Two outcomes:
            //
            // - Mint succeeded: clear the stash and emit an attribute.
            // The CW20's bank-side state (recipient balance bumped)
            // stands.
            //
            // - Mint failed: clear the stash and accumulate the amount
            // under `user` in FAILED_MINTS. This is the load-bearing
            // liveness invariant: a single rejecting recipient no
            // longer reverts the entire batch tx; their amount is
            // held for `ClaimFailedDistribution` to retrieve later.
            // We always return Ok(...) from this branch — bubbling
            // the error would re-introduce the very stall this fix
            // was designed to eliminate.
            //
            // The dispatch arm is gated on `PENDING_MINT_REPLIES.has(id)`
            // so any id ≥ BASE without a stash entry falls through to the
            // canonical "unknown reply id" handler below (preserves the
            // pre-fix invariant and keeps the unknown-id regression test
            // valid).
            let pending = PENDING_MINT_REPLIES
                .load(deps.storage, msg.id)
                .map_err(|e| {
                    StdError::generic_err(format!(
                        "distribution-mint reply load failed for id {}: {}",
                        msg.id, e
                    ))
                })?;
            PENDING_MINT_REPLIES.remove(deps.storage, msg.id);

            match msg.result {
                SubMsgResult::Ok(_) => Ok(Response::new()
                    .add_attribute("action", "distribution_mint_succeeded")
                    .add_attribute("user", pending.user.to_string())
                    .add_attribute("amount", pending.amount.to_string())
                    .add_attribute("reply_id", msg.id.to_string())),
                SubMsgResult::Err(e) => {
                    // Saturate-safe accumulation under the user's
                    // canonical address. `Uint128::checked_add` returns
                    // OverflowError on the (effectively impossible)
                    // overflow path; bubble it as StdError so the reply
                    // does revert in that single edge case rather than
                    // silently dropping the failed amount.
                    FAILED_MINTS.update::<_, StdError>(
                        deps.storage,
                        &pending.user,
                        |existing| {
                            let prior = existing.unwrap_or_default();
                            prior
                                .checked_add(pending.amount)
                                .map_err(|o| StdError::generic_err(o.to_string()))
                        },
                    )?;
                    Ok(Response::new()
                        .add_attribute("action", "distribution_mint_isolated_failure")
                        .add_attribute("user", pending.user.to_string())
                        .add_attribute("amount", pending.amount.to_string())
                        .add_attribute("reply_id", msg.id.to_string())
                        .add_attribute("reason", e)
                        .add_attribute("block_time", env.block.time.seconds().to_string()))
                }
            }
        }
        other => Err(StdError::generic_err(
            pool_core::generic::unknown_reply_id_msg(
                pool_core::state::POOL_KIND_COMMIT,
                other,
            ),
        )),
    }
}


// ---------------------------------------------------------------------------
// Migrate
// ---------------------------------------------------------------------------

#[entry_point]
pub fn migrate(deps: DepsMut, env: Env, msg: MigrateMsg) -> Result<Response, ContractError> {
    // Reject downgrades. The chain has already replaced the wasm
    // bytecode by the time this handler runs, so this is the last
    // chance to abort a downgrade — a hard `Err` here causes the chain
    // to revert the migration and leave the pool on its previous code.
    //
    // Defensive layer in addition to the factory's 48h timelock on
    // ExecutePoolUpgrade. Once this check ships in every released
    // version, any future migration that would replace `version > N`
    // with `version <= N` is blocked at runtime regardless of admin
    // observability. Equal-version migrations are allowed (idempotent
    // re-runs); strictly-greater stored is rejected.
    //
    // `may_load`-style: a missing cw2 entry (legacy pre-cw2 pool, or a
    // test fixture that bypassed `instantiate`) skips the check rather
    // than erroring. Production pools always set cw2 via
    // `set_contract_version` at instantiate time.
    if let Ok(stored_version) = cw2::get_contract_version(deps.storage) {
        let stored_semver: semver::Version =
            stored_version
                .version
                .parse()
                .map_err(|e: semver::Error| ContractError::StoredVersionInvalid {
                    version: stored_version.version.clone(),
                    msg: e.to_string(),
                })?;
        let current_semver: semver::Version =
            CONTRACT_VERSION
                .parse()
                .map_err(|e: semver::Error| ContractError::CurrentVersionInvalid {
                    version: CONTRACT_VERSION.to_string(),
                    msg: e.to_string(),
                })?;
        if stored_semver > current_semver {
            return Err(ContractError::DowngradeRefused {
                stored: stored_semver.to_string(),
                current: current_semver.to_string(),
            });
        }
    }

    match msg {
        MigrateMsg::UpdateFees { new_fees } => {
            if new_fees > MAX_LP_FEE || new_fees < MIN_LP_FEE {
                return Err(ContractError::LpFeeOutOfRange {
                    got: new_fees,
                    min: MIN_LP_FEE,
                    max: MAX_LP_FEE,
                });
            }
            POOL_SPECS.update(deps.storage, |mut specs| -> StdResult<_> {
                specs.lp_fee = new_fees;
                Ok(specs)
            })?;
        }
        // No per-variant work — falls through to the unconditional
        // `set_contract_version` below so the cw2 stored version
        // always lands at the new release on a successful migrate.
        MigrateMsg::UpdateVersion {} => {}
    }

    // Reset the price accumulator on every migrate. Mirrors the equivalent
    // block in `standard-pool::contract::migrate`; the rationale (clean
    // unit-scale boundary across a `PRICE_ACCUMULATOR_SCALE` change in
    // `pool_core::swap::update_price_accumulator`) lives there. Costs at
    // most one factory oracle TWAP round per upgrade, which the breaker /
    // snapshot machinery handles cleanly.
    if let Ok(mut state) = POOL_STATE.load(deps.storage) {
        state.price0_cumulative_last = cosmwasm_std::Uint128::zero();
        state.price1_cumulative_last = cosmwasm_std::Uint128::zero();
        state.block_time_last = env.block.time.seconds();
        POOL_STATE.save(deps.storage, &state)?;
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("version", CONTRACT_VERSION))
}
