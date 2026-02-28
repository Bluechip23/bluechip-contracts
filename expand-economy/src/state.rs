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

// 48-hours
pub const WITHDRAW_TIMELOCK_SECONDS: u64 = 172_800;
pub const PENDING_WITHDRAWAL: Item<PendingWithdrawal> = Item::new("pending_withdrawal");
