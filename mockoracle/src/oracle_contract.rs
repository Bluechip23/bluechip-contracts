// ============================================================================
// MOCK ORACLE — FOR TESTING ONLY
//
// This contract has NO access control on SetPrice. Anyone can set any price.
// Entry points are gated behind the "testing" feature flag to prevent
// accidental deployment to production.
//
// To build for local testing:
//   cargo build -p oracle --features testing
// ============================================================================

use crate::msg::PriceResponse;
#[cfg(feature = "testing")]
use crate::msg::{ExecuteMsg, InstantiateMsg, PythQueryMsg, PriceFeedResponse, PriceFeed, PythPriceRetrievalResponse};
#[cfg(feature = "testing")]
use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128,
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
            let new_price = PriceResponse {
                price,
                publish_time: env.block.time.seconds(), // current block timestamp
                expo: -8,                                // example default
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
        PythQueryMsg::PythConversionPriceFeed { id } => {
            let stored_price = PRICES
                .may_load(deps.storage, &id)?
                .ok_or_else(|| StdError::generic_err("Price feed not found"))?;
            
            let current_time = env.block.time.seconds() as i64;

            let response = PriceFeedResponse {
                price_feed: Some(PriceFeed {
                    id,
                    price: PythPriceRetrievalResponse {
                        price: stored_price.price.u128() as i64,
                        conf: stored_price.conf.u128() as u64,
                        expo: stored_price.expo,
                        publish_time: current_time,
                    },
                    ema_price: PythPriceRetrievalResponse {
                        price: stored_price.price.u128() as i64,
                        conf: stored_price.conf.u128() as u64,
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
