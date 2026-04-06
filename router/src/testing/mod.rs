//! Test-only support code for the router.
//!
//! `mock_pool` is a minimal XYK pool used in place of the real bluechip
//! pool contract so the router's integration tests can stand up a few
//! pools without dragging the entire factory + oracle + threshold flow
//! into every test setup. `integration_tests` contains the actual
//! end-to-end test cases driven by `cw-multi-test`.

pub mod mock_pool;

#[cfg(test)]
mod integration_tests;
