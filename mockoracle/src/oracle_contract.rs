use cosmwasm_std::{Binary, to_json_binary, entry_point, MessageInfo, Deps, Env, StdError, DepsMut, StdResult, Response, Uint128};
use crate::msg::{PythQueryMsg, PriceResponse, ExecuteMsg, InstantiateMsg};
use cw_storage_plus::Map;

pub const PRICES: Map<String, Uint128> = Map::new("prices");

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
    _env: Env,
    _info: MessageInfo,
    msg: ExecuteMsg,
) -> StdResult<Response> {
    match msg {
        ExecuteMsg::SetPrice { price_id, price } => {
            PRICES.save(deps.storage, price_id, &price)?;
            Ok(Response::new().add_attribute("action", "set_price"))
        }
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: PythQueryMsg) -> StdResult<Binary> {
    match msg {
        PythQueryMsg::GetPrice { price_id } => {
            let price = PRICES
                .may_load(deps.storage, price_id.clone())?
                .ok_or_else(|| StdError::generic_err("Symbol not found"))?;
            to_json_binary(&PriceResponse { price })
        }
    }
}