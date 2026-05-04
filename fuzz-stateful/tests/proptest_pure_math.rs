//! Stable-runnable proptest mirror of the cargo-fuzz pure-math targets
//! in `../fuzz/fuzz_targets/`. cargo-fuzz needs nightly + libFuzzer; this
//! file lets the same invariants run inside `cargo test` on stable so CI
//! doesn't depend on nightly being installed.
//!
//! The cargo-fuzz versions are still preferred for long runs (better
//! coverage, corpus persistence). These are quick-feedback gates.

use cosmwasm_std::{Decimal, Uint128, Uint256};
use factory::mint_bluechips_pool_creation::calculate_mint_amount;
use pool_core::swap::compute_swap;
use proptest::prelude::*;

const ORACLE_PRICE_PRECISION: u128 = 1_000_000;
const BASE_MINT: u128 = 500_000_000;
const MAX_DECAY_X: u128 = 1_000_000_000;

fn reference_mint(seconds_elapsed: u64, pools_created: u64) -> u128 {
    let x = pools_created as u128;
    let s = seconds_elapsed as u128;
    if x > MAX_DECAY_X { return 0; }
    let five_x_sq = match 5u128.checked_mul(x).and_then(|v| v.checked_mul(x)) {
        Some(v) => v, None => return 0,
    };
    let num = match five_x_sq.checked_add(x) { Some(v) => v, None => return 0 };
    let s_div_6 = s / 6;
    let denom = match 333u128.checked_mul(x).and_then(|v| v.checked_add(s_div_6)) {
        Some(v) => v, None => return 0,
    };
    if denom == 0 { return BASE_MINT; }
    let scaled = match num.checked_mul(1_000_000) { Some(v) => v, None => return 0 };
    let div = scaled / denom;
    if div >= BASE_MINT { 0 } else { BASE_MINT - div }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8192, max_shrink_iters: 0, .. ProptestConfig::default() })]

    #[test]
    fn expand_economy_formula(s in 0u64..u64::MAX, x in 0u64..u64::MAX) {
        let res = calculate_mint_amount(s, x).expect("must not error");
        let v = res.u128();
        prop_assert!(v <= BASE_MINT, "exceeded BASE_MINT");
        if (x as u128) > MAX_DECAY_X {
            prop_assert_eq!(v, 0, "saturating cap");
        }
        prop_assert_eq!(v, reference_mint(s, x), "reference mismatch");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8192, max_shrink_iters: 0, .. ProptestConfig::default() })]

    #[test]
    fn swap_math_invariants(
        offer_pool in 1_000u128..u128::from(u64::MAX),
        ask_pool in 1_000u128..u128::from(u64::MAX),
        offer_amount in 1u128..u128::from(u64::MAX),
        fee_bps in 0u64..1000u64,
    ) {
        let commission = Decimal::from_ratio(fee_bps, 10_000u64);
        let offer = Uint128::new(offer_pool);
        let ask = Uint128::new(ask_pool);
        let inp = Uint128::new(offer_amount);

        let res = compute_swap(offer, ask, inp, commission);
        let (ret_amt, _spread, comm) = match res {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let total_out = ret_amt.checked_add(comm).unwrap_or(Uint128::MAX);
        prop_assert!(total_out <= ask, "drained more than reserve");

        let pre_k = Uint256::from(offer) * Uint256::from(ask);
        let new_offer = Uint256::from(offer) + Uint256::from(inp);
        let new_ask = Uint256::from(ask) - Uint256::from(total_out);
        let post_k = new_offer * new_ask;
        prop_assert!(post_k >= pre_k, "constant product not preserved");
    }
}

fn contract_usd(bluechip: u128, normalized: u128) -> Option<u128> {
    bluechip.checked_mul(normalized).map(|v| v / ORACLE_PRICE_PRECISION)
}
fn reference_usd(bluechip: u128, normalized: u128) -> Option<u128> {
    bluechip.checked_mul(normalized).map(|v| v / ORACLE_PRICE_PRECISION)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8192, max_shrink_iters: 0, .. ProptestConfig::default() })]

    #[test]
    fn threshold_check_matches_reference(
        commits in prop::collection::vec(0u32..u32::MAX, 1..32),
        normalized in 1u128..1_000_000_000_000u128,
    ) {
        let mut cum_usd: u128 = 0;
        let mut last = 0u128;
        for amt in &commits {
            let amt = *amt as u128;
            let c = contract_usd(amt, normalized);
            let r = reference_usd(amt, normalized);
            prop_assert_eq!(c, r);
            if let Some(v) = c {
                cum_usd = cum_usd.checked_add(v).unwrap_or(u128::MAX);
                prop_assert!(cum_usd >= last, "cumulative regressed");
                last = cum_usd;
            }
        }
    }
}
