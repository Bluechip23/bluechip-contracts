//! Bluechip multi-hop swap router.
//!
//! Lets users execute a swap that traverses multiple Bluechip pools in a
//! single atomic transaction. The router itself contains no AMM math: it
//! delegates all pricing and asset movement to the pool contracts and only
//! orchestrates the sequence, validates the route, and enforces slippage
//! and deadline guarantees on the final output.
//!
//! See [`crate::contract`] for entry points and [`crate::msg`] for the
//! external message surface.

pub mod contract;
pub mod error;
pub mod execution;
pub mod msg;
pub mod simulation;
pub mod state;

pub use crate::error::RouterError;
