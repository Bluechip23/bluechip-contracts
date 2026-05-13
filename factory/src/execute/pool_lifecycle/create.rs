//! Pool creation entry points for both pool kinds, plus the input
//! validators that guard them.
//!
//! Commit pools and standard pools have separate create paths because
//! they differ in nearly every input dimension (standard pools wrap
//! pre-existing assets, commit pools mint a fresh CW20 at creation) —
//! but share the same reply-ID / register_pool plumbing downstream.

use cosmwasm_std::{
    to_json_binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response, StdError, SubMsg,
    Uint128, WasmMsg,
};
use cw20::MinterResponse;
use cw_utils::{must_pay, PaymentError};

use crate::error::ContractError;
use crate::msg::{CreatorTokenInfo, TokenInstantiateMsg};
use crate::pool_struct::{CreatePool, TempPoolCreation};
use crate::state::{
    canonical_pair_key, CreationStatus, COMMIT_POOL_COUNTER,
    COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS, FACTORYINSTANTIATEINFO, LAST_COMMIT_POOL_CREATE_AT,
    LAST_STANDARD_POOL_CREATE_AT, PAIRS, POOL_COUNTER, POOL_CREATION_CONTEXT,
    PoolCreationContext, PoolCreationState, STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS,
};

use super::super::{encode_reply_id, MINT_STANDARD_NFT, SET_TOKENS};

// Sentinel placeholder the caller must supply for the CreatorToken slot.
// The factory mints a fresh CW20 during pool creation and rewrites this
// entry to the real address in mint_create_pool. Any other value in the
// CreatorToken slot is rejected so attackers can't smuggle an arbitrary
// (possibly malicious) CW20 into the pool's asset_infos.
pub const CREATOR_TOKEN_SENTINEL: &str = "WILL_BE_CREATED_BY_FACTORY";

/// Validates the pair shape supplied by the commit-pool creator:
///   - exactly one Bluechip entry whose denom equals the factory's canonical
///     `bluechip_denom` (prevents attackers from registering pools under a
///     fake native denom they control via tokenfactory or similar)
///   - exactly one CreatorToken entry whose contract_addr equals the sentinel
///
/// Anything else (duplicate Bluechips with different denoms, two CreatorTokens,
/// a CreatorToken pointing at some pre-existing CW20, a Bluechip with a wrong
/// denom) is rejected up front so the downstream instantiate doesn't have to
/// untangle a malformed pair.
pub(crate) fn validate_pool_token_info(
    pool_token_info: &[crate::asset::TokenType; 2],
    canonical_bluechip_denom: &str,
) -> Result<(), ContractError> {
    use crate::asset::TokenType;

    // Strict ordering: bluechip MUST be at index 0, creator-token at
    // index 1. Every downstream piece of pool code (post_threshold_commit,
    // simple_swap, threshold_payout reserves) hard-codes the assumption
    // that `reserve0` is bluechip and `reserve1` is creator-token. The
    // factory's `mint_create_pool` rewrites the sentinel in place
    // preserving order, so a `[CreatorToken sentinel, Bluechip]` input
    // would propagate a reversed pair into the pool and silently produce
    // wrong-direction swaps. Enforcing order here keeps the assumption
    // load-bearing rather than incidental.
    match (&pool_token_info[0], &pool_token_info[1]) {
        (TokenType::Native { denom }, TokenType::CreatorToken { contract_addr }) => {
            if denom.trim().is_empty() {
                return Err(ContractError::Std(StdError::generic_err(
                    "Bluechip denom must be non-empty",
                )));
            }
            if denom != canonical_bluechip_denom {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Bluechip denom must match the factory canonical denom \"{}\"; got \"{}\"",
                    canonical_bluechip_denom, denom
                ))));
            }
            if contract_addr.as_str() != CREATOR_TOKEN_SENTINEL {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "CreatorToken contract_addr must be the sentinel \"{}\"; got \"{}\". The factory mints the CW20 itself and rewrites this field.",
                    CREATOR_TOKEN_SENTINEL, contract_addr
                ))));
            }
            Ok(())
        }
        _ => Err(ContractError::Std(StdError::generic_err(
            "pool_token_info must be [Bluechip(canonical denom), CreatorToken(sentinel)] — \
             order matters: bluechip at index 0, creator-token at index 1.",
        ))),
    }
}

/// Validates creator token metadata before any state is written.
/// - decimals must be 6 (threshold payout and mint cap are calibrated for 6-decimal tokens)
/// - name: 3-50 chars, printable ASCII only (no control chars, no extended unicode)
/// - symbol: 3-12 chars, uppercase ASCII letters and digits only (matches cw20-base spec)
pub(crate) fn validate_creator_token_info(
    token_info: &CreatorTokenInfo,
) -> Result<(), ContractError> {
    if token_info.decimal != 6 {
        return Err(ContractError::Std(StdError::generic_err(
            "Token decimals must be 6. Threshold payout amounts and mint caps are calibrated for 6-decimal tokens.",
        )));
    }

    let name_len = token_info.name.chars().count();
    if !(3..=50).contains(&name_len) {
        return Err(ContractError::Std(StdError::generic_err(
            "Token name must be between 3 and 50 characters",
        )));
    }
    if !token_info
        .name
        .chars()
        .all(|c| c.is_ascii() && !c.is_ascii_control())
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Token name must contain only printable ASCII characters",
        )));
    }

    let symbol_len = token_info.symbol.chars().count();
    if !(3..=12).contains(&symbol_len) {
        return Err(ContractError::Std(StdError::generic_err(
            "Token symbol must be between 3 and 12 characters",
        )));
    }
    if !token_info
        .symbol
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Token symbol must contain only uppercase ASCII letters (A-Z) and digits (0-9)",
        )));
    }
    // Require at least one letter. Pure-digit symbols ("123", "001")
    // pass the character-class check above but render as malformed in
    // most CW20 frontends and confuse human readers (looks like a token
    // ID, not a ticker). Mainline tickers are letters + optional digits;
    // gating on ≥1 letter rules out the cosmetic-bug shape without
    // restricting legitimate naming.
    if !token_info.symbol.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(ContractError::Std(StdError::generic_err(
            "Token symbol must contain at least one uppercase ASCII letter (A-Z); \
             all-digit symbols are not allowed",
        )));
    }

    Ok(())
}

pub(crate) fn execute_create_creator_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_msg: CreatePool,
    token_info: CreatorTokenInfo,
) -> Result<Response, ContractError> {
    // Validate token metadata and pair shape up front, before any state
    // writes. These checks must stay at the top of the handler — they
    // guard every later step of pool creation.
    validate_creator_token_info(&token_info)?;
    let factory_cw20 = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    validate_pool_token_info(&pool_msg.pool_token_info, &factory_cw20.bluechip_denom)?;

    // Per-address rate limit. Reject if `info.sender` already
    // created a commit pool within the last
    // COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS. Stamps the new timestamp
    // before any SubMsg dispatch, so a failed reply chain (which
    // reverts the whole tx atomically) also reverts the stamp —
    // no permanent rate-limit state leaks from failed creates.
    //
    // Runs BEFORE the fee oracle / funds check so a rate-limited
    // caller sees the rate-limit error directly rather than a
    // misleading "insufficient fee" error (when the actual block
    // is the cooldown, not the fee).
    let now = env.block.time;
    let prior_stamp =
        LAST_COMMIT_POOL_CREATE_AT.may_load(deps.storage, info.sender.clone())?;
    if let Some(last) = prior_stamp {
        let next_allowed = last.plus_seconds(COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS);
        if now < next_allowed {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Rate-limited: this address can create another commit pool after {} \
                 (last create at {}, cooldown {}s)",
                next_allowed, last, COMMIT_POOL_CREATE_RATE_LIMIT_SECONDS
            ))));
        }
    }
    LAST_COMMIT_POOL_CREATE_AT.save(deps.storage, info.sender.clone(), &now)?;
    // Sync the timestamp-ordered secondary index used by PruneRateLimits.
    // Remove the prior (old_ts, addr) entry first so the index stays
    // single-entry-per-address; the index is keyed by timestamp so an
    // un-removed prior would persist as a stale ghost.
    if let Some(prior) = prior_stamp {
        crate::state::COMMIT_POOL_CREATE_TS_INDEX
            .remove(deps.storage, (prior.seconds(), info.sender.clone()));
    }
    crate::state::COMMIT_POOL_CREATE_TS_INDEX.save(
        deps.storage,
        (now.seconds(), info.sender.clone()),
        &(),
    )?;

    // Charge a USD-denominated creation fee (paid in canonical bluechip)
    // for commit-pool creation as anti-spam friction. Reuses the same
    // fee knob as standard pools so deployments can enable/disable from
    // a single config value.
    //
    // Fallback policy (HIGH-3 audit fix):
    //   - oracle returns a non-zero amount (steady state OR best-effort
    //     warm-up backed by `pre_reset_last_price`): use the conversion.
    //   - oracle unavailable AND `INITIAL_ANCHOR_SET == false`: this is
    //     the true bootstrap window before the anchor pool exists, so
    //     fall back to the hardcoded `STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP`.
    //     Reachable for at most the first standard-pool creation (which
    //     becomes the anchor) — bounded one-shot.
    //   - oracle unavailable AND `INITIAL_ANCHOR_SET == true`: refuse
    //     creation with `OracleUnavailable`. Without this gate, an
    //     attacker who waited for an oracle outage could pay the flat
    //     hardcoded amount regardless of the bluechip USD price (could
    //     be 100× too cheap if bluechip moons, or 100× too expensive
    //     if it crashes). Refusing converts an attack window into a
    //     temporary creation freeze, which is safer than mispricing.
    let usd_fee = factory_cw20.standard_pool_creation_fee_usd;
    let (required_bluechip, fee_source) = if usd_fee.is_zero() {
        (Uint128::zero(), "disabled")
    } else {
        match crate::internal_bluechip_price_oracle::usd_to_bluechip_best_effort(
            deps.as_ref(),
            usd_fee,
            &env,
        ) {
            Ok(conv) if !conv.amount.is_zero() => (conv.amount, "oracle"),
            _ => {
                let initial_anchor_set = crate::state::INITIAL_ANCHOR_SET
                    .may_load(deps.storage)?
                    .unwrap_or(false);
                if !initial_anchor_set {
                    (
                        crate::state::STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP,
                        "fallback_bootstrap",
                    )
                } else {
                    return Err(ContractError::Std(StdError::generic_err(
                        "Cannot price creation fee: oracle unavailable AND no recent \
                         bluechip USD estimate. Hardcoded fallback is only permitted \
                         during the pre-anchor bootstrap window. Wait for the oracle \
                         to recover (next UpdateOraclePrice succeeds), or — if this is \
                         a sustained outage — investigate the Pyth feed and anchor \
                         pool before retrying.",
                    )));
                }
            }
        }
    };
    // Strict single-denom funds validation (audit hardening: replace the
    // prior best-effort `.find()` + refund-extras pattern with `must_pay`).
    // `must_pay` enforces that `info.funds` contains exactly one Coin
    // entry whose denom equals `bluechip_denom` and whose amount is
    // non-zero; any other shape (multi-denom, wrong denom, empty, zero
    // amount) errors out and the tx reverts. On revert the bank module
    // auto-returns all attached funds to the caller — no in-tx refund
    // path required for non-bluechip denoms, which closes the
    // "extra-funds-attached" griefing vector.
    //
    // Two-mode behavior keyed off the live fee:
    //   - Fee enabled (`required_bluechip > 0`): use `must_pay`. Surplus
    //     over `required_bluechip` is refunded in the same tx, since
    //     callers can't predict the exact oracle-converted amount
    //     between quoting and submission.
    //   - Fee disabled (`required_bluechip == 0`): no funds are expected
    //     and none are accepted. Any attached funds (even bluechip)
    //     error out — callers who paid by mistake get everything back on
    //     revert. This is intentional: a disabled fee shouldn't quietly
    //     accept then refund payments, because that masks frontend bugs.
    let paid_bluechip = if required_bluechip.is_zero() {
        if !info.funds.is_empty() {
            return Err(ContractError::Std(StdError::generic_err(
                "Commit-pool creation fee is disabled; do not attach any funds.",
            )));
        }
        Uint128::zero()
    } else {
        match must_pay(&info, &factory_cw20.bluechip_denom) {
            Ok(amount) => amount,
            Err(PaymentError::NoFunds {}) | Err(PaymentError::MissingDenom(_)) => {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Insufficient commit-pool creation fee: required {} {}, paid 0 {}",
                    required_bluechip, factory_cw20.bluechip_denom, factory_cw20.bluechip_denom
                ))));
            }
            Err(e) => {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Invalid commit-pool creation funds: {}. Send exactly one denom ({}).",
                    e, factory_cw20.bluechip_denom
                ))));
            }
        }
    };
    if paid_bluechip < required_bluechip {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Insufficient commit-pool creation fee: required {} {}, paid {} {}",
            required_bluechip, factory_cw20.bluechip_denom, paid_bluechip, factory_cw20.bluechip_denom
        ))));
    }
    let surplus = paid_bluechip.checked_sub(required_bluechip)?;
    let mut fee_messages: Vec<CosmosMsg> = Vec::new();
    if !required_bluechip.is_zero() {
        fee_messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: factory_cw20.bluechip_wallet_address.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: factory_cw20.bluechip_denom.clone(),
                amount: required_bluechip,
            }],
        }));
    }
    if !surplus.is_zero() {
        fee_messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: factory_cw20.bluechip_denom.clone(),
                amount: surplus,
            }],
        }));
    }

    let creator_attr = info.sender.to_string();
    let pool_counter = POOL_COUNTER.may_load(deps.storage)?.unwrap_or(0);
    let pool_id = pool_counter + 1;
    POOL_COUNTER.save(deps.storage, &pool_id)?;
    // Allocate the commit-pool-only ordinal. Bumped only here, never in
    // `execute_create_standard_pool`, so the bluechip mint-decay formula
    // sees a count of legitimate commit-pool creations rather than a
    // count that permissionless standard-pool creation can inflate.
    let commit_pool_counter = COMMIT_POOL_COUNTER.may_load(deps.storage)?.unwrap_or(0);
    let commit_pool_ordinal = commit_pool_counter + 1;
    COMMIT_POOL_COUNTER.save(deps.storage, &commit_pool_ordinal)?;

    let msg = WasmMsg::Instantiate {
        code_id: factory_cw20.cw20_token_contract_id,
        //creating the creator token only, no minting.
        msg: to_json_binary(&TokenInstantiateMsg {
            name: token_info.name.clone(),
            symbol: token_info.symbol,
            decimals: token_info.decimal,
            initial_balances: vec![],
            mint: Some(MinterResponse {
                minter: env.contract.address.to_string(),
                // Mint cap pinned to the exact threshold-payout total
                // derived from `factory_cw20.threshold_payout_amounts`
                // (default: creator 325e9 + bluechip 25e9 + pool_seed
                // 350e9 + commit_return 500e9 = 1.2e12). No protocol
                // path ever needs to mint beyond this — the payout is
                // fixed at threshold-cross and validated by
                // `ThresholdPayoutAmounts::validate` (propose-time)
                // and `validate_pool_threshold_payments` (runtime).
                // If any future code path ever gained mint authority
                // and tried to mint extra tokens, cw20-base would
                // reject the mint and revert the entire tx
                // (fail-closed) rather than silently letting
                // additional supply be created.
                cap: Some(factory_cw20.threshold_payout_amounts.total_mint()?),
            }),
        })?,
        //no initial balance. waits until threshold is crossed to mint creator tokens.
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: token_info.name,
    };
    POOL_CREATION_CONTEXT.save(
        deps.storage,
        pool_id,
        &PoolCreationContext {
            temp: TempPoolCreation {
                temp_pool_info: pool_msg,
                temp_creator_wallet: info.sender.clone(),
                pool_id,
                creator_token_addr: None,
                nft_addr: None,
            },
            state: PoolCreationState {
                pool_id,
                creator: info.sender,
                creation_time: env.block.time,
                status: CreationStatus::Started,
            },
            commit_pool_ordinal,
        },
    )?;
    let sub_msg = vec![SubMsg::reply_on_success(
        msg,
        encode_reply_id(pool_id, SET_TOKENS),
    )];

    Ok(Response::new()
        .add_messages(fee_messages)
        .add_attribute("action", "create")
        .add_attribute("creator", creator_attr)
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("required_fee_bluechip", required_bluechip.to_string())
        .add_attribute("paid_fee_bluechip", paid_bluechip.to_string())
        .add_attribute("refunded_bluechip", surplus.to_string())
        .add_attribute("fee_source", fee_source)
        .add_submessages(sub_msg))
}

/// Validates a `[TokenType; 2]` pair supplied to `CreateStandardPool`.
///
/// Rules (looser than the commit-pool validator at `validate_pool_token_info`
/// because standard pools can hold canonical-bluechip/native, canonical-
/// bluechip/CW20, or mixed-native pairs as long as the canonical bluechip is
/// present on one side):
///   - No self-pair: the two entries must differ. Same denom on both sides
///     (`Bluechip("uatom")` + `Bluechip("uatom")`) or same address on both
///     sides (`CreatorToken("cosmos1...")` ×2) is rejected.
///   - `Bluechip { denom }`: each native denom must be non-empty.
///   - Canonical inclusion: at least one leg must equal the factory's
///     canonical `bluechip_denom`. This keeps standard pools anchored to
///     protocol bluechip liquidity while still allowing a second native
///     denom (e.g. ATOM) or a CW20 leg.
///   - `CreatorToken { contract_addr }`: address must bech32-validate, AND
///     the address must answer a `Cw20QueryMsg::TokenInfo {}` query (so we
///     reject typos and non-CW20 contracts at creation rather than at first
///     deposit).
fn validate_standard_pool_token_info(
    deps: Deps,
    canonical_bluechip_denom: &str,
    pair: &[crate::asset::TokenType; 2],
) -> Result<(), ContractError> {
    use crate::asset::TokenType;

    // Self-pair check.
    match (&pair[0], &pair[1]) {
        (TokenType::Native { denom: a }, TokenType::Native { denom: b }) if a == b => {
            return Err(ContractError::Std(StdError::generic_err(
                "Standard pool pair cannot use the same Bluechip denom on both sides",
            )));
        }
        (
            TokenType::CreatorToken { contract_addr: a },
            TokenType::CreatorToken { contract_addr: b },
        ) if a == b => {
            return Err(ContractError::Std(StdError::generic_err(
                "Standard pool pair cannot use the same CreatorToken on both sides",
            )));
        }
        _ => {}
    }

    let mut has_canonical_bluechip = false;
    for entry in pair.iter() {
        match entry {
            TokenType::Native { denom } => {
                if denom.trim().is_empty() {
                    return Err(ContractError::Std(StdError::generic_err(
                        "Standard pool: Bluechip denom must be non-empty",
                    )));
                }
                if denom == canonical_bluechip_denom {
                    has_canonical_bluechip = true;
                }
            }
            TokenType::CreatorToken { contract_addr } => {
                deps.api.addr_validate(contract_addr.as_str()).map_err(|e| {
                    ContractError::Std(StdError::generic_err(format!(
                        "Standard pool: invalid CreatorToken address {}: {}",
                        contract_addr, e
                    )))
                })?;
                // Verify the address actually responds to a CW20 TokenInfo
                // query. Catches typos pointing at random contracts and
                // pre-instantiate addresses. The query is cheap and the
                // response is discarded — we only care whether it succeeds.
                let _info: cw20::TokenInfoResponse = deps
                    .querier
                    .query_wasm_smart(
                        contract_addr.to_string(),
                        &cw20::Cw20QueryMsg::TokenInfo {},
                    )
                    .map_err(|e| {
                        ContractError::Std(StdError::generic_err(format!(
                            "Standard pool: CreatorToken {} did not respond to TokenInfo query (not a CW20?): {}",
                            contract_addr, e
                        )))
                    })?;
            }
        }
    }
    if !has_canonical_bluechip {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Standard pool must include canonical bluechip denom \"{}\" on at least one side",
            canonical_bluechip_denom
        ))));
    }

    Ok(())
}

/// Permissionless entry point for creating a plain xyk pool around two
/// pre-existing assets. Caller pays a USD-denominated fee (in ubluechip)
/// configured on the factory; the fee is forwarded to
/// `bluechip_wallet_address`. The pool is NOT eligible for oracle sampling
/// and has no commit phase or distribution.
///
/// Reply chain (2 steps, vs the commit-pool chain's 3): NFT instantiate
/// → pool instantiate → register & transfer NFT ownership. CW20 minting
/// is skipped entirely (standard pools wrap existing tokens).
pub(crate) fn execute_create_standard_pool(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    pool_token_info: [crate::asset::TokenType; 2],
    label: String,
) -> Result<Response, ContractError> {
    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    // Pair-shape validation runs first so bad input fails before we charge
    // the fee or write any state.
    validate_standard_pool_token_info(
        deps.as_ref(),
        &factory_config.bluechip_denom,
        &pool_token_info,
    )?;

    // Pair-uniqueness pre-check (single-pool-per-pair invariant). The
    // canonical guard lives inside `register_pool` and would catch this
    // duplicate at finalize time too, but doing the check here lets us
    // reject before charging the creation fee — without this, a duplicate
    // attempt would forward `required_bluechip` to the bluechip wallet,
    // run the NFT instantiate, and only fail in the finalize reply, which
    // (because the reply chain is `reply_on_success`) reverts the whole
    // tx and the fee is recovered atomically — but a frontend caller
    // sees a much later, less actionable error than what they get here.
    //
    // Doing this check AFTER `validate_standard_pool_token_info` means
    // we only canonicalize already-shape-validated pairs, so the key
    // function never sees malformed input.
    let pair_key = canonical_pair_key(&pool_token_info);
    if let Some(existing) = PAIRS.may_load(deps.storage, pair_key.clone())? {
        return Err(ContractError::DuplicatePair {
            existing_pool_id: existing,
            asset_a: pair_key.0,
            asset_b: pair_key.1,
        });
    }

    // Per-address rate limit on standard-pool creation (audit fix). Mirror
    // of the commit-pool rate-limit at `execute_create_creator_pool`.
    // Stamps the new timestamp before any further state writes — a
    // failed downstream step (insufficient funds, fee-forward revert,
    // reply chain failure) reverts this stamp atomically along with the
    // rest of the tx, so no permanent rate-limit residue from rejected
    // creations.
    let now = env.block.time;
    let prior_std_stamp =
        LAST_STANDARD_POOL_CREATE_AT.may_load(deps.storage, info.sender.clone())?;
    if let Some(last) = prior_std_stamp {
        let next_allowed = last.plus_seconds(STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS);
        if now < next_allowed {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Rate-limited: this address can create another standard pool after {} \
                 (last create at {}, cooldown {}s)",
                next_allowed, last, STANDARD_POOL_CREATE_RATE_LIMIT_SECONDS
            ))));
        }
    }
    LAST_STANDARD_POOL_CREATE_AT.save(deps.storage, info.sender.clone(), &now)?;
    // Sync the timestamp-ordered secondary index for prune. See the
    // commit-pool variant above for the rationale.
    if let Some(prior) = prior_std_stamp {
        crate::state::STANDARD_POOL_CREATE_TS_INDEX
            .remove(deps.storage, (prior.seconds(), info.sender.clone()));
    }
    crate::state::STANDARD_POOL_CREATE_TS_INDEX.save(
        deps.storage,
        (now.seconds(), info.sender.clone()),
        &(),
    )?;

    if label.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "label must be non-empty",
        )));
    }
    // Bound label length up front. The label is propagated to the
    // pool wasm's instantiate `label` field and emitted as an attribute;
    // both have SDK-level limits (512 bytes typical) but failing there
    // surfaces deep in the reply chain rather than at message ingress.
    // 128 chars is plenty for human-readable identifiers and well clear
    // of any chain-side cap.
    const MAX_LABEL_LEN: usize = 128;
    if label.chars().count() > MAX_LABEL_LEN {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "label too long: {} chars (max {})",
            label.chars().count(),
            MAX_LABEL_LEN
        ))));
    }

    // Compute required fee. See the per-branch policy doc on the
    // `match` below for the full fallback logic.
    let usd_fee = factory_config.standard_pool_creation_fee_usd;
    let (required_bluechip, fee_source) = if usd_fee.is_zero() {
        (Uint128::zero(), "disabled")
    } else {
        // Best-effort USD→bluechip conversion. Falls back to
        // `pre_reset_last_price` during the post-reset warm-up window
        // instead of erroring; keeps standard-pool creation functional
        // through anchor rotations rather than forcing every standard-
        // pool creator to wait ~30 min after every rotation.
        //
        // Hardcoded-fallback policy (HIGH-3 audit fix): if even
        // best-effort returns nothing, only fall back when this is the
        // true pre-anchor bootstrap window (`INITIAL_ANCHOR_SET == false`,
        // meaning no anchor pool exists yet — this is the very first
        // standard-pool creation that becomes the anchor). After the
        // anchor is set, an oracle outage MUST refuse creation rather
        // than charging a flat amount untied to the live USD value, to
        // prevent attackers from timing creations during outages to
        // bypass the configured USD fee.
        match crate::internal_bluechip_price_oracle::usd_to_bluechip_best_effort(
            deps.as_ref(),
            usd_fee,
            &env,
        ) {
            Ok(conv) if !conv.amount.is_zero() => (conv.amount, "oracle"),
            _ => {
                let initial_anchor_set = crate::state::INITIAL_ANCHOR_SET
                    .may_load(deps.storage)?
                    .unwrap_or(false);
                if !initial_anchor_set {
                    (
                        crate::state::STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP,
                        "fallback_bootstrap",
                    )
                } else {
                    return Err(ContractError::Std(StdError::generic_err(
                        "Cannot price creation fee: oracle unavailable AND no recent \
                         bluechip USD estimate. Hardcoded fallback is only permitted \
                         during the pre-anchor bootstrap window. Wait for the oracle \
                         to recover (next UpdateOraclePrice succeeds) before retrying.",
                    )));
                }
            }
        }
    };

    // Strict single-denom funds validation (audit hardening: replace the
    // prior best-effort `.find()` + refund-extras pattern with `must_pay`).
    // See the equivalent block in `execute_create_commit_pool` for the
    // full rationale. Summary:
    //   - Fee enabled: `must_pay` requires exactly one Coin entry of
    //     `bluechip_denom`, non-zero. Any other shape errors and reverts;
    //     bank-module revert auto-returns the caller's funds.
    //   - Fee disabled: no funds expected, none accepted.
    let paid_bluechip = if required_bluechip.is_zero() {
        if !info.funds.is_empty() {
            return Err(ContractError::Std(StdError::generic_err(
                "Standard-pool creation fee is disabled; do not attach any funds.",
            )));
        }
        Uint128::zero()
    } else {
        match must_pay(&info, &factory_config.bluechip_denom) {
            Ok(amount) => amount,
            Err(PaymentError::NoFunds {}) | Err(PaymentError::MissingDenom(_)) => {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Insufficient creation fee: required {} {}, paid 0 {}",
                    required_bluechip, factory_config.bluechip_denom, factory_config.bluechip_denom
                ))));
            }
            Err(e) => {
                return Err(ContractError::Std(StdError::generic_err(format!(
                    "Invalid creation funds: {}. Send exactly one denom ({}).",
                    e, factory_config.bluechip_denom
                ))));
            }
        }
    };
    if paid_bluechip < required_bluechip {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Insufficient creation fee: required {} {}, paid {} {}",
            required_bluechip, factory_config.bluechip_denom, paid_bluechip, factory_config.bluechip_denom
        ))));
    }

    // Bump pool_id BEFORE touching state so a failed fee-forward tx
    // doesn't burn a counter slot on revert; it does mean an in-flight
    // parallel commit pool and standard pool can't both reserve the
    // same id, which is intentional (they share POOLS_BY_ID).
    let pool_counter = POOL_COUNTER.may_load(deps.storage)?.unwrap_or(0);
    let pool_id = pool_counter + 1;
    POOL_COUNTER.save(deps.storage, &pool_id)?;

    // Forward exactly `required_bluechip` to the bluechip wallet and
    // refund any surplus (`paid - required`) to the caller. Both sends
    // ride in the same Response as the NFT-instantiate SubMsg, so they
    // dispatch atomically with pool creation: if any reply step fails
    // downstream the whole tx reverts and the caller keeps `paid`
    // intact.
    //
    // Disabled-fee case (`required_bluechip == 0`): the entire `paid`
    // amount is returned to the caller; nothing reaches the wallet. A
    // caller who attached zero funds gets neither send (no-op).
    //
    // Partial-move `bluechip_denom` out of factory_config since it has
    // no further reads after this point; remaining fields
    // (`cw721_nft_contract_id`, `bluechip_wallet_address`) are still
    // accessible because partial moves don't invalidate the rest of
    // the struct.
    let surplus = paid_bluechip.checked_sub(required_bluechip)?;
    let bluechip_denom = factory_config.bluechip_denom;
    let mut messages: Vec<CosmosMsg> = Vec::new();
    if !required_bluechip.is_zero() {
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: factory_config.bluechip_wallet_address.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: bluechip_denom.clone(),
                amount: required_bluechip,
            }],
        }));
    }
    if !surplus.is_zero() {
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: bluechip_denom.clone(),
                amount: surplus,
            }],
        }));
    }

    crate::state::STANDARD_POOL_CREATION_CONTEXT.save(
        deps.storage,
        pool_id,
        &crate::state::StandardPoolCreationContext {
            pool_id,
            pool_token_info,
            creator: info.sender.clone(),
            label: label.clone(),
            nft_addr: None,
        },
    )?;

    let nft_msg = WasmMsg::Instantiate {
        code_id: factory_config.cw721_nft_contract_id,
        msg: to_json_binary(&pool_factory_interfaces::cw721_msgs::Cw721InstantiateMsg {
            name: format!("Standard Pool {} LP", pool_id),
            symbol: "AMM-LP".to_string(),
            minter: env.contract.address.to_string(),
        })?,
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: format!("AMM-LP-NFT-Standard-{}", pool_id),
    };
    let sub_msg = SubMsg::reply_on_success(nft_msg, encode_reply_id(pool_id, MINT_STANDARD_NFT));

    Ok(Response::new()
        .add_messages(messages)
        .add_submessage(sub_msg)
        .add_attribute("action", "create_standard_pool")
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("creator", info.sender.to_string())
        .add_attribute("required_fee_bluechip", required_bluechip.to_string())
        .add_attribute("paid_fee_bluechip", paid_bluechip.to_string())
        .add_attribute("refunded_bluechip", surplus.to_string())
        .add_attribute("fee_source", fee_source)
        .add_attribute("label", label))
}
