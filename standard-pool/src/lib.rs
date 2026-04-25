//! Bluechip Standard Pool — plain xyk pool around two pre-existing
//! assets (any combination of `Native` and `CreatorToken`). No commit
//! phase, no threshold, no distribution; immediately tradeable and
//! depositable at creation.
//!
//! The vast majority of this crate's logic lives in `pool_core`. The
//! modules below are thin entry-point wrappers: they define the
//! `#[entry_point]` exports and route each `ExecuteMsg` / `QueryMsg`
//! variant to the corresponding `pool_core::*` handler.
//!
//! STATUS: skeleton (4b-i). Module bodies are placeholders pending
//! 4b-ii (msg.rs definitions + instantiate/execute/query/migrate
//! dispatch).

pub mod contract;
pub mod error;
pub mod msg;
pub mod query;

#[cfg(test)]
mod testing;
