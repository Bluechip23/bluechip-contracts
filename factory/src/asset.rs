// Re-export all shared asset types from the interfaces crate.
// Previously, TokenType, TokenInfo, and related helpers were defined
// independently here, duplicating pool/src/asset.rs. Now both crates
// share a single definition from pool-factory-interfaces.
pub use pool_factory_interfaces::asset::*;
