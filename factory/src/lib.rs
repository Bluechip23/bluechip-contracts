pub mod asset;
pub mod error;
pub mod execute;
pub mod internal_bluechip_price_oracle;
pub mod internal_bluechip_price_oracle_query;
pub mod migrate;
pub mod mint_bluechips_pool_creation;
pub mod msg;
pub mod pool_create_cleanup;
pub mod pool_creation_reply;
pub mod pool_struct;
pub mod pyth_types;
pub mod query;
pub mod state;

// ---------------------------------------------------------------------------
// Re-exports: top-level facade for downstream crates so `factory::ExecuteMsg`
// works without needing to learn the internal module layout.
// ---------------------------------------------------------------------------
pub use error::ContractError;
pub use msg::ExecuteMsg;
pub use query::QueryMsg;
pub use state::FactoryInstantiate;

/// cw2 contract name, written by both `instantiate` and `migrate`. Centralized
/// so a future re-publish under a different crate name only requires one edit.
pub const CONTRACT_NAME: &str = "crates.io:bluechip-factory";
/// cw2 contract version, sourced from Cargo.toml at compile time.
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod mock_querier;
#[cfg(test)]
mod testing;
