//! Re-export of `pool_core::asset::*`. Preserves every existing
//! `use crate::asset::X;` import in the creator-pool crate, including
//! the `TokenInfoPoolExt` trait import needed for method-call resolution
//! on `TokenInfo` values.
pub use pool_core::asset::*;
