use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Timestamp, Uint128};
use cw_storage_plus::Item;

#[cw_serde]
pub struct Config {
    pub factory_address: Addr,
    pub owner: Addr,
}

pub const CONFIG: Item<Config> = Item::new("config");

#[cw_serde]
pub struct PendingWithdrawal {
    pub amount: Uint128,
    pub denom: String,
    pub recipient: String,
    pub execute_after: Timestamp,
}

pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 172_800;
pub const CONFIG_TIMELOCK_SECONDS: u64 = 172_800;
pub const PENDING_WITHDRAWAL: Item<PendingWithdrawal> = Item::new("pending_withdrawal");

#[cw_serde]
pub struct PendingConfigUpdate {
    pub factory_address: Option<String>,
    pub owner: Option<String>,
    pub effective_after: Timestamp,
}
pub const PENDING_CONFIG_UPDATE: Item<PendingConfigUpdate> = Item::new("pending_config_update");
