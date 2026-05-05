//! Re-exports of `pool_core::liquidity` LP-operation handlers.
//!
//! Explicit names (not glob) so the surface of `creator_pool::liquidity`
//! is pinned at this file: removing one of these from `pool_core::liquidity`
//! becomes a build error here rather than silently shrinking the API.
pub use pool_core::liquidity::{
    add_to_position, execute_add_to_position, execute_add_to_position_with_verify,
    execute_collect_fees, execute_deposit_liquidity, execute_deposit_liquidity_with_verify,
    execute_remove_all_liquidity, execute_remove_partial_liquidity,
    execute_remove_partial_liquidity_by_percent, remove_all_liquidity, remove_partial_liquidity,
};
