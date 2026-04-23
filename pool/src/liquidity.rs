//! LP-operation handlers live in `pool_core::liquidity`. This re-export
//! preserves every `use crate::liquidity::X;` import in the creator-pool
//! crate and its tests.
pub use pool_core::liquidity::*;
