//! Re-export of the shared `ContractError` type from `pool-core`.
//!
//! Every existing `use crate::error::ContractError;` resolves through
//! this glob re-export unchanged. Creator-pool uses the identical
//! pattern; both contracts produce the same error type on the wire so
//! client-side error handling works uniformly across both pool kinds.
//!
//! Note: the full `ContractError` enum has commit-phase variants
//! (`ShortOfThreshold`, `TooFrequentCommits`, `InvalidThresholdParams`,
//! etc.) that this crate never constructs. Rust does not warn on
//! un-constructed enum variants of public types, so sharing the type
//! costs nothing.
pub use pool_core::error::*;
