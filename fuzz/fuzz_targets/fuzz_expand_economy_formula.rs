//! Pure-math fuzz of the expand-economy decay polynomial:
//!     m(s, x) = max(0, 500_000_000 - ((5x² + x) * 1_000_000) / (s/6 + 333x))
//! capped to 0 when x > 1_000_000_000.
//!
//! Exercises:
//!   * Never panics on any (s, x) within u64 bounds.
//!   * Output ∈ [0, 500_000_000] ubluechip.
//!   * Matches a naive reference implementation (u128 arithmetic) for
//!     all inputs the contract path also computes (i.e. x ≤ 1e9).
//!   * For x > 1e9, contract returns Uint128::zero() (saturating cap).

#![no_main]

use libfuzzer_sys::fuzz_target;
use cosmwasm_std::Uint128;
use factory::mint_bluechips_pool_creation::calculate_mint_amount;

const BASE: u128 = 500_000_000;
const MAX_DECAY_X: u128 = 1_000_000_000;

/// Reference implementation. Returns the same value as the contract for
/// inputs within range; for x > MAX_DECAY_X returns 0 (the contract's
/// saturating-cap behavior).
fn reference(seconds_elapsed: u64, pools_created: u64) -> u128 {
    let x = pools_created as u128;
    let s = seconds_elapsed as u128;
    if x > MAX_DECAY_X {
        return 0;
    }
    let five_x_sq = 5u128.checked_mul(x).and_then(|v| v.checked_mul(x));
    let Some(five_x_sq) = five_x_sq else { return 0 };
    let Some(num) = five_x_sq.checked_add(x) else { return 0 };
    let s_div_6 = s / 6;
    let Some(denom) = 333u128.checked_mul(x).and_then(|v| v.checked_add(s_div_6)) else { return 0 };
    if denom == 0 {
        return BASE;
    }
    let Some(scaled) = num.checked_mul(1_000_000) else { return 0 };
    let div_result = scaled / denom;
    if div_result >= BASE { 0 } else { BASE - div_result }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 { return; }
    let s = u64::from_le_bytes(data[0..8].try_into().unwrap());
    let x = u64::from_le_bytes(data[8..16].try_into().unwrap());

    let res = calculate_mint_amount(s, x).expect("calculate_mint_amount must not error");
    let v = res.u128();

    // Bounded output.
    assert!(v <= BASE, "mint amount {} exceeds BASE {}", v, BASE);
    // Output is Uint128::zero() at the saturating cap.
    if (x as u128) > MAX_DECAY_X {
        assert!(v == 0, "saturation cap not honored: x={} v={}", x, v);
    }

    // Reference comparison.
    let want = reference(s, x);
    assert_eq!(
        v, want,
        "mismatch with reference: s={} x={} contract={} reference={}",
        s, x, v, want
    );

    // Debug noise to keep the type used.
    let _ = Uint128::new(v);
});
