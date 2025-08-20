#![allow(non_snake_case)]
use crate::asset::{PairType};
use crate::error::ContractError;
use crate::msg::{
  PoolInstantiateMsg,
};


use crate::state::{
    CommitInfo, ExpectedFactory, OracleInfo, PairInfo, PoolFeeState, PoolInfo, PoolSpecs,
    ThresholdPayout, COMMITSTATUS, COMMIT_CONFIG,  EXPECTED_FACTORY, FEEINFO,
     NATIVE_RAISED, ORACLE_INFO, POOL_FEE_STATE, POOL_INFO, POOL_SPECS, POOL_STATE,
     THRESHOLD_HIT, THRESHOLD_PAYOUT,  USD_RAISED,
    
};
use crate::state::{
    PoolState, Position, LIQUIDITY_POSITIONS,
    NEXT_POSITION_ID,
};
use cosmwasm_std::{
    entry_point, from_json, Addr, Decimal,
    DepsMut,Env, MessageInfo,
    Response, Uint128,
};
use cw2::{set_contract_version};
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

pub fn validate_factory_address(
    stored_factory_addr: &Addr,
    candidate_factory_addr: &Addr,
) -> Result<(), ContractError> {
    if stored_factory_addr != candidate_factory_addr {
        return Err(ContractError::InvalidFactory {});
    }
    Ok(())
}

