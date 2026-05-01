//! Bluechip Standard Pool — plain xyk pool around two pre-existing
//! assets (any combination of `Native` and `CreatorToken`). No commit
//! phase, no threshold, no distribution; immediately tradeable and
//! depositable at creation.
//!
//! ECONOMIC SCOPE.
//! Standard pools are completely OUTSIDE the bluechip
//! expand-economy / mint-decay flow:
//!   - They never cross a commit threshold (there is no `Commit`
//!     ExecuteMsg variant on this contract).
//!   - They never call `NotifyThresholdCrossed` on the factory.
//!   - The factory rejects `NotifyThresholdCrossed` from any pool
//!     with `pool_kind == Standard` (defense-in-depth in
//!     `execute_notify_threshold_crossed`).
//!   - `factory::calculate_and_mint_bluechip` carries an
//!     additional hard guard that rejects standard-pool inputs.
//!   - `commit_pool_ordinal` is set to 0 in `PoolDetails` for every
//!     standard pool, isolating them from the mint-decay polynomial's
//!     `x` term (which is fed exclusively from commit-pool ordinals).
//!
//! The arbitrary-asset shape standard pools allow (any tokenfactory
//! denom, any third-party CW20 paired against bluechip) is also why
//! this contract wires SubMsg-based deposit balance verification —
//! a fee-on-transfer / rebasing CW20 must not corrupt reserve
//! accounting. Creator-pool's CW20 is freshly minted by the factory
//! and is structurally trusted, so it skips that path.
//!
//! The vast majority of this crate's logic lives in `pool_core`. The
//! modules below are thin entry-point wrappers: they define the
//! `#[entry_point]` exports and route each `ExecuteMsg` / `QueryMsg`
//! variant to the corresponding `pool_core::*` handler.

pub mod contract;
pub mod error;
pub mod msg;
pub mod query;

#[cfg(test)]
mod testing;
