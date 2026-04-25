use crate::{
    asset::TokenType,
    error::ContractError,
    execute::{encode_reply_id, FINALIZE_POOL, FINALIZE_STANDARD_POOL, MINT_CREATE_POOL},
    msg::CreatePoolReplyMsg,
    pool_create_cleanup::{extract_contract_address, give_pool_ownership_cw20_and_nft},
    pool_struct::{CommitFeeInfo, PoolDetails, ThresholdPayoutAmounts},
    state::{
        CreationStatus, FACTORYINSTANTIATEINFO, POOL_CREATION_CONTEXT, STANDARD_POOL_CREATION_CONTEXT,
    },
};
use cosmwasm_std::{
    to_json_binary, CosmosMsg, DepsMut, Env, Reply, Response, StdError, StdResult, SubMsg, Uint128,
    WasmMsg,
};
use pool_factory_interfaces::{cw721_msgs::Cw721InstantiateMsg, PoolKind, StandardPoolInstantiateMsg};

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
    let result = msg.result.into_result().map_err(|e| {
        ContractError::Std(StdError::generic_err(format!(
            "set_tokens reply_on_success saw Err (should be impossible): {}",
            e
        )))
    })?;

    let mut ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let token_address = extract_contract_address(&deps, &result)?;

    // Store only in ctx.temp; ctx.state.creator_token_address is now
    // derived at query time from ctx.temp. Saves one Addr clone per
    // pool creation.
    ctx.temp.creator_token_addr = Some(token_address.clone());
    ctx.state.status = CreationStatus::TokenCreated;
    POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;

    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let nft_instantiate_msg = to_json_binary(&Cw721InstantiateMsg {
        name: "AMM LP Positions".to_string(),
        symbol: "AMM-LP".to_string(),
        minter: env.contract.address.to_string(),
    })?;

    let nft_msg = WasmMsg::Instantiate {
        code_id: config.cw721_nft_contract_id,
        msg: nft_instantiate_msg,
        funds: vec![],
        admin: Some(env.contract.address.to_string()),
        label: format!("AMM-LP-NFT-{}", token_address),
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
    let result = msg.result.into_result().map_err(|e| {
        ContractError::Std(StdError::generic_err(format!(
            "mint_create_pool reply_on_success saw Err (should be impossible): {}",
            e
        )))
    })?;

    let mut ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let nft_address = extract_contract_address(&deps, &result)?;

    // Store only in ctx.temp; ctx.state.mint_new_position_nft_address is
    // now derived at query time from ctx.temp. Saves one Addr clone per
    // pool creation.
    ctx.temp.nft_addr = Some(nft_address.clone());
    ctx.state.status = CreationStatus::NftCreated;
    POOL_CREATION_CONTEXT.save(deps.storage, pool_id, &ctx)?;

    let factory_config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let token_address = ctx
        .temp
        .creator_token_addr
        .clone()
        .ok_or_else(|| ContractError::Std(StdError::generic_err("missing token address")))?;

    let threshold_payout = ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000),
        bluechip_reward_amount: Uint128::new(25_000_000_000),
        pool_seed_amount: Uint128::new(350_000_000_000),
        commit_return_amount: Uint128::new(500_000_000_000),
    };

    threshold_payout.validate(Uint128::new(1_200_000_000_000))?;

    let threshold_binary = to_json_binary(&threshold_payout)?;

    // Update asset infos with actual token address
    let mut updated_asset_infos = ctx.temp.temp_pool_info.pool_token_info.clone();
    for asset_info in updated_asset_infos.iter_mut() {
        if let TokenType::CreatorToken { contract_addr } = asset_info {
            if contract_addr.as_str() == "WILL_BE_CREATED_BY_FACTORY" {
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
        commit_amount_for_threshold: factory_config.commit_amount_for_threshold_bluechip,
        commit_threshold_limit_usd: factory_config.commit_threshold_limit_usd,
        token_address,
        position_nft_address: nft_address.clone(),
        max_bluechip_lock_per_pool: factory_config.max_bluechip_lock_per_pool,
        creator_excess_liquidity_lock_days: factory_config.creator_excess_liquidity_lock_days,
    };
    // Flat struct after 4d — creator-pool's `instantiate` now accepts
    // `PoolInstantiateMsg` (a struct, not a tagged enum) directly. JSON
    // shape matches what CommitPoolInstantiateMsg used to round-trip to
    // inside the removed `Commit(...)` variant.
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
    let result = msg.result.into_result().map_err(|e| {
        ContractError::Std(StdError::generic_err(format!(
            "finalize_pool reply_on_success saw Err (should be impossible): {}",
            e
        )))
    })?;

    let ctx = POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let pool_address = extract_contract_address(&deps, &result)?;

    let token_address = ctx
        .temp
        .creator_token_addr
        .clone()
        .ok_or_else(|| ContractError::Std(StdError::generic_err("missing token address")))?;
    let nft_address = ctx
        .temp
        .nft_addr
        .clone()
        .ok_or_else(|| ContractError::Std(StdError::generic_err("missing nft address")))?;

    let pool_details = PoolDetails {
        pool_id,
        pool_token_info: ctx.temp.temp_pool_info.pool_token_info.clone(),
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

    // Transfer ownership to pool
    let ownership_msgs =
        give_pool_ownership_cw20_and_nft(&token_address, &nft_address, &pool_address)?;

    // Creation succeeded end-to-end. The entire creation context
    // (temp + state) is dropped rather than left around with
    // status=Completed, which would accumulate indefinitely.
    POOL_CREATION_CONTEXT.remove(deps.storage, pool_id);

    // Single atomic write across the three pool-registry maps so
    // they cannot drift. See state::register_pool.
    crate::state::register_pool(deps.storage, pool_id, &pool_address, &pool_details)?;

    Ok(Response::new()
        .add_messages(ownership_msgs)
        .add_attribute("action", "pool_created_successfully")
        .add_attribute("pool_address", pool_address)
        .add_attribute("pool_id", pool_id.to_string()))
}

// ---------------------------------------------------------------------------
// H14 — Standard pool reply chain
// ---------------------------------------------------------------------------
//
// Standard pools have a 2-step reply chain (vs the commit-pool's 3 steps):
//   1. CW721 NFT instantiate (kicked off by `execute_create_standard_pool`)
//        -> reply lands in `mint_standard_nft`
//   2. Pool wasm instantiate (kicked off by `mint_standard_nft`)
//        -> reply lands in `finalize_standard_pool` which transfers NFT
//           ownership to the new pool and registers it.
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
    let result = msg.result.into_result().map_err(|e| {
        ContractError::Std(StdError::generic_err(format!(
            "mint_standard_nft reply_on_success saw Err (should be impossible): {}",
            e
        )))
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
    };
    // Dual-code_id routing (H14 4c): standard pools instantiate
    // against the separate standard-pool wasm, sending a flat
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
    let result = msg.result.into_result().map_err(|e| {
        ContractError::Std(StdError::generic_err(format!(
            "finalize_standard_pool reply_on_success saw Err (should be impossible): {}",
            e
        )))
    })?;

    let ctx = STANDARD_POOL_CREATION_CONTEXT.load(deps.storage, pool_id)?;
    let pool_address = extract_contract_address(&deps, &result)?;
    let nft_address = ctx
        .nft_addr
        .clone()
        .ok_or_else(|| ContractError::Std(StdError::generic_err("missing nft address")))?;

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

    STANDARD_POOL_CREATION_CONTEXT.remove(deps.storage, pool_id);

    crate::state::register_pool(deps.storage, pool_id, &pool_address, &pool_details)?;

    Ok(Response::new()
        .add_message(nft_transfer)
        .add_attribute("action", "standard_pool_created_successfully")
        .add_attribute("pool_address", pool_address)
        .add_attribute("pool_id", pool_id.to_string())
        .add_attribute("creator", ctx.creator.to_string()))
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
