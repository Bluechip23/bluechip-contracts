//! Tiny "factory" used only by the fuzz harness.
//!
//! Why a shim? The production factory has a multi-thousand-line oracle
//! bootstrap path (Pyth pull, anchor pool, TWAP, internal cache, 48h
//! timelocks) that we'd have to fully bring up before any pool could
//! make a single `ConvertBluechipToUsd` query. The shim implements
//! exactly the surface area the pools call back into:
//!
//!   * `FactoryQueryMsg::ConvertBluechipToUsd { amount }` — answered
//!     using a stored `rate` (USD per 1 bluechip, in 6-decimal USD).
//!   * `FactoryQueryMsg::ConvertUsdToBluechip { amount }` — inverse.
//!   * `FactoryQueryMsg::GetBluechipUsdPrice {}` — returns the rate.
//!   * `FactoryExecuteMsg::NotifyThresholdCrossed { pool_id }` — records
//!     a "minted" flag per pool_id so the harness can assert the
//!     idempotency invariant.
//!   * `FactoryExecuteMsg::PayDistributionBounty { recipient }` — no-op
//!     bank send (we never run distribution in the harness).
//!
//! Plus harness-only ops:
//!   * `SetRate { new_rate, timestamp }` — admin sets the oracle rate.
//!   * `RegisterPool { pool_id, addr }` — record a known pool so
//!     callbacks can be authenticated.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    entry_point, to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult, Uint128,
};
use cw_storage_plus::{Item, Map};
use pool_factory_interfaces::{
    BluechipPriceResponse, ConversionResponse, FactoryExecuteMsg, FactoryQueryMsg,
};

#[cw_serde]
pub struct InstantiateMsg {
    pub admin: Addr,
    pub initial_rate: Uint128,
    pub bluechip_denom: String,
}

#[cw_serde]
pub enum HarnessExecuteMsg {
    /// Admin: set the oracle rate. `new_rate` is USD-per-bluechip in 6-dec
    /// fixed point. `timestamp` is the synthetic publish_time we want the
    /// pool to see; 0 means "now".
    SetRate {
        new_rate: Uint128,
        timestamp: u64,
    },
    /// Admin: associate a pool address with a pool_id so threshold
    /// notifications can be authenticated.
    RegisterPool {
        pool_id: u64,
        addr: Addr,
    },
    /// Pool callback (production interface).
    Factory(FactoryExecuteMsg),
}

#[cw_serde]
pub enum HarnessQueryMsg {
    Factory(FactoryQueryMsg),
    /// The wire-format the production pools actually send (see
    /// `creator_pool::swap_helper::FactoryQueryWrapper`). The pool
    /// wraps every oracle query under this variant — so we accept
    /// it here and dispatch to the inner FactoryQueryMsg.
    InternalBlueChipOracleQuery(FactoryQueryMsg),
    /// Has pool_id ever notified threshold crossed? Used by invariants.
    ThresholdMinted { pool_id: u64 },
    /// Total mint notifications received (for monotonicity invariants).
    NotifyCount {},
    /// Currently configured rate.
    Rate {},
}

#[cw_serde]
pub struct RateInfo {
    pub rate: Uint128,
    pub timestamp: u64,
}

const ADMIN: Item<Addr> = Item::new("admin");
const RATE: Item<RateInfo> = Item::new("rate");
const DENOM: Item<String> = Item::new("denom");
const POOL_ADDR_BY_ID: Map<u64, Addr> = Map::new("pool_id");
const POOL_ID_BY_ADDR: Map<&Addr, u64> = Map::new("pool_addr");
const MINTED: Map<u64, bool> = Map::new("minted");
const NOTIFY_COUNT: Item<u64> = Item::new("notifies");

/// USD precision used by the production factory (6 decimals).
pub const PRECISION: u128 = 1_000_000;

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> StdResult<Response> {
    ADMIN.save(deps.storage, &msg.admin)?;
    RATE.save(
        deps.storage,
        &RateInfo {
            rate: msg.initial_rate,
            timestamp: env.block.time.seconds(),
        },
    )?;
    DENOM.save(deps.storage, &msg.bluechip_denom)?;
    NOTIFY_COUNT.save(deps.storage, &0u64)?;
    Ok(Response::new())
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: HarnessExecuteMsg,
) -> StdResult<Response> {
    match msg {
        HarnessExecuteMsg::SetRate { new_rate, timestamp } => {
            ensure_admin(deps.as_ref(), &info.sender)?;
            if new_rate.is_zero() {
                return Err(StdError::generic_err("rate must be > 0"));
            }
            let ts = if timestamp == 0 {
                env.block.time.seconds()
            } else {
                timestamp
            };
            RATE.save(deps.storage, &RateInfo { rate: new_rate, timestamp: ts })?;
            Ok(Response::new().add_attribute("action", "set_rate"))
        }
        HarnessExecuteMsg::RegisterPool { pool_id, addr } => {
            ensure_admin(deps.as_ref(), &info.sender)?;
            POOL_ADDR_BY_ID.save(deps.storage, pool_id, &addr)?;
            POOL_ID_BY_ADDR.save(deps.storage, &addr, &pool_id)?;
            Ok(Response::new().add_attribute("action", "register_pool"))
        }
        HarnessExecuteMsg::Factory(FactoryExecuteMsg::NotifyThresholdCrossed { pool_id }) => {
            // Authenticate caller is the registered pool.
            let registered = POOL_ADDR_BY_ID.may_load(deps.storage, pool_id)?;
            match registered {
                Some(addr) if addr == info.sender => {}
                _ => return Err(StdError::generic_err("unauthorized notify")),
            }
            // Idempotency: production factory's POOL_THRESHOLD_MINTED guard.
            if MINTED.may_load(deps.storage, pool_id)?.unwrap_or(false) {
                return Err(StdError::generic_err("already minted"));
            }
            MINTED.save(deps.storage, pool_id, &true)?;
            let n = NOTIFY_COUNT.load(deps.storage)?;
            NOTIFY_COUNT.save(deps.storage, &(n + 1))?;
            Ok(Response::new().add_attribute("action", "threshold_crossed"))
        }
        HarnessExecuteMsg::Factory(FactoryExecuteMsg::PayDistributionBounty { recipient: _ }) => {
            // Authenticate caller is a registered pool.
            let _ = POOL_ID_BY_ADDR
                .may_load(deps.storage, &info.sender)?
                .ok_or_else(|| StdError::generic_err("unregistered pool"))?;
            // Real factory pays bounty; harness no-ops to avoid coupling
            // to bluechip balance accounting.
            Ok(Response::new().add_attribute("action", "pay_bounty"))
        }
    }
}

#[entry_point]
pub fn query(deps: Deps, env: Env, msg: HarnessQueryMsg) -> StdResult<Binary> {
    match msg {
        HarnessQueryMsg::InternalBlueChipOracleQuery(q) => {
            query(deps, env, HarnessQueryMsg::Factory(q))
        }
        HarnessQueryMsg::Factory(q) => match q {
            FactoryQueryMsg::ConvertBluechipToUsd { amount } => {
                let r = RATE.load(deps.storage)?;
                // USD = bluechip * rate / PRECISION
                let usd = amount
                    .checked_mul(r.rate)?
                    .checked_div(Uint128::new(PRECISION))
                    .map_err(|_| StdError::generic_err("div by zero"))?;
                to_json_binary(&ConversionResponse {
                    amount: usd,
                    rate_used: r.rate,
                    timestamp: r.timestamp,
                })
            }
            FactoryQueryMsg::ConvertUsdToBluechip { amount } => {
                let r = RATE.load(deps.storage)?;
                if r.rate.is_zero() {
                    return Err(StdError::generic_err("rate is zero"));
                }
                let bluechip = amount
                    .checked_mul(Uint128::new(PRECISION))?
                    .checked_div(r.rate)
                    .map_err(|_| StdError::generic_err("div by zero"))?;
                to_json_binary(&ConversionResponse {
                    amount: bluechip,
                    rate_used: r.rate,
                    timestamp: r.timestamp,
                })
            }
            FactoryQueryMsg::GetBluechipUsdPrice {} => {
                let r = RATE.load(deps.storage)?;
                to_json_binary(&BluechipPriceResponse {
                    price: r.rate,
                    timestamp: r.timestamp,
                    is_cached: false,
                })
            }
        },
        HarnessQueryMsg::ThresholdMinted { pool_id } => {
            let v = MINTED.may_load(deps.storage, pool_id)?.unwrap_or(false);
            to_json_binary(&v)
        }
        HarnessQueryMsg::NotifyCount {} => {
            let n = NOTIFY_COUNT.load(deps.storage)?;
            to_json_binary(&n)
        }
        HarnessQueryMsg::Rate {} => {
            let r = RATE.load(deps.storage)?;
            to_json_binary(&r)
        }
    }
}

fn ensure_admin(deps: Deps, sender: &Addr) -> StdResult<()> {
    let admin = ADMIN.load(deps.storage)?;
    if admin != *sender {
        return Err(StdError::generic_err("unauthorized"));
    }
    Ok(())
}
