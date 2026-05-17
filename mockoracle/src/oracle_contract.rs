// ============================================================================
// MOCK ORACLE — FOR TESTING ONLY
//
// This contract has NO access control on SetPrice. Anyone can set any price.
// Entry points are gated behind the "testing" feature flag to prevent
// accidental deployment to production.
//
// To build for local testing:
//   cargo build -p oracle --features testing
//
// IMPORTANT — staleness checks against this mock are dead code:
//   Both `GetPrice` and `PriceFeed` overwrite `publish_time` to
//   the current block time on every query. This means the factory's
//   `MAX_PRICE_AGE_SECONDS_BEFORE_STALE` gate
//   (factory/src/internal_bluechip_price_oracle.rs::query_pyth_atom_usd_price)
//   will ALWAYS see a fresh timestamp when reading from this mock. Tests
//   that need to exercise the staleness fallback path must do so against
//   the production code path or against a different test double.
// ============================================================================

use crate::msg::PriceResponse;
#[cfg(feature = "testing")]
use crate::msg::{
    ExecuteMsg, InstantiateMsg, PriceFeed, PriceFeedResponse, PythPriceRetrievalResponse,
    PythQueryMsg,
};
#[cfg(feature = "testing")]
use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, Int64, MessageInfo, Response,
    StdError, StdResult, Uint128, Uint64,
};

use cw_storage_plus::Map;

pub const PRICES: Map<&str, PriceResponse> = Map::new("prices");

#[cfg(feature = "testing")]
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    _deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    _msg: InstantiateMsg,
) -> StdResult<Response> {
    Ok(Response::default())
}

#[cfg(feature = "testing")]
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: ExecuteMsg,
) -> StdResult<Response> {
    match msg {
        ExecuteMsg::SetPrice { price_id, price } => {
            // Reject zero up front so the failure surfaces here rather than
            // downstream as a div-by-zero in the factory's USD<->bluechip
            // conversion. The factory does its own zero-check, but failing
            // at the source makes test setup mistakes obvious.
            if price.is_zero() {
                return Err(StdError::generic_err("price must be > 0"));
            }
            let new_price = PriceResponse {
                price,
                publish_time: env.block.time.seconds(), // current block timestamp
                expo: -8,                               // example default
                conf: Uint128::zero(),                  // example default
            };
            PRICES.save(deps.storage, &price_id, &new_price)?;
            Ok(Response::new().add_attribute("action", "set_price"))
        }
    }
}

#[cfg(feature = "testing")]
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: PythQueryMsg) -> StdResult<Binary> {
    match msg {
        PythQueryMsg::GetPrice { price_id } => {
            let mut stored_price = PRICES
                .may_load(deps.storage, &price_id)?
                .ok_or_else(|| StdError::generic_err("Symbol not found"))?;

            stored_price.publish_time = env.block.time.seconds();
            to_json_binary(&stored_price)
        }
        PythQueryMsg::PriceFeed { id } => {
            let stored_price = PRICES
                .may_load(deps.storage, &id)?
                .ok_or_else(|| StdError::generic_err("Price feed not found"))?;

            let current_time = i64::try_from(env.block.time.seconds()).map_err(|_| {
                StdError::generic_err("block time exceeds i64::MAX (impossible in practice)")
            })?;

            // Pyth's wire format is i64 for price and u64 for conf — but our
            // mock stores both as Uint128, so we need a checked narrow.
            // Surface a clear error instead of silently wrapping into a
            // negative price (which downstream Pyth-style validators would
            // reject with a confusing message).
            let price_i64 = i64::try_from(stored_price.price.u128()).map_err(|_| {
                StdError::generic_err(format!(
                    "stored price {} exceeds i64::MAX; mock rejects values that wouldn't fit a real Pyth feed",
                    stored_price.price
                ))
            })?;
            let conf_u64 = u64::try_from(stored_price.conf.u128()).map_err(|_| {
                StdError::generic_err(format!(
                    "stored conf {} exceeds u64::MAX",
                    stored_price.conf
                ))
            })?;

            let response = PriceFeedResponse {
                price_feed: Some(PriceFeed {
                    id,
                    price: PythPriceRetrievalResponse {
                        price: Int64::new(price_i64),
                        conf: Uint64::new(conf_u64),
                        expo: stored_price.expo,
                        publish_time: current_time,
                    },
                    ema_price: PythPriceRetrievalResponse {
                        price: Int64::new(price_i64),
                        conf: Uint64::new(conf_u64),
                        expo: stored_price.expo,
                        publish_time: current_time,
                    },
                }),
                price: None,
            };
            to_json_binary(&response)
        }
    }
}
