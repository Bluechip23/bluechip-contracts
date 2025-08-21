#![allow(non_snake_case)]
use crate::asset::{Asset, AssetInfo, PairType};
use crate::error::ContractError;
use crate::msg::{
   ExecuteMsg,
    MigrateMsg, 
    PoolInstantiateMsg, 
  
};
use crate::oracle::{OracleData, PriceResponse, PythQueryMsg};
use crate::response::MsgInstantiateContractResponse;
use crate::state::{
    CommitInfo, ExpectedFactory, OracleInfo, PairInfo, PoolFeeState, PoolInfo, PoolSpecs,
    ThresholdPayout, COMMITSTATUS, COMMIT_CONFIG, COMMIT_LEDGER, EXPECTED_FACTORY, FEEINFO,
    MAX_ORACLE_AGE, NATIVE_RAISED, ORACLE_INFO, POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE,
    RATE_LIMIT_GUARD, THRESHOLD_HIT, THRESHOLD_PAYOUT,  USD_RAISED,
    USER_LAST_COMMIT,
};
use crate::state::{
    Commiting, PoolState, Position, COMMIT_INFO, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID,
};
use cosmwasm_std::{
    entry_point, from_json,Addr, Binary, Coin, CosmosMsg, Decimal,
    DepsMut, Env, Fraction, MessageInfo, QuerierWrapper, Reply,
    Response, StdError, StdResult, SubMsgResult, Timestamp, Uint128, Uint256, 
};
use cw2::{get_contract_version, set_contract_version};
use protobuf::Message;
use std::str::FromStr;
use std::vec;
// The default swap slippage
pub const DEFAULT_SLIPPAGE: &str = "0.005";
// The maximum allowed swap slippage
pub const MAX_ALLOWED_SLIPPAGE: &str = "0.5";

// Decimal precision for TWAP results
pub const TWAP_PRECISION: u8 = 6;

// Contract name that is used for migration.
const CONTRACT_NAME: &str = "betfi-pair";
// Contract version that is used for migration.
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: PoolInstantiateMsg,
) -> Result<Response, ContractError> {
    //ensure the correct factory contract address was used in creating the pool
    let cfg = ExpectedFactory {
        expected_factory_address: msg.factory_addr.clone(),
    };
    EXPECTED_FACTORY.save(deps.storage, &cfg)?;

    let real_factory = EXPECTED_FACTORY.load(deps.storage)?;

    validate_factory_address(&real_factory.expected_factory_address, &msg.factory_addr)?;

    if info.sender != real_factory.expected_factory_address {
        return Err(ContractError::Unauthorized {});
    }
    msg.asset_infos[0].check(deps.api)?;
    msg.asset_infos[1].check(deps.api)?;

    if msg.asset_infos[0] == msg.asset_infos[1] {
        return Err(ContractError::DoublingAssets {});
    }

    if (msg.fee_info.bluechip_fee + msg.fee_info.creator_fee) > Decimal::one() {
        return Err(ContractError::InvalidFee {});
    }
    let threshold_payouts = if let Some(params_binary) = msg.threshold_payout {
        let params: ThresholdPayout = from_json(&params_binary)?;
        //make sure params match - no funny business with token minting.
        //checks total value and predetermined amounts for creator, BlueChip, original subscribers (commit amount), and the pool itself
        validate_pool_threshold_payments(&params)?;
        params
    } else {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Your params could not be validated during pool instantiation."),
        });
    };
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let pool_info = PoolInfo {
        pool_id: msg.pool_id,
        pair_info: PairInfo {
            contract_addr: env.contract.address.clone(),
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
        fee_multiplier: Decimal::one(),
    };

    let pool_specs = PoolSpecs {
        lp_fee: Decimal::permille(3),   // 0.3% LP fee
        min_commit_interval: 13,        // Minimum commit interval in seconds
        usd_payment_tolerance_bps: 100, // 1% tolerance
    };

    let threshold_payout_amounts = ThresholdPayout {
        creator_amount: threshold_payouts.creator_amount,
        bluechip_amount: threshold_payouts.bluechip_amount,
        pool_amount: threshold_payouts.pool_amount,
        commit_amount: threshold_payouts.commit_amount,
    };

    let commit_config = CommitInfo {
        commit_limit_usd: msg.commit_limit_usd,
        commit_amount_for_threshold: msg.commit_amount_for_threshold,
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
        // Initially false, set to true after NFT ownership is verified
        nft_ownership_accepted: false,
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
fn validate_pool_threshold_payments(params: &ThresholdPayout) -> Result<(), ContractError> {
    // the ONLY acceptable values
    const EXPECTED_CREATOR: u128 = 325_000_000_000;
    const EXPECTED_BLUECHIP: u128 = 25_000_000_000;
    const EXPECTED_POOL: u128 = 350_000_000_000;
    const EXPECTED_COMMIT: u128 = 500_000_000_000;
    const EXPECTED_TOTAL: u128 = 1_200_000_000_000;

    // verify each amount specifically - creator amount
    if params.creator_amount != Uint128::new(EXPECTED_CREATOR) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Creator amount must be {}", EXPECTED_CREATOR),
        });
    }
    //bluechip amount
    if params.bluechip_amount != Uint128::new(EXPECTED_BLUECHIP) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("BlueChip amount must be {}", EXPECTED_BLUECHIP),
        });
    }
    //pool seeding amount
    if params.pool_amount != Uint128::new(EXPECTED_POOL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Pool amount must be {}", EXPECTED_POOL),
        });
    }
    //amount sent back to origincal commiters
    if params.commit_amount != Uint128::new(EXPECTED_COMMIT) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Commit amount must be {}", EXPECTED_COMMIT),
        });
    }

    // Verify total
    let total =
        params.creator_amount + params.bluechip_amount + params.pool_amount + params.commit_amount;
    //throw error if anything of them is off - there is also a max mint number to help with the exactness
    if total != Uint128::new(EXPECTED_TOTAL) {
        return Err(ContractError::InvalidThresholdParams {
            msg: format!("Total must equal {} (got {})", EXPECTED_TOTAL, total),
        });
    }

    Ok(())
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
        //special swap funcntion that behaves differently before and after a threshold - 
        //contributed to commit ledger prior to crossing the threshold - acts a swap post threshold
        ExecuteMsg::Commit {
            asset,
            amount,
            deadline,
            belief_price,
            max_spread,
        } => commit(
            deps,
            env,
            info,
            asset,
            amount,
            deadline,
            belief_price,
            max_spread,
        ),
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

        POOL_INFO.update(deps.storage, |pool_info| -> Result<_, ContractError> {
            Ok(pool_info)
        })?;

        return Ok(Response::new().add_attribute("lp_token", lp));
    }

    Err(StdError::generic_err("unknown reply id").into())
}

//this is a relatively simple addition so I have kept a few helper functions that are not needed but will be needed for post threshold commits
//I only did this so you dont get a million things in the post threshold logic since that gets a litte more complicated

//Not used in the pre threshold commit. this tracks how many fees accumulate from a swap with the pool. 
//You do not need to full understand it, just a pre cursor for future transactions. 
fn update_fee_growth(
    pool_fee_state: &mut PoolFeeState,
    pool_state: &PoolState,
    offer_pool_idx: usize,
    commission_amt: Uint128,
) -> Result<(), ContractError> {
    if pool_state.total_liquidity.is_zero() || commission_amt.is_zero() {
        return Ok(());
    }
    let fee_growth = Decimal::from_ratio(commission_amt, pool_state.total_liquidity);
    if offer_pool_idx == 0 {
        // Token0 offered, fees collected in token0
        pool_fee_state.fee_growth_global_0 += fee_growth;
        pool_fee_state.total_fees_collected_0 += commission_amt;
    } else {
        // Token1 offered, fees collected in token1
        pool_fee_state.fee_growth_global_1 += fee_growth;
        pool_fee_state.total_fees_collected_1 += commission_amt;
    }
    Ok(())
}

// Update price accumulator with time-weighted average
//not used, pre threshold commits do not have a changing price. 
fn update_price_accumulator(
    pool_state: &mut PoolState,
    current_time: u64,
) -> Result<(), ContractError> {
    let time_elapsed = current_time.saturating_sub(pool_state.block_time_last);

    if time_elapsed > 0 && !pool_state.reserve0.is_zero() && !pool_state.reserve1.is_zero() {
        // Calculate price * time_elapsed directly
        let price0_increment = pool_state
            .reserve1
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve0)
            .map_err(|_| ContractError::DivideByZero)?;

        let price1_increment = pool_state
            .reserve0
            .checked_mul(Uint128::from(time_elapsed))
            .map_err(ContractError::from)?
            .checked_div(pool_state.reserve1)
            .map_err(|_| ContractError::DivideByZero)?;

        pool_state.price0_cumulative_last = pool_state
            .price0_cumulative_last
            .checked_add(price0_increment)?;
        pool_state.price1_cumulative_last = pool_state
            .price1_cumulative_last
            .checked_add(price1_increment)?;
        pool_state.block_time_last = current_time;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]

pub fn commit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    asset: Asset,
    amount: Uint128,
    deadline: Option<Timestamp>,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
) -> Result<Response, ContractError> {
    enforce_deadline(env.block.time, deadline)?;
    // stope spam attacks by setting a 13 second interval for transactions
    let rate_limit_guard = RATE_LIMIT_GUARD.may_load(deps.storage)?.unwrap_or(false);
    if rate_limit_guard {
        return Err(ContractError::ReentrancyGuard {});
    }
    RATE_LIMIT_GUARD.save(deps.storage, &true)?;
    let pool_specs: PoolSpecs = POOL_SPECS.load(deps.storage)?;
    let sender = info.sender.clone();

    if let Err(e) = check_rate_limit(&mut deps, &env, &pool_specs, &sender) {
        RATE_LIMIT_GUARD.save(deps.storage, &false)?;
        return Err(e);
    }
    let result = execute_commit_logic(
        &mut deps,
        env,
        info,
        asset,
        amount,
        belief_price,
        max_spread,
    );

    RATE_LIMIT_GUARD.save(deps.storage, &false)?;

    result
}

pub fn execute_commit_logic(
    deps: &mut DepsMut,
    env: Env,
    info: MessageInfo,
    asset: Asset,
    amount: Uint128,
    //we arent using this yet, but this will be used to calculate slippage. You can find the function to "assert slippage" below.
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    //there will be a another parameter here as well called deadline - it basically just says if something weird happens and the transaction doesnt get processed quickly enough the sender would like it to fail.
    //deadline: Option<Timestamp>
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    let oracle_info = ORACLE_INFO.load(deps.storage)?;
    let fee_info = FEEINFO.load(deps.storage)?;
    let sender = info.sender.clone();
    let denom = match &pool_info.pair_info.asset_infos[0] {
        AssetInfo::NativeToken { denom } => denom.clone(),
        AssetInfo::Token { contract_addr } => {
            return Err(ContractError::Std(StdError::generic_err(
                "Expected native token, found CW20 token",
            )));
        }
    };
    // Validate asset type
    if !asset.info.equal(&pool_info.pair_info.asset_infos[0])
        && !asset.info.equal(&pool_info.pair_info.asset_infos[1])
    {
        return Err(ContractError::AssetMismatch {});
    }

    // Validate amount matches
    if amount != asset.amount {
        return Err(ContractError::MismatchAmount {});
    }

    if asset.amount.is_zero() {
        return Err(ContractError::ZeroAmount {});
    }

    let oracle_data = get_and_validate_oracle_price(
        &deps.querier,
        &oracle_info.oracle_addr,
        &oracle_info.oracle_symbol,
        env.block.time.seconds(),
    )?;

    //convert amount to usd for commit storage
    let usd_value = native_to_usd(oracle_data.price, asset.amount, oracle_data.expo)?;

    // Verify funds were actually sent
    let sent = info
        .funds
        .iter()
        .find(|c| c.denom == denom.as_str())
        .map(|c| c.amount)
        .unwrap_or_default();
    if sent < amount {
        return Err(ContractError::MismatchAmount {});
    }

    let mut messages: Vec<CosmosMsg> = Vec::new();

    // fees calculated right away
    let bluechip_fee_amt =
        amount * fee_info.bluechip_fee.numerator() / fee_info.bluechip_fee.denominator();
    let creator_fee_amt =
        amount * fee_info.creator_fee.numerator() / fee_info.creator_fee.denominator();

    // Create fee transfer messages
    let bluechip_transfer =
        get_bank_transfer_to_msg(&fee_info.bluechip_address, &denom, bluechip_fee_amt).map_err(
            |e| {
                ContractError::Std(StdError::generic_err(format!(
                    "Bluechip transfer failed: {}",
                    e
                )))
            },
        )?;

    let creator_transfer =
        get_bank_transfer_to_msg(&fee_info.creator_address, &denom, creator_fee_amt).map_err(
            |e| {
                ContractError::Std(StdError::generic_err(format!(
                    "Creator transfer failed: {}",
                    e
                )))
            },
        )?;

    messages.push(bluechip_transfer);
    messages.push(creator_transfer);

    // load state of threshold of the pool
    let threshold_already_hit = THRESHOLD_HIT.load(deps.storage)?;

    if !threshold_already_hit {
        return process_pre_threshold_commit(deps, env, sender, &asset, usd_value, messages);
    } else {
        //random error to satisfy the code now
        return Err(ContractError::AssetMismatch {});
    }
}

//commit transaction prior to threhold being crossed. commit to ledger and store values for return mint
fn process_pre_threshold_commit(
    deps: &mut DepsMut,
    env: Env,
    sender: Addr,
    asset: &Asset,
    usd_value: Uint128,
    messages: Vec<CosmosMsg>,
) -> Result<Response, ContractError> {
    let pool_info = POOL_INFO.load(deps.storage)?;
    //do not calculate fees in function, they are calculated prior.
    // Update commit ledger
    COMMIT_LEDGER.update::<_, ContractError>(deps.storage, &sender, |v| {
        Ok(v.unwrap_or_default() + usd_value)
    })?;

    // Update total USD raised
    let usd_total = USD_RAISED.update::<_, ContractError>(deps.storage, |r| Ok(r + usd_value))?;
    COMMITSTATUS.save(deps.storage, &usd_total)?;

    COMMIT_INFO.update(
        deps.storage,
        &sender,
        |maybe_commiting| -> Result<_, ContractError> {
            match maybe_commiting {
                Some(mut commiting) => {
                    commiting.total_paid_native += asset.amount;
                    commiting.total_paid_usd += usd_value;
                    commiting.last_payment_native = asset.amount;
                    commiting.last_payment_usd = usd_value;
                    commiting.last_commited = env.block.time;
                    Ok(commiting)
                }
                None => Ok(Commiting {
                    pool_id: pool_info.pool_id,
                    commiter: sender.clone(),
                    total_paid_native: asset.amount,
                    total_paid_usd: usd_value,
                    last_commited: env.block.time,
                    last_payment_native: asset.amount,
                    last_payment_usd: usd_value,
                }),
            }
        },
    )?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "commit")
        .add_attribute("phase", "funding")
        .add_attribute("committer", sender)
        .add_attribute("block_committed", env.block.time.to_string())
        .add_attribute("commit_amount_native", asset.amount.to_string())
        .add_attribute("commit_amount_usd", usd_value.to_string()))
}


fn check_rate_limit(
    deps: &mut DepsMut,
    env: &Env,
    pool_specs: &PoolSpecs,
    sender: &Addr,
) -> Result<(), ContractError> {
    if let Some(last_commit_time) = USER_LAST_COMMIT.may_load(deps.storage, sender)? {
        let time_since_last = env.block.time.seconds() - last_commit_time;

        if time_since_last < pool_specs.min_commit_interval {
            let wait_time = pool_specs.min_commit_interval - time_since_last;
            return Err(ContractError::TooFrequentCommits { wait_time });
        }
    }

    USER_LAST_COMMIT.save(deps.storage, sender, &env.block.time.seconds())?;

    Ok(())
}

fn native_to_usd(
    cached_price: Uint128,
    native_amount: Uint128,
    expo: i32, // micro-native
) -> StdResult<Uint128> {
    if expo != -8 {
        return Err(StdError::generic_err(format!(
            "Unexpected price exponent: {}. Expected: -8",
            expo
        )));
    }
    // 2. convert: (µnative × price) / 10^(8-6) = µUSD
    let usd_micro_u256 = (Uint256::from(native_amount) * Uint256::from(cached_price))
        / Uint256::from(100_000_000u128); // 10^(8-6) = 100

    let usd_micro = Uint128::try_from(usd_micro_u256)?;
    Ok(usd_micro)
}

pub fn get_and_validate_oracle_price(
    querier: &QuerierWrapper,
    oracle_addr: &Addr,
    symbol: &str,
    current_time: u64,
) -> StdResult<OracleData> {
    let resp: PriceResponse = querier
        .query_wasm_smart(
            oracle_addr.clone(),
            &PythQueryMsg::GetPrice {
                price_id: symbol.into(),
            },
        )
        .map_err(|e| StdError::generic_err(format!("Oracle query failed: {}", e)))?;

    // Staleness check
    let zero: Uint128 = Uint128::zero();
    if resp.price <= zero {
        return Err(StdError::generic_err(
            "Invalid zero or negative price from oracle",
        ));
    }

    if current_time.saturating_sub(resp.publish_time) > MAX_ORACLE_AGE {
        return Err(StdError::generic_err("Oracle price too stale"));
    }
    Ok(OracleData {
        price: resp.price,
        expo: resp.expo,
    })
}

//this is used to enforce the deadline i commented in earlier.
fn enforce_deadline(current: Timestamp, deadline: Option<Timestamp>) -> Result<(), ContractError> {
    if let Some(dl) = deadline {
        if current > dl {
            return Err(ContractError::TransactionExpired {});
        }
    }
    Ok(())
}
//this is used for pool creation. Make sure the factory address used to create the pool was the correct one.
pub fn validate_factory_address(
    stored_factory_addr: &Addr,
    candidate_factory_addr: &Addr,
) -> Result<(), ContractError> {
    if stored_factory_addr != candidate_factory_addr {
        return Err(ContractError::InvalidFactory {});
    }
    Ok(())
}

//Here is where the user dictates how much spread they are comfortable with in their price. 
//pre threshold commits DO NOT NEED THIS because they are not making swaps. they are just commiting to a ledger
//for a predetermined ratio of the payout that happens after the threshold is crossed
pub fn assert_max_spread(
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    offer_amount: Uint128,
    return_amount: Uint128,
    spread_amount: Uint128,
) -> Result<(), ContractError> {
    let default_spread = Decimal::from_str(DEFAULT_SLIPPAGE)?;
    let max_allowed_spread = Decimal::from_str(MAX_ALLOWED_SLIPPAGE)?;

    let max_spread = max_spread.unwrap_or(default_spread);
    if belief_price == Some(Decimal::zero()) {
        return Err(ContractError::InvalidBeliefPrice {});
    }
    if max_spread.gt(&max_allowed_spread) {
        return Err(ContractError::AllowedSpreadAssertion {});
    }

    if let Some(belief_price) = belief_price {
        let expected_return = offer_amount * belief_price.inv().unwrap().numerator()
            / belief_price.inv().unwrap().denominator();
        let spread_amount = expected_return
            .checked_sub(return_amount)
            .unwrap_or_else(|_| Uint128::zero());

        if return_amount < expected_return
            && Decimal::from_ratio(spread_amount, expected_return) > max_spread
        {
            return Err(ContractError::MaxSpreadAssertion {});
        }
    } else if Decimal::from_ratio(spread_amount, return_amount + spread_amount) > max_spread {
        return Err(ContractError::MaxSpreadAssertion {});
    }

    Ok(())
}

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

//since sending fees is a secondary transaction, we need to wrap it in a the wasm BankMsg Send
pub fn get_bank_transfer_to_msg(
    recipient: &Addr,
    denom: &str,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_bank_msg = cosmwasm_std::BankMsg::Send {
        to_address: recipient.into(),
        amount: vec![Coin {
            denom: denom.to_string(),
            amount,
        }],
    };
    let transfer_bank_cosmos_msg: CosmosMsg = transfer_bank_msg.into();
    Ok(transfer_bank_cosmos_msg)
}

