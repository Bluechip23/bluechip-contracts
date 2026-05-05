//! Pool lifecycle: creation (commit + standard pools) and per-pool admin
//! forwards (pause, unpause, emergency withdraw, recovery, and the
//! threshold-crossed callback).
//!
//! Split into two files:
//!   - `create`  — creation entry points + input validators
//!   - `admin`   — post-creation state transitions
//!
//! Submodules are exposed directly (`pool_lifecycle::admin`,
//! `pool_lifecycle::create`) rather than glob-re-exported here, so the
//! public surface of each lives next to its definitions and a new
//! `pub fn` in either submodule does not silently widen
//! `crate::execute::pool_lifecycle::*`.

pub mod admin;
pub mod create;
