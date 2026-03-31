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

    // F2-H1: Config updates now follow a 48-hour timelock to prevent a
    // compromised owner key from instantly redirecting factory_address
    // and draining funds via RequestExpansion.
    ProposeConfigUpdate {
        factory_address: Option<String>,
        owner: Option<String>,
    },
    ExecuteConfigUpdate {},
    CancelConfigUpdate {},

    // 48hr timelock
    ProposeWithdrawal {
        amount: Uint128,
        denom: String,
        recipient: Option<String>,
    },

    ExecuteWithdrawal {},

    CancelWithdrawal {},
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
