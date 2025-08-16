use crate::msg::{ExecuteMsg, InstantiateMsg, PriceResponse, PythQueryMsg};
use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError,
    StdResult, Uint128,
};
use cw_storage_plus::Map;

pub const PRICES: Map<&str, PriceResponse> = Map::new("prices");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    _deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    _msg: InstantiateMsg,
) -> StdResult<Response> {
    Ok(Response::default())
}

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

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: PythQueryMsg) -> StdResult<Binary> {
    match msg {
        PythQueryMsg::GetPrice { price_id } => {
            let stored_price = PRICES
                .may_load(deps.storage, &price_id)?
                .ok_or_else(|| StdError::generic_err("Symbol not found"))?;
            to_json_binary(&stored_price)
        }
    }
}
