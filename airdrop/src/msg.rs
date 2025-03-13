use crate::state::State;
use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Uint128};

#[cw_serde]
pub struct InstantiateMsg {
    pub total_whitelist_wallets: Uint128,
    pub eligible_wallets: Uint128,
    pub airdrop_amount: Uint128,
}       

#[cw_serde]
pub enum ExecuteMsg {
    // SetRewards { recipients: Vec<Recipient> },
    ImportWhitelist { whitelist: Vec<Addr> },
    Claim {},
    Start {},
}

#[cw_serde]
pub enum QueryMsg {
    Config {},
    IsWhitelisted { address: Addr },
    IsClaimed { address: Addr },
}

#[cw_serde]
pub struct ConfigResponse {
    pub config: State,
}

#[cw_serde]
pub struct StatusResponse {
    pub status: bool,
}
