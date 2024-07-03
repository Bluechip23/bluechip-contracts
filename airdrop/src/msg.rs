use cosmwasm_std::{Addr, Uint128};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// use crate::state::Recipient;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct InstantiateMsg {
    pub total_whitelist_wallets: Uint128,
    pub eligible_wallets: Uint128,
    pub airdrop_amount: Uint128,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    // SetRewards { recipients: Vec<Recipient> },
    ImportWhitelist { whitelist: Vec<Addr> },
    Claim {},
    Start {},
}
