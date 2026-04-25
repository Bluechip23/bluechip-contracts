//! Integration tests for standard-pool.
//!
//! Exercises pool-core's shared handlers end-to-end through standard-
//! pool's thin dispatch, with no commit-phase layer in the way.
//! Organized by handler area — one file per concern.

mod collect_fees;
mod deposit_liquidity;
mod emergency_withdraw;
mod fixtures;
mod instantiation;
mod queries;
mod remove_liquidity;
mod swap;
