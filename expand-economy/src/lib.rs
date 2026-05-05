pub mod attrs;
pub mod contract;
pub mod denom;
pub mod error;
pub mod expand;
pub mod factory_query;
pub mod helpers;
pub mod migrate;
pub mod msg;
pub mod state;
pub mod timelock;

#[cfg(test)]
mod audit_tests;
#[cfg(test)]
mod tests;

pub use crate::error::ContractError;

/// cw2 contract name persisted on every (re)instantiate and migrate.
/// Centralized so a future re-publish under a different crate name
/// only requires one edit.
pub const CONTRACT_NAME: &str = "crates.io:expand-economy";
/// cw2 contract version, sourced from Cargo.toml at compile time.
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
