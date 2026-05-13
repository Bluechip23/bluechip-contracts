//! LP-operation handlers, split into four submodules by operation:
//! - [`deposit`] — first-time deposit (mints a new position NFT)
//! - [`add`]     — top-up an existing position
//! - [`remove`]  — full / partial / by-percent withdrawal
//! - [`fees`]    — LP-fee collection with creator-pot clip routing
//!
//! Every public handler is re-exported at the `liquidity::` path so
//! downstream crates (`creator-pool`, `standard-pool`) can continue
//! to import them as `pool_core::liquidity::execute_*` unchanged.

pub mod add;
pub mod deposit;
pub mod fees;
pub mod remove;

pub use add::*;
pub use deposit::*;
pub use fees::*;
pub use remove::*;
