#![allow(non_snake_case)]
use crate::asset::{
    call_pool_info, PairType, 
};
use crate::error::ContractError;
use crate::msg::{
    ConfigResponse, ExecuteMsg,
    FeeInfoResponse,MigrateMsg, PoolFeeStateResponse, PoolInfoResponse,
    PoolInitParams, PoolInstantiateMsg, PoolResponse, PoolStateResponse,
    QueryMsg, 
   
};
use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitInfo,
    OracleInfo,
    PairInfo,
    PoolFeeState,
    PoolInfo,
    PoolSpecs,
    ThresholdPayout,
    COMMITSTATUS,
    COMMIT_CONFIG,
    FEEINFO,
    NATIVE_RAISED,
    ORACLE_INFO,
    POOL_FEE_STATE,
    POOL_INFO,
    POOL_SPECS,
    POOL_STATE,
    THRESHOLD_HIT,
    THRESHOLD_PAYOUT,
    USD_RAISED,
 
};
use crate::state::{
    PoolState, Position,  LIQUIDITY_POSITIONS, NEXT_POSITION_ID,
   
};
use cosmwasm_std::{
    entry_point, from_json, to_json_binary, Addr,  Binary, Decimal,
    Deps, DepsMut,Env,  MessageInfo, Reply,
    Response, StdError, StdResult, SubMsgResult, Uint128, 
};
use cw2::{get_contract_version, set_contract_version};
use protobuf::Message;

/// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
/// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";

// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;

/// Contract name that is used for migration.
const CONTRACT_NAME: &str = "betfi-pair";
/// Contract version that is used for migration.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// ## Description
/// Creates a new contract with the specified parameters in the [`InstantiateMsg`].
/// Returns the [`Response`] with the specified attributes if the operation was successful, or a [`ContractError`] if
/// the contract was not created.
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
/// * **env** is an object of type [`Env`].
///
/// * **_info** is an object of type [`MessageInfo`].
/// * **msg** is a message of type [`InstantiateMsg`] which contains the basic settings for creating a contract.

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    //++++++++++++++++++++++++++++++++++++++++++++++need to check if the asset info is valid++++++++++++++++++++++++++++++++++++++++++++++++++++++++
    msg.asset_infos[0].check(deps.api)?;
    msg.asset_infos[1].check(deps.api)?;

    if msg.asset_infos[0] == msg.asset_infos[1] {
        return Err(ContractError::DoublingAssets {});
    }

    if (msg.fee_info.bluechip_fee + msg.fee_info.creator_fee) > Decimal::one() {
        return Err(ContractError::InvalidFee {});
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let pool_params: PoolInitParams = match msg.init_params {
        Some(data) => from_json(&data)?,
        None => return Err(StdError::generic_err("Missing init_params").into()),
    };

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pair_info: PairInfo {
            contract_addr: env.contract.address.clone(),
            liquidity_token: Addr::unchecked(""), // Set later
            asset_infos: msg.asset_infos.clone(),
            pair_type: PairType::Xyk {},
        },
        factory_addr: msg.factory_addr.clone(),
        token_address: msg.token_address.clone(),
        position_nft_address: msg.position_nft_address.clone(),
    };

    let liquidity_position = Position {
        liquidity: Uint128::zero(),
        owner: Addr::unchecked(""),
        fee_growth_inside_0_last: Decimal::zero(),
        fee_growth_inside_1_last: Decimal::zero(),
        created_at: env.block.time.seconds(),
        last_fee_collection: env.block.time.seconds(),
    };

    let pool_specs = PoolSpecs {
        subscription_period: 2592000,   // 30 days in seconds
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayout {
        creator_amount: pool_params.creator_amount,
        bluechip_amount: pool_params.bluechip_amount,
        pool_amount: pool_params.pool_amount,
        commit_amount: pool_params.commit_amount,
    };

    let commit_config = CommitInfo {
        commit_limit: msg.commit_limit,
        commit_limit_usd: msg.commit_limit_usd,
        available_payment: msg.available_payment.clone(),
        available_payment_usd: msg.available_payment_usd.clone(),
    };

    let oracle_info = OracleInfo {
        oracle_addr: msg.oracle_addr.clone(),
        oracle_symbol: msg.oracle_symbol.clone(),
    };

    let pool_state = PoolState {
        total_liquidity: Uint128::zero(),
        block_time_last: env.block.time.seconds(),
        reserve0: Uint128::zero(), // native token
        reserve1: Uint128::zero(),
        price0_cumulative_last: Uint128::zero(),
        price1_cumulative_last: Uint128::zero(),
        nft_ownership_accepted: false, // Initially false, set to true after NFT ownership is verified
    };

    let pool_fee_state = PoolFeeState {
        fee_growth_global_0: Decimal::zero(),
        fee_growth_global_1: Decimal::zero(),
        total_fees_collected_0: Uint128::zero(),
        total_fees_collected_1: Uint128::zero(),
    };

    USD_RAISED.save(deps.storage, &Uint128::zero())?;
    FEEINFO.save(deps.storage, &msg.fee_info)?;
    COMMITSTATUS.save(deps.storage, &Uint128::zero())?;
    NATIVE_RAISED.save(deps.storage, &Uint128::zero())?;
    THRESHOLD_HIT.save(deps.storage, &false)?;
    NEXT_POSITION_ID.save(deps.storage, &0u64)?;
    POOL_INFO.save(deps.storage, &pool_info)?;
    POOL_FEE_STATE.save(deps.storage, &pool_fee_state)?;
    POOL_STATE.save(deps.storage, &pool_state)?;
    POOL_SPECS.save(deps.storage, &pool_specs)?;
    THRESHOLD_PAYOUT.save(deps.storage, &threshold_payout_amounts)?;
    COMMIT_CONFIG.save(deps.storage, &commit_config)?;
    LIQUIDITY_POSITIONS.save(deps.storage, "0", &liquidity_position)?;
    ORACLE_INFO.save(deps.storage, &oracle_info)?;
    // Create the LP token contract
    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("pool", env.contract.address.to_string()))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::UpdateConfig { .. } => Err(ContractError::NonSupported {}),
}
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, ContractError> {
    if msg.id == 42 {
        let res = match msg.result {
            SubMsgResult::Ok(res) => res,
            SubMsgResult::Err(err) => {
                return Err(StdError::generic_err(format!("submsg error: {err}")).into())
            }
        };

        let data: Binary = res
            .data
            .ok_or_else(|| StdError::not_found("instantiate data"))?;
        let parsed: MsgInstantiateContractResponse = Message::parse_from_bytes(data.as_slice())
            .map_err(|_| {
                StdError::parse_err(
                    "MsgInstantiateContractResponse",
                    "invalid instantiate reply data",
                )
            })?;

        let lp: Addr = deps.api.addr_validate(parsed.get_contract_address())?;

        // CHANGED: Update POOL_INFO instead of CONFIG
        POOL_INFO.update(deps.storage, |mut pool_info| -> Result<_, ContractError> {
            pool_info.pair_info.liquidity_token = lp.clone();
            Ok(pool_info)
        })?;

        return Ok(Response::new().add_attribute("lp_token", lp));
    }

    Err(StdError::generic_err("unknown reply id").into())
}

/// ## Description
/// Calculates the amount of fees the Maker contract gets according to specified pair parameters.
/// Returns a [`None`] if the Maker fee is zero, otherwise returns a [`Asset`] struct with the specified attributes.
/// ## Params
/// * **pool_info** is an object of type [`AssetInfo`]. Contains information about the pool asset for which the commission will be calculated.
///
/// * **commission_amount** is an object of type [`Env`]. This is the total amount of fees charged for a swap.
///
/// * **maker_commission_rate** is an object of type [`MessageInfo`]. This is the percentage of fees that go to the Maker contract.


/// Returns a [`ContractError`] on failure.
/// If `belief_price` and `max_spread` are both specified, we compute a new spread,
/// otherwise we just use the swap spread to check `max_spread`.
/// ## Params
/// * **belief_price** is an object of type [`Option<Decimal>`]. This is the belief price used in the swap.
///
/// * **max_spread** is an object of type [`Option<Decimal>`]. This is the
/// max spread allowed so that the swap can be executed successfuly.
///
/// * **offer_amount** is an object of type [`Uint128`]. This is the amount of assets to swap.
///
/// * **return_amount** is an object of type [`Uint128`]. This is the amount of assets to receive from the swap.
///
/// * **spread_amount** is an object of type [`Uint128`]. This is the spread used in the swap.


/// ## Description
/// Used for the contract migration. Returns a default object of type [`Response`].
/// ## Params
/// * **deps** is an object of type [`DepsMut`].
///
/// * **_env** is an object of type [`Env`].
///
/// * **_msg** is an object of type [`MigrateMsg`].
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    let version = get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(ContractError::CannotMigrate {
            previous_contract: version.contract,
        });
    }
    if version.version != CONTRACT_VERSION {
        return Err(ContractError::CannotMigrate {
            previous_contract: version.contract,
        });
    }

    Ok(Response::new()
        .add_attribute("previous_contract_name", &version.contract)
        .add_attribute("previous_contract_version", &version.version)
        .add_attribute("new_contract_name", CONTRACT_NAME)
        .add_attribute("new_contract_version", CONTRACT_VERSION))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::PoolInfo {} => to_json_binary(&query_pool_info(deps)?),
        QueryMsg::Pair {} => to_json_binary(&query_pair_info(deps)?),
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::FeeInfo {} => to_json_binary(&query_fee_info(deps)?),

    }
}

/// ## Description
/// Returns information about the pair contract in an object of type [`PairInfo`].
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_pair_info(deps: Deps) -> StdResult<PairInfo> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    Ok(pool_info.pair_info)
}
/// ## Description
/// Returns the amounts of assets in the pair contract as well as the amount of LP
/// tokens currently minted in an object of type [`PoolResponse`].
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_pool(deps: Deps) -> StdResult<PoolResponse> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let assets = call_pool_info(deps, pool_info)?;

    let resp = PoolResponse { assets };

    Ok(resp)
}




/// ## Description
/// Returns the pair contract configuration in a [`ConfigResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let pool_state = POOL_STATE.load(deps.storage)?;
    Ok(ConfigResponse {
        block_time_last: pool_state.block_time_last,
        params: None,
    })
}

/// ## Description
/// Returns the pair contract configuration in a [`FeeInfoResponse`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_fee_info(deps: Deps) -> StdResult<FeeInfoResponse> {
    let fee_info = FEEINFO.load(deps.storage)?;
    Ok(FeeInfoResponse { fee_info })
}

/// ## Description
/// Returns the pair contract configuration in a [`bool`] object.
/// ## Params
/// * **deps** is an object of type [`Deps`].
pub fn query_check_commit(deps: Deps) -> StdResult<bool> {
    let commit_info = COMMIT_CONFIG.load(deps.storage)?;
    let usd_raised = COMMITSTATUS.load(deps.storage)?;
    // true once we've raised at least the USD threshold
    Ok(usd_raised >= commit_info.commit_limit_usd)
}

fn query_pool_info(deps: Deps) -> StdResult<PoolInfoResponse> {
    let pool_fee_state = POOL_FEE_STATE.load(deps.storage)?;
    let next_position_id = NEXT_POSITION_ID.load(deps.storage)?;
    let pool_state = POOL_STATE.load(deps.storage)?;

    Ok(PoolInfoResponse {
        pool_state: PoolStateResponse {
            nft_ownership_accepted: pool_state.nft_ownership_accepted,
            reserve0: pool_state.reserve0,
            reserve1: pool_state.reserve1,
            total_liquidity: pool_state.total_liquidity,
            block_time_last: pool_state.block_time_last,
        },
        fee_state: PoolFeeStateResponse {
            fee_growth_global_0: pool_fee_state.fee_growth_global_0,
            fee_growth_global_1: pool_fee_state.fee_growth_global_1,
            total_fees_collected_0: pool_fee_state.total_fees_collected_0,
            total_fees_collected_1: pool_fee_state.total_fees_collected_1,
        },
        total_positions: next_position_id,
    })
}


// Helper function to calculate unclaimed fees
