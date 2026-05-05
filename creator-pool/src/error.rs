//! Re-export of the shared `ContractError` type from `pool-core`.
//!
//! The full enum definition lives in `pool_core::error::ContractError` so
//! it can be shared between this crate (creator-pool) and `standard-pool`.
//! Explicit re-export (not glob) pins this crate's `error` surface to a
//! single named symbol so a future change to `pool_core::error` that
//! removes `ContractError` becomes a build error here rather than
//! silently shrinking the crate's API.
pub use pool_core::error::ContractError;
