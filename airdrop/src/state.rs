use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use cosmwasm_std::{Addr, Uint128};
use cw_storage_plus::{Item, Map};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct Recipient {
    pub address: String,
    pub amount: Uint128,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct State {
    pub owner: Addr,
    pub total_whitelist_wallets: Uint128,
    pub eligible_wallets: Uint128,
    pub imported_wallets: Uint128,
    pub claimed_wallets: Uint128,
    pub airdrop_amount: Uint128,
    pub is_opened: bool,
}

pub const STATE: Item<State> = Item::new("state");
pub const REWARDS: Map<&Addr, Uint128> = Map::new("rewards");
pub const WHITELISTED: Map<&Addr, bool> = Map::new("whitelisted");
pub const CLAIMED: Map<&Addr, bool> = Map::new("claimed");
