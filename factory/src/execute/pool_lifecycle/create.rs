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

use crate::error::ContractError;
use crate::msg::{CreatorTokenInfo, TokenInstantiateMsg};
use crate::pool_struct::{CreatePool, TempPoolCreation};
use crate::state::{
    CreationStatus, FACTORYINSTANTIATEINFO, POOL_COUNTER, POOL_CREATION_CONTEXT,
    PoolCreationContext, PoolCreationState,
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

    let mut bluechip_count = 0usize;
    let mut creator_count = 0usize;
    for t in pool_token_info.iter() {
        match t {
            TokenType::Native { denom } => {
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
                bluechip_count += 1;
            }
            TokenType::CreatorToken { contract_addr } => {
                if contract_addr.as_str() != CREATOR_TOKEN_SENTINEL {
                    return Err(ContractError::Std(StdError::generic_err(format!(
                        "CreatorToken contract_addr must be the sentinel \"{}\"; got \"{}\". The factory mints the CW20 itself and rewrites this field.",
                        CREATOR_TOKEN_SENTINEL, contract_addr
                    ))));
                }
                creator_count += 1;
            }
        }
    }
    if bluechip_count != 1 || creator_count != 1 {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "pool_token_info must contain exactly one Bluechip and one CreatorToken (got {} Bluechip, {} CreatorToken)",
            bluechip_count, creator_count
        ))));
    }
    Ok(())
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

    // Standard pools now go through `ExecuteMsg::CreateStandardPool`
    // (dispatched to the new standard-pool wasm via the
    // `standard_pool_wasm_contract_id` code_id added in 4c). The old
    // `is_standard_pool: Some(true)` bypass on `CreatePool` is gone.

    let sender = info.sender.clone();
    let pool_counter = POOL_COUNTER.may_load(deps.storage)?.unwrap_or(0);
    let pool_id = pool_counter + 1;
    POOL_COUNTER.save(deps.storage, &pool_id)?;
    let msg = WasmMsg::Instantiate {
        code_id: factory_cw20.cw20_token_contract_id,
        //creating the creator token only, no minting.
        msg: to_json_binary(&TokenInstantiateMsg {
            name: token_info.name.clone(),
            symbol: token_info.symbol.clone(),
            decimals: token_info.decimal,
            initial_balances: vec![],
            mint: Some(MinterResponse {
                minter: env.contract.address.to_string(),
                //amount minted after threshold.
                cap: Some(Uint128::new(1_500_000_000_000)),
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
                creator_token_address: None,
                mint_new_position_nft_address: None,
                pool_address: None,
                creation_time: env.block.time,
                status: CreationStatus::Started,
            },
        },
    )?;
    let sub_msg = vec![SubMsg::reply_on_success(
        msg,
        encode_reply_id(pool_id, SET_TOKENS),
    )];

    Ok(Response::new()
        .add_attribute("action", "create")
        .add_attribute("creator", sender.to_string())
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessages(sub_msg))
}

/// Validates a `[TokenType; 2]` pair supplied to `CreateStandardPool`.
///
/// Rules (looser than the commit-pool validator at `validate_pool_token_info`
/// because standard pools can hold ANY pair, not just bluechip + creator):
///   - No self-pair: the two entries must differ. Same denom on both sides
///     (`Bluechip("uatom")` + `Bluechip("uatom")`) or same address on both
///     sides (`CreatorToken("cosmos1...")` ×2) is rejected.
///   - `Bluechip { denom }`: denom must be non-empty. We do NOT enforce the
///     canonical bluechip_denom check from H3 here — standard pools can
///     include arbitrary native or IBC denoms (this is how the ATOM/bluechip
///     anchor pool is built).
///   - `CreatorToken { contract_addr }`: address must bech32-validate, AND
///     the address must answer a `Cw20QueryMsg::TokenInfo {}` query (so we
///     reject typos and non-CW20 contracts at creation rather than at first
///     deposit).
fn validate_standard_pool_token_info(
    deps: Deps,
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

    for entry in pair.iter() {
        match entry {
            TokenType::Native { denom } => {
                if denom.trim().is_empty() {
                    return Err(ContractError::Std(StdError::generic_err(
                        "Standard pool: Bluechip denom must be non-empty",
                    )));
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
    validate_standard_pool_token_info(deps.as_ref(), &pool_token_info)?;

    if label.trim().is_empty() {
        return Err(ContractError::Std(StdError::generic_err(
            "label must be non-empty",
        )));
    }

    // Compute required fee. USD-denominated config converted to bluechip
    // via the oracle; falls back to the hardcoded constant when the oracle
    // is unavailable (the bootstrap case — the very first standard pool,
    // typically the anchor itself, has no oracle data to draw on).
    let usd_fee = factory_config.standard_pool_creation_fee_usd;
    let (required_bluechip, fee_source) = if usd_fee.is_zero() {
        (Uint128::zero(), "disabled")
    } else {
        match crate::internal_bluechip_price_oracle::usd_to_bluechip(
            deps.as_ref(),
            usd_fee,
            env.clone(),
        ) {
            Ok(conv) if !conv.amount.is_zero() => (conv.amount, "oracle"),
            _ => (
                crate::state::STANDARD_POOL_CREATION_FEE_FALLBACK_BLUECHIP,
                "fallback",
            ),
        }
    };

    // Caller must supply at least the required amount in the canonical
    // bluechip denom. We don't refund overpayment — the caller controls
    // their input and can use the conversion query to size it precisely.
    let paid_bluechip = info
        .funds
        .iter()
        .find(|c| c.denom == factory_config.bluechip_denom)
        .map(|c| c.amount)
        .unwrap_or_default();
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

    // Forward whatever the caller paid to the bluechip wallet. If they
    // overpaid, the surplus stays with the protocol — same convention as
    // commit-fee handling.
    let mut messages: Vec<CosmosMsg> = Vec::new();
    if !paid_bluechip.is_zero() {
        messages.push(CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
            to_address: factory_config.bluechip_wallet_address.to_string(),
            amount: vec![cosmwasm_std::Coin {
                denom: factory_config.bluechip_denom.clone(),
                amount: paid_bluechip,
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
        .add_attribute("fee_source", fee_source)
        .add_attribute("label", label))
}
