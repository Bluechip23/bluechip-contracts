use crate::{
    asset::TokenType,
    error::ContractError,
    execute::{encode_reply_id, FINALIZE_POOL, FINALIZE_STANDARD_POOL, MINT_CREATE_POOL},
    msg::CreatePoolReplyMsg,
    pool_create_cleanup::{extract_contract_address, give_pool_ownership_cw20_and_nft},
    pool_struct::{CommitFeeInfo, PoolDetails},
    state::{
        CreationStatus, FACTORYINSTANTIATEINFO, POOL_CREATION_CONTEXT, STANDARD_POOL_CREATION_CONTEXT,
    },
};
use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, Reply, Response, StdError, StdResult, SubMsg, WasmMsg,
};
use pool_factory_interfaces::{cw721_msgs::Cw721InstantiateMsg, PoolKind, StandardPoolInstantiateMsg};

/// CW721 NFT branding for liquidity-position NFTs minted on commit-pool
/// creation. Hoisted to module scope so a deployment-specific re-skin
/// (white-label, fork) is a single edit per constant. Pool-creation
/// label format is `LP_NFT_LABEL_PREFIX{token_addr}` so the on-chain
/// label always carries the deterministic creator-token suffix.
const LP_NFT_NAME: &str = "AMM LP Positions";
const LP_NFT_SYMBOL: &str = "AMM-LP";
const LP_NFT_LABEL_PREFIX: &str = "AMM-LP-NFT-";

// pool_creation_reply.rs
//
// Every step of the pool-creation reply chain uses `SubMsg::reply_on_success`.
// Under that dispatch mode, a failing submessage bypasses the reply handler
// and propagates the error up through the entire tx, rolling back ALL state
// writes atomically (including prior successful reply handlers' writes and
// the CW20/CW721 instantiations themselves). So the handlers below only need
// to implement the happy path; a defensive `into_result` guards against a
// future change to `reply_always` / `reply_on_error` without also updating
// these handlers.

pub fn set_tokens(
    deps: DepsMut,
    env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    let reply_id = msg.id;
    let result = msg.result.into_result().map_err(|e| {
        ContractError::ReplyOnSuccessSawError {
            id: reply_id,
            msg: format!("set_tokens: {}", e),
        }
    })?;

    let mut ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let token_address = extract_contract_address(&deps, &result)?;

    // Store only in ctx.temp; ctx.state.creator_token_address is
    // derived at query time from ctx.temp.
    ctx.temp.creator_token_addr = Some(token_address.clone());
    ctx.state.status = CreationStatus::TokenCreated;
    POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;

    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let nft_instantiate_msg = to_json_binary(&Cw721InstantiateMsg {
        name: LP_NFT_NAME.to_string(),
        symbol: LP_NFT_SYMBOL.to_string(),
        minter: env.contract.address.to_string(),
    })?;

    let nft_msg = WasmMsg::Instantiate {
        code_id: config.cw721_nft_contract_id,
        msg: nft_instantiate_msg,
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: format!("{}{}", LP_NFT_LABEL_PREFIX, token_address),
    };

    let sub_msg = SubMsg::reply_on_success(nft_msg, encode_reply_id(pool_id, MINT_CREATE_POOL));

    Ok(Response::new()
        .add_attribute("action", "token_created_successfully")
        .add_attribute("token_address", token_address)
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessage(sub_msg))
}

pub fn mint_create_pool(
    deps: DepsMut,
    env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    let reply_id = msg.id;
    let result = msg.result.into_result().map_err(|e| {
        ContractError::ReplyOnSuccessSawError {
            id: reply_id,
            msg: format!("mint_create_pool: {}", e),
        }
    })?;

    let mut ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let nft_address = extract_contract_address(&deps, &result)?;

    // Store only in ctx.temp; ctx.state.mint_new_position_nft_address is
    // derived at query time from ctx.temp.
    ctx.temp.nft_addr = Some(nft_address.clone());
    ctx.state.status = CreationStatus::NftCreated;
    POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let token_address = ctx.temp.creator_token_addr.clone().ok_or(
        ContractError::ReplyMissingAddress {
            step: "mint_create_pool",
            kind: "token",
        },
    )?;

    // Threshold-payout splits live on `FactoryInstantiate` so they
    // ride the standard 48h propose/apply flow. `validate()` is also
    // called at propose time; calling it here is belt-and-suspenders
    // for old serialized records that bypassed the gate.
    let threshold_payout = factory_config.threshold_payout_amounts.clone();
    threshold_payout.validate()?;

    let threshold_binary = to_json_binary(&threshold_payout)?;

    // Update asset infos with actual token address. The sentinel is the
    // string the factory's commit-pool create handler accepts in the
    // `CreatorToken` slot at submit time (see `validate_pool_token_info`).
    let mut updated_asset_infos = ctx.temp.temp_pool_info.pool_token_info.clone();
    for asset_info in updated_asset_infos.iter_mut() {
        if let TokenType::CreatorToken { contract_addr } = asset_info {
            if contract_addr.as_str() == crate::execute::pool_lifecycle::create::CREATOR_TOKEN_SENTINEL {
                *contract_addr = token_address.clone();
            }
        }
    }
    let commit_msg = CreatePoolReplyMsg {
        pool_id,
        pool_token_info: updated_asset_infos,
        used_factory_addr: env.contract.address.clone(),
        cw20_token_contract_id: factory_config.cw20_token_contract_id,
        threshold_payout: Some(threshold_binary),
        commit_fee_info: CommitFeeInfo {
            bluechip_wallet_address: factory_config.bluechip_wallet_address.clone(),
            creator_wallet_address: ctx.temp.temp_creator_wallet.clone(),
            commit_fee_bluechip: factory_config.commit_fee_bluechip,
            commit_fee_creator: factory_config.commit_fee_creator,
        },
        commit_threshold_limit_usd: factory_config.commit_threshold_limit_usd,
        token_address,
        position_nft_address: nft_address.clone(),
        max_bluechip_lock_per_pool: factory_config.max_bluechip_lock_per_pool,
        creator_excess_liquidity_lock_days: factory_config.creator_excess_liquidity_lock_days,
    };
    let pool_msg = WasmMsg::Instantiate {
        code_id: factory_config.create_pool_wasm_contract_id,
        msg: to_json_binary(&commit_msg)?,
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: format!("Pool-{}", pool_id),
    };

    let sub_msg = SubMsg::reply_on_success(pool_msg, encode_reply_id(pool_id, FINALIZE_POOL));

    Ok(Response::new()
        .add_attribute("action", "nft_created_successfully")
        .add_attribute("nft_address", nft_address)
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessage(sub_msg))
}

pub fn finalize_pool(
    deps: DepsMut,
    _env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    let reply_id = msg.id;
    let result = msg.result.into_result().map_err(|e| {
        ContractError::ReplyOnSuccessSawError {
            id: reply_id,
            msg: format!("finalize_pool: {}", e),
        }
    })?;

    let ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let pool_address = extract_contract_address(&deps, &result)?;

    let token_address = ctx.temp.creator_token_addr.clone().ok_or(
        ContractError::ReplyMissingAddress {
            step: "finalize_pool",
            kind: "token",
        },
    )?;
    let nft_address = ctx.temp.nft_addr.clone().ok_or(
        ContractError::ReplyMissingAddress {
            step: "finalize_pool",
            kind: "nft",
        },
    )?;

    // Rebuild `pool_token_info` from the source of truth for the
    // creator-token address, which is `ctx.temp.creator_token_addr`
    // (set in `set_tokens` when the CW20 was instantiated). The original
    // `ctx.temp.temp_pool_info.pool_token_info` still carries the literal
    // `CREATOR_TOKEN_SENTINEL` placeholder string the user supplied at
    // create time. Persisting it unchanged into `POOLS_BY_ID` would leave
    // every commit pool's registry entry with the placeholder address in
    // the CreatorToken slot, breaking `query_creator_token_info` and any
    // other consumer that reads the CW20 address out of the registry.
    //
    // Reconstructing here from `creator_token_addr` is unambiguous —
    // the validator at `validate_pool_token_info` enforces a strict
    // [Native(bluechip), CreatorToken(sentinel)] shape, so the bluechip
    // side is always at index 0 and the only field that needs the
    // real address is the CreatorToken at index 1.
    let bluechip_side = ctx.temp.temp_pool_info.pool_token_info[0].clone();
    let pool_token_info = [
        bluechip_side,
        TokenType::CreatorToken {
            contract_addr: token_address.clone(),
        },
    ];

    let pool_details = PoolDetails {
        pool_id,
        pool_token_info,
        creator_pool_addr: pool_address.clone(),
        // This reply handler is specifically for the commit-pool creation
        // chain (triggered by ExecuteMsg::Create). Standard pools have
        // their own reply chain that sets pool_kind = Standard.
        pool_kind: pool_factory_interfaces::PoolKind::Commit,
        // Captured at create time on `PoolCreationContext.commit_pool_ordinal`
        // so the threshold-mint decay formula uses commit-pool-count
        // semantics rather than a global pool counter mixed with
        // permissionlessly-created standard pools.
        commit_pool_ordinal: ctx.commit_pool_ordinal,
    };

    let ownership_msgs =
        give_pool_ownership_cw20_and_nft(&token_address, &nft_address, &pool_address)?;

    // Symmetric two-phase NFT accept (mirrors `finalize_standard_pool`).
    // `give_pool_ownership_cw20_and_nft` only emits the CW721
    // `TransferOwnership` (cw_ownable is two-phase: sets pending_owner,
    // current owner unchanged). Without this trigger, the factory
    // remained the NFT contract's actual owner until the lazy
    // `AcceptOwnership` in `trigger_threshold_payout` fired at threshold
    // cross — potentially never, for a pool that fails to threshold.
    // Dispatching `AcceptNftOwnership {}` to the freshly-created pool
    // here closes that window inside the create tx: the pool's handler
    // emits the matching `AcceptOwnership` to the NFT and the create tx
    // ends with the pool as actual owner.
    let pool_accept_trigger = build_pool_accept_nft_ownership_call(&pool_address)?;

    // Creation succeeded end-to-end. The entire creation context
    // (temp + state) is dropped rather than left around with
    // status=Completed, which would accumulate indefinitely.
    POOL_CREATION_CONTEXT.remove(deps.storage, pool_id);

    // Single atomic write across the three pool-registry maps so
    // they cannot drift. See state::register_pool.
    crate::state::register_pool(deps.storage, pool_id, &pool_address, &pool_details)?;

    Ok(Response::new()
        // Order matters: `ownership_msgs` carries the CW721
        // `TransferOwnership` that sets `pending_owner = pool` on the
        // NFT. The accept-trigger executes next, and its handler emits
        // the matching `AcceptOwnership` to the NFT. Reversing the
        // order would have the pool attempt to accept before being
        // staged as `pending_owner` and the NFT contract would reject
        // with `NoPendingOwner`, reverting the entire create tx.
        .add_messages(ownership_msgs)
        .add_message(pool_accept_trigger)
        .add_attribute("action", "pool_created_successfully")
        .add_attribute("pool_address", pool_address)
        .add_attribute("pool_id", pool_id.to_string()))
}

// ---------------------------------------------------------------------------
// Standard pool reply chain
// ---------------------------------------------------------------------------
//
// Standard pools have a 2-step reply chain (vs the commit-pool's 3 steps):
// 1. CW721 NFT instantiate (kicked off by `execute_create_standard_pool`)
// -> reply lands in `mint_standard_nft`
// 2. Pool wasm instantiate (kicked off by `mint_standard_nft`)
// -> reply lands in `finalize_standard_pool` which transfers NFT
// ownership to the new pool and registers it.
//
// No CW20 instantiate step — standard pools wrap pre-existing tokens.
// No CW20 ownership transfer at finalize — there's nothing to transfer.
// Same `reply_on_success` atomicity guarantees as the commit-pool chain
// (see pool_create_cleanup.rs file-level comment).

pub fn mint_standard_nft(
    deps: DepsMut,
    env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    let reply_id = msg.id;
    let result = msg.result.into_result().map_err(|e| {
        ContractError::ReplyOnSuccessSawError {
            id: reply_id,
            msg: format!("mint_standard_nft: {}", e),
        }
    })?;

    let mut ctx = STANDARD_POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let nft_address = extract_contract_address(&deps, &result)?;
    ctx.nft_addr = Some(nft_address.clone());
    STANDARD_POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;

    let std_msg = StandardPoolInstantiateMsg {
        pool_id,
        pool_token_info: ctx.pool_token_info.clone(),
        used_factory_addr: env.contract.address.clone(),
        position_nft_address: nft_address.clone(),
        // Real wallet address sourced from factory config so that
        // an emergency drain on a standard pool sends funds to a
        // controllable wallet instead of the factory contract (which
        // has no withdrawal mechanism).
        bluechip_wallet_address: factory_config.bluechip_wallet_address.clone(),
    };
    // Dual-code_id routing: standard pools instantiate against the
    // separate standard-pool wasm, sending a flat
    // StandardPoolInstantiateMsg (standard-pool's `instantiate` takes
    // that type directly — no tagged-enum wrapper).
    if factory_config.standard_pool_wasm_contract_id == 0 {
        return Err(ContractError::Std(StdError::generic_err(
            "standard_pool_wasm_contract_id is not configured; \
             propose a factory config update that sets it before \
             creating standard pools",
        )));
    }
    let pool_msg = WasmMsg::Instantiate {
        code_id: factory_config.standard_pool_wasm_contract_id,
        msg: to_json_binary(&std_msg)?,
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: ctx.label.clone(),
    };

    let sub_msg =
        SubMsg::reply_on_success(pool_msg, encode_reply_id(pool_id, FINALIZE_STANDARD_POOL));

    Ok(Response::new()
        .add_attribute("action", "standard_nft_created")
        .add_attribute("nft_address", nft_address)
        .add_attribute("pool_id", pool_id.to_string())
        .add_submessage(sub_msg))
}

pub fn finalize_standard_pool(
    deps: DepsMut,
    _env: Env,
    msg: Reply,
    pool_id: u64,
) -> Result<Response, ContractError> {
    let reply_id = msg.id;
    let result = msg.result.into_result().map_err(|e| {
        ContractError::ReplyOnSuccessSawError {
            id: reply_id,
            msg: format!("finalize_standard_pool: {}", e),
        }
    })?;

    let ctx = STANDARD_POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let pool_address = extract_contract_address(&deps, &result)?;
    let nft_address = ctx.nft_addr.clone().ok_or(
        ContractError::ReplyMissingAddress {
            step: "finalize_standard_pool",
            kind: "nft",
        },
    )?;

    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: ctx.pool_token_info.clone(),
        creator_pool_addr: pool_address.clone(),
        pool_kind: PoolKind::Standard,
        // Standard pools never participate in the commit-pool decay
        // schedule. Zero ordinal flags this in `calculate_and_mint_bluechip`
        // (which never runs for standard pools anyway, but defense-in-depth).
        commit_pool_ordinal: 0,
    };

    // Standard pools have only the NFT to transfer (no CW20 minter to
    // hand off — the pool wraps pre-existing CW20s that already have
    // their own minters set elsewhere, or two native denoms with no
    // minter at all).
    let nft_transfer = give_pool_nft_ownership(&nft_address, &pool_address)?;

    // Close the pending-ownership window in the same tx as
    // pool creation. The NFT contract sees TransferOwnership first
    // (from the factory) → pool becomes pending_owner; then the pool
    // itself processes AcceptNftOwnership and sends AcceptOwnership
    // back to the NFT. By the end of this tx the NFT contract has
    // the pool as its actual owner, with no window for an outside
    // observer to interact with the half-transferred NFT.
    let pool_accept_trigger = build_pool_accept_nft_ownership_call(&pool_address)?;

    STANDARD_POOL_CREATION_CONTEXT.remove(deps.storage, pool_id);

    crate::state::register_pool(deps.storage, pool_id, &pool_address, &pool_details)?;

    Ok(Response::new()
        // Order matters: TransferOwnership must process before the
        // pool tries to accept, otherwise the NFT contract would
        // reject AcceptOwnership with NoPendingOwner.
        .add_message(nft_transfer)
        .add_message(pool_accept_trigger)
        .add_attribute("action", "standard_pool_created_successfully")
        .add_attribute("pool_address", pool_address)
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("creator", ctx.creator.to_string()))
}

/// Minimal typed mirror of the pool-side ExecuteMsg variants the factory
/// ever needs to call back into. Intentionally NOT a re-export of
/// `standard_pool::msg::ExecuteMsg` / `creator_pool::msg::ExecuteMsg` —
/// the factory must not take a circular dep on either pool crate. Both
/// pool kinds expose the same `AcceptNftOwnership {}` variant with the
/// same snake_case wire shape, so a single helper here serves both
/// finalize paths. Wire compatibility is locked in by the round-trip
/// parse tests in each pool crate's testing module.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum PoolFactoryCallback {
    AcceptNftOwnership {},
}

/// Builds the `Wasm::Execute { AcceptNftOwnership {} }` call back into
/// a freshly-created pool (either kind). Sender on the resulting
/// transaction is the factory contract, which is what the pool-side
/// `execute_accept_nft_ownership` handlers authorise on.
fn build_pool_accept_nft_ownership_call(pool_addr: &cosmwasm_std::Addr) -> StdResult<CosmosMsg> {
    Ok(WasmMsg::Execute {
        contract_addr: pool_addr.to_string(),
        msg: to_json_binary(&PoolFactoryCallback::AcceptNftOwnership {})?,
        funds: vec![],
    }
    .into())
}

/// Standalone NFT-ownership transfer for the standard-pool finalize path.
/// The commit-pool helper `give_pool_ownership_cw20_and_nft` bundles the
/// CW20 minter handoff, which standard pools don't need; rather than
/// branching that helper, we keep the two flows clean with separate
/// builders.
fn give_pool_nft_ownership(nft_addr: &cosmwasm_std::Addr, pool_addr: &cosmwasm_std::Addr) -> StdResult<CosmosMsg> {
    use pool_factory_interfaces::cw721_msgs::{Action, Cw721ExecuteMsg};
    Ok(WasmMsg::Execute {
        contract_addr: nft_addr.to_string(),
        msg: to_json_binary(&Cw721ExecuteMsg::<()>::UpdateOwnership(
            Action::TransferOwnership {
                new_owner: pool_addr.to_string(),
                expiry: None,
            },
        ))?,
        funds: vec![],
    }
    .into())
}
