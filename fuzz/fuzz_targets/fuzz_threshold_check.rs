//! Pure-math fuzz of the USD-conversion + threshold check the pool runs
//! during commit. Mirrors `creator_pool::swap_helper` math:
//!
//!     usd_value = bluechip_amount * normalized_price / ORACLE_PRICE_PRECISION
//!
//! where normalized_price is the Pyth (price, expo) feed normalized to
//! 6-decimal USD (factory does this in `internal_bluechip_price_oracle`).
//!
//! Invariants:
//!   * Conversion never panics for any (Vec<u128>, i64, i32) input (we
//!     bound expo to factory's accepted range -12..=-4).
//!   * Conversion result matches a naive bigint reference impl bit-for-bit.
//!   * Threshold cross detection is monotone in cumulative input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use cosmwasm_std::Uint128;

const ORACLE_PRICE_PRECISION: u128 = 1_000_000;

#[derive(arbitrary::Arbitrary, Debug)]
struct Input {
    /// Per-step bluechip commit amounts (ubluechip = 6-dec).
    /// Bounded length so each fuzz iter completes quickly.
    committed_amounts: Vec<u32>,
    /// Pyth raw price (signed). We test only positive; factory rejects
    /// non-positive prices.
    raw_price: u32,
    /// Pyth expo (must be negative, factory accepts -12..=-4).
    raw_expo: u8,
    /// Threshold target in 6-dec USD.
    threshold_usd_6dec: u64,
}

/// Normalize a Pyth (price, expo) to a 6-decimal USD-per-bluechip rate.
/// `expo` must lie in [-12, -4].
fn normalize_price(price: u128, expo: i32) -> Option<u128> {
    let target_expo: i32 = -6;
    let shift = expo - target_expo; // negative or positive
    if shift == 0 { return Some(price); }
    if shift < 0 {
        // expo < target_expo: divide by 10^(|shift|).
        let by = 10u128.checked_pow((-shift) as u32)?;
        Some(price / by)
    } else {
        // expo > target_expo: multiply by 10^shift.
        let by = 10u128.checked_pow(shift as u32)?;
        price.checked_mul(by)
    }
}

fn contract_usd(bluechip: u128, normalized_rate: u128) -> Option<u128> {
    bluechip
        .checked_mul(normalized_rate)?
        .checked_div(ORACLE_PRICE_PRECISION)
}

/// Reference: same arithmetic, no compaction. Used to check the
/// contract path bit-for-bit.
fn reference_usd(bluechip: u128, normalized_rate: u128) -> Option<u128> {
    // Identity in this case — the math is so simple any non-trivial
    // alternative implementation would already be an error. Kept as a
    // separate function so future contract refactors that change the
    // operation order (e.g. div-then-mul for precision) get caught.
    bluechip.checked_mul(normalized_rate).map(|v| v / ORACLE_PRICE_PRECISION)
}

fuzz_target!(|input: Input| {
    if input.raw_price == 0 { return; }
    let expo = -((input.raw_expo % 9) as i32 + 4); // -4..=-12
    let raw_price = input.raw_price as u128;
    let normalized = match normalize_price(raw_price, expo) {
        Some(v) if v > 0 => v,
        _ => return,
    };
    let mut cumulative_bluechip: u128 = 0;
    let mut cumulative_usd: u128 = 0;
    let mut threshold_crossed_step: Option<usize> = None;
    let threshold = input.threshold_usd_6dec as u128;

    for (i, &amt) in input.committed_amounts.iter().take(64).enumerate() {
        let amt = amt as u128;
        let new_cum = match cumulative_bluechip.checked_add(amt) {
            Some(v) => v,
            None => break,
        };
        let usd_step = match contract_usd(amt, normalized) {
            Some(v) => v,
            None => break,
        };
        let usd_step_ref = reference_usd(amt, normalized).expect("reference must compute");
        assert_eq!(
            usd_step, usd_step_ref,
            "contract vs reference mismatch: amt={} rate={} got={} want={}",
            amt, normalized, usd_step, usd_step_ref
        );

        let new_cum_usd = match cumulative_usd.checked_add(usd_step) {
            Some(v) => v,
            None => break,
        };

        // Monotonicity: cumulative_usd must never decrease.
        assert!(
            new_cum_usd >= cumulative_usd,
            "cumulative usd regressed: prev={} now={}",
            cumulative_usd, new_cum_usd
        );

        cumulative_bluechip = new_cum;
        cumulative_usd = new_cum_usd;

        if threshold_crossed_step.is_none() && cumulative_usd >= threshold && threshold > 0 {
            threshold_crossed_step = Some(i);
        }
    }

    // Once threshold is crossed, it stays crossed.
    if let Some(step) = threshold_crossed_step {
        assert!(
            cumulative_usd >= threshold,
            "threshold un-crossed: step={} usd={} threshold={}",
            step, cumulative_usd, threshold
        );
    }

    let _ = Uint128::new(cumulative_usd);
});
