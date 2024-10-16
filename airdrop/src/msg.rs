use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Uint128};

// use crate::state::Recipient;

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
