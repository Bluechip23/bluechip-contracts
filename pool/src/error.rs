//! Re-export of the shared `ContractError` type from `pool-core`.
//!
//! The full enum definition lives in `pool_core::error::ContractError` so
//! it can be shared between this crate (creator-pool) and `standard-pool`.
//! Existing `use crate::error::ContractError;` imports continue to work
//! unchanged thanks to the glob re-export below.
pub use pool_core::error::*;
