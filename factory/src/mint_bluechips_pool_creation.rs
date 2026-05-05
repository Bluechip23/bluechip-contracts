use crate::{
    error::ContractError,
    state::{FACTORYINSTANTIATEINFO, FIRST_THRESHOLD_TIMESTAMP, POOLS_BY_ID},
};
use cosmwasm_std::{BankMsg, Coin, CosmosMsg, DepsMut, Env, StdError, StdResult, Uint128};

/// Saturating cap on the mint-decay polynomial input (MEDIUM-4 audit
/// fix). Two concentric bounds are at play:
///
///   1. **Polynomial-is-zero bound (≈ 33,300):** `(5x²+x) / (s/6 + 333x)`
///      crosses the 500e6 base around `x ≈ 33,300` at s == 0, so the
///      polynomial output is structurally zero for any larger ordinal.
///      Any cap above this is purely conservative.
///
///   2. **u128 overflow bound (≈ 8 × 10^18):** the inner `5 * x * x`
///      overflows u128 around `x ≈ sqrt(2^128 / 5) ≈ 8 × 10^18`.
///
/// `MAX_DECAY_X = 1_000_000_000` sits comfortably between the two —
/// well above any realistic ordinal an honest deployment will ever
/// see, and well below the overflow ceiling. The cap exists purely
/// as defense-in-depth against a buggy migration or storage
/// corruption that injects an absurd ordinal directly into a
/// `PoolDetails.commit_pool_ordinal`; under normal operation the
/// per-address 1h create cooldown bounds the ordinal far below this.
const MAX_DECAY_X: u128 = 1_000_000_000;

pub fn calculate_mint_amount(seconds_elapsed: u64, pools_created: u64) -> StdResult<Uint128> {
    // Formula (with `x = pools_created`, `s = seconds_elapsed`):
    //   500 - ((5x^2 + x) / ((s/6) + 333x))
    let pools_created = pools_created as u128;
    let seconds_elapsed = seconds_elapsed as u128;

    // Defense-in-depth saturating cap. For pools_created > MAX_DECAY_X the
    // polynomial output is zero by definition; short-circuit rather
    // than risk overflow in `5 * pools_created * pools_created`.
    if pools_created > MAX_DECAY_X {
        return Ok(Uint128::zero());
    }

    let five_x_squared = 5u128
        .checked_mul(pools_created)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator"))?
        .checked_mul(pools_created)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator"))?;

    let numerator = five_x_squared
        .checked_add(pools_created)
        .ok_or_else(|| StdError::generic_err("Overflow in numerator addition"))?;
    let s_div_6 = seconds_elapsed / 6;
    let denominator = s_div_6
        .checked_add(
            333u128
                .checked_mul(pools_created)
                .ok_or_else(|| StdError::generic_err("Overflow in denominator"))?,
        )
        .ok_or_else(|| StdError::generic_err("Overflow in denominator"))?;

    if denominator == 0 {
        return Ok(Uint128::new(500_000_000));
    }
    let scaled_numerator = numerator
        .checked_mul(1_000_000)
        .ok_or_else(|| StdError::generic_err("Overflow in scaled numerator"))?;

    let division_result = scaled_numerator / denominator;

    let base_amount = 500_000_000u128;

    if division_result >= base_amount {
        return Ok(Uint128::zero());
    }

    Ok(Uint128::new(base_amount - division_result))
}

/// Calculates and mints bluechip tokens when a pool crosses its commit threshold.
/// `pool_id` is the sequential ID of the pool — the decay formula uses this as `x`
/// so that later pools receive fewer minted tokens.
///
/// STANDARD POOLS DO NOT MINT.
/// Only commit pools cross a threshold; standard pools wrap pre-existing
/// assets and have no commit-phase concept. The single call site,
/// `execute_notify_threshold_crossed`, already rejects
/// `PoolKind::Standard`, so no `Standard` pool can reach this function
/// in the current code base. The defensive guard below is
/// belt-and-braces: any future call site that forgets the upstream
/// check still cannot trigger a bluechip mint on behalf of a standard
/// pool. Keeping standard pools out of the mint formula is a hard
/// invariant — a standard pool inflating `x` (or worse, getting a
/// `mint_amount > 0`) would dilute every legitimate commit pool's
/// mint reward and let permissionless pool creation drain the
/// expand-economy budget.
pub fn calculate_and_mint_bluechip(
    deps: &mut DepsMut,
    env: Env,
    pool_id: u64,
) -> Result<Vec<CosmosMsg>, ContractError> {
    // Defense-in-depth: hard guard that this function is only
    // ever called for commit pools. Belongs above the mock-feature
    // short-circuit so even mock builds enforce the invariant.
    let pool_details = POOLS_BY_ID.load(deps.storage, pool_id)?;
    if pool_details.pool_kind == pool_factory_interfaces::PoolKind::Standard {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Pool {} is a standard pool — standard pools do not participate \
             in the expand-economy mint formula. The caller should never \
             reach calculate_and_mint_bluechip for a standard pool.",
            pool_id
        ))));
    }

    // Lazy-init the "first threshold crossed" anchor timestamp. The
    // decay formula's `s` input is `block.time - first_threshold_time`
    // so `s == 0` for the pool that triggers this branch for the very
    // first time. Subsequent pools see a growing `s`, which shrinks
    // the mint amount per the polynomial below.
    let _first_threshold_time = match FIRST_THRESHOLD_TIMESTAMP.may_load(deps.storage)? {
        Some(time) => time,
        None => {
            FIRST_THRESHOLD_TIMESTAMP.save(deps.storage, &env.block.time)?;
            env.block.time
        }
    };

    #[cfg(feature = "mock")]
    {
        return Ok(Vec::new());
    }

    #[cfg(not(feature = "mock"))]
    {
    let first_threshold_time = _first_threshold_time;
    let mut msgs: Vec<CosmosMsg> = Vec::new();
    let config = FACTORYINSTANTIATEINFO.load(deps.storage)?;
    let seconds_elapsed = env
        .block
        .time
        .seconds()
        .saturating_sub(first_threshold_time.seconds());

    // Use the commit-pool-only ordinal in the decay polynomial so that
    // permissionless standard-pool creations (which also bump the global
    // POOL_COUNTER) cannot inflate `x` and shrink legitimate commit pools'
    // mint reward toward zero. Legacy commit pools written before
    // `commit_pool_ordinal` existed have it default to zero on
    // deserialize; for those, fall back to `pool_id` to preserve the
    // exact mint amount they would have produced under the old code.
    // (`pool_details` is reused from the standard-pool guard above.)
    let decay_x = if pool_details.commit_pool_ordinal == 0 {
        pool_id
    } else {
        pool_details.commit_pool_ordinal
    };
    let mint_amount = calculate_mint_amount(seconds_elapsed, decay_x)?;

    if !mint_amount.is_zero() {
        if let Some(expand_economy_contract) = config.bluechip_mint_contract_address {
            msgs.push(CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
                contract_addr: expand_economy_contract.to_string(),
                msg: cosmwasm_std::to_json_binary(
                    &pool_factory_interfaces::ExpandEconomyExecuteMsg::ExpandEconomy(
                        pool_factory_interfaces::ExpandEconomyMsg::RequestExpansion {
                            recipient: config.bluechip_wallet_address.to_string(),
                            amount: mint_amount,
                        },
                    ),
                )?,
                funds: vec![],
            }));
        } else {
            // Read the canonical bluechip denom from factory config rather
            // than hardcoding "ubluechip" — the field is documented as the
            // chain bank denom and a deployment on a chain using a
            // different denom (e.g. an IBC-wrapped variant) would have
            // failed the bank send here.
            msgs.push(CosmosMsg::Bank(BankMsg::Send {
                to_address: config.bluechip_wallet_address.to_string(),
                amount: vec![Coin {
                    denom: config.bluechip_denom.clone(),
                    amount: mint_amount,
                }],
            }));
        }
    }

    Ok(msgs)
    }
}
