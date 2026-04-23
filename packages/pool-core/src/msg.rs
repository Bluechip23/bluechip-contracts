//! Shared wire-format types used by both creator-pool and standard-pool.
//!
//! Populated in two phases:
//!   - Step 2a (current) — `CommitFeeInfo` only, because it's the value
//!     type of the shared `COMMITFEEINFO` storage Item and therefore has
//!     to be importable from `pool_core::state`.
//!   - Step 2d — the rest of the shared wire types (response structs,
//!     `Cw20HookMsg`, `PoolConfigUpdate`, `CommitStatus`, etc.).
//!
//! The creator-pool crate re-exports this module via a glob so existing
//! `use crate::msg::CommitFeeInfo;` call sites continue to resolve.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal};

#[cw_serde]
pub struct CommitFeeInfo {
    pub bluechip_wallet_address: Addr,
    pub creator_wallet_address: Addr,
    pub commit_fee_bluechip: Decimal,
    pub commit_fee_creator: Decimal,
}
