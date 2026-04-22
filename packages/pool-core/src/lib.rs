//! Shared AMM + liquidity-position core for Bluechip creator pools and
//! standard pools.
//!
//! This crate is a pure library — it exports no `#[entry_point]`s. The
//! two pool contract crates (`creator-pool` and `standard-pool`) provide
//! their own instantiate/execute/query/migrate/reply entry points and
//! dispatch into the handler functions re-exported here.
//!
//! Scope:
//!   - AMM math: constant-product swap, spread/slippage, price
//!     accumulator.
//!   - Liquidity positions: deposit, add, remove (partial / full /
//!     percentage), collect fees, NFT ownership sync, fee-size
//!     multiplier clipping.
//!   - Asset handling: pair-shape-agnostic transfer/collect helpers for
//!     Native/CW20/CW20-CW20/Native-Native pools.
//!   - Admin ops shared by both pool kinds: pause, unpause, emergency
//!     withdraw (initiate + execute + cancel), ensure_not_drained.
//!   - Shared state items and structs backing the above.
//!
//! Out of scope (lives in the consuming contract crates):
//!   - Commit-phase logic: commit, threshold crossing, distribution,
//!     claim-creator-excess, claim-creator-fees, retry-factory-notify,
//!     oracle-backed USD conversions.  (creator-pool/)
//!   - Entry points, factory message dispatch, contract-level tests.
//!
//! Intended consumers:
//!   - `creator-pool` — the original two-phase pool. Extends this crate
//!     with commit-phase state and handlers.
//!   - `standard-pool` — plain xyk pool. Thin wrapper that delegates
//!     every op to functions here.
//!
//! STATUS: skeleton. Code extraction from `pool/` happens in subsequent
//! commits (see H14 split-series plan).

pub mod error;
