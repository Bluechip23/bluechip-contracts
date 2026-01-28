use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Uint128};
pub use pool_factory_interfaces::ExpandEconomyMsg;

#[cw_serde]
pub struct InstantiateMsg {
    pub factory_address: String,
    pub owner: Option<String>,
}

#[cw_serde]
pub enum ExecuteMsg {
    ExpandEconomy(ExpandEconomyMsg),

    UpdateConfig {
        factory_address: Option<String>,
        owner: Option<String>,
    },

    Withdraw {
        amount: Uint128,
        denom: String,
        recipient: Option<String>,
    },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(ConfigResponse)]
    GetConfig {},

    #[returns(cosmwasm_std::Coin)]
    GetBalance { denom: String },
}

#[cw_serde]
pub struct ConfigResponse {
    pub factory_address: Addr,
    pub owner: Addr,
}
