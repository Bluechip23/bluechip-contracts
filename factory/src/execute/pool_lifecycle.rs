//! Pool lifecycle: creation (commit + standard pools) and per-pool admin
//! forwards (pause, unpause, emergency withdraw, recovery, and the
//! threshold-crossed callback).
//!
//! Split into two files:
//!   - `create`  — creation entry points + input validators
//!   - `admin`   — post-creation state transitions

pub mod admin;
pub mod create;

pub use admin::*;
pub use create::*;
