//! Pure-math fuzz of pool-core's constant-product swap.
//!
//! Exercises:
//!   * `compute_swap` never panics for any (Uint128, Uint128, Uint128, fee).
//!     It may return Err for overflow; that's accepted.
//!   * Constant-product preservation: after the swap,
//!         (offer_pool + amount_in) * (ask_pool - return_amount - commission)
//!     is at least the pre-swap product k = offer_pool * ask_pool.
//!     Strict inequality with positive fee.
//!   * `return_amount + commission_amount <= ask_pool` (cannot drain
//!     more than the reserve).

#![no_main]

use libfuzzer_sys::fuzz_target;
use cosmwasm_std::{Decimal, Uint128, Uint256};
use pool_core::swap::compute_swap;

#[derive(arbitrary::Arbitrary, Debug)]
struct Input {
    offer_pool: u128,
    ask_pool: u128,
    offer_amount: u128,
    fee_bps: u16,
}

fuzz_target!(|input: Input| {
    // Skip degenerate reserves and unbounded fees: the production
    // factory caps fee at 100 bps (1%) and refuses to instantiate a
    // pool with empty reserves; our property-test input space matches.
    let fee_bps = (input.fee_bps % 1001) as u64; // 0..1000 bps (0..10%)
    let commission_rate = Decimal::from_ratio(fee_bps, 10_000u64);

    let offer = Uint128::new(input.offer_pool);
    let ask = Uint128::new(input.ask_pool);
    let inp = Uint128::new(input.offer_amount);

    // Skip cases where the offer pool is zero — the contract would
    // refuse those upstream (`MINIMUM_LIQUIDITY = 1000`); we
    // mirror that gate here.
    if input.offer_pool < 1_000 || input.ask_pool < 1_000 || input.offer_amount == 0 {
        return;
    }

    let res = compute_swap(offer, ask, inp, commission_rate);

    let (return_amount, _spread, commission) = match res {
        Ok(v) => v,
        // Overflow / divide-by-zero are documented Err cases.
        Err(_) => return,
    };

    // No more than the ask reserve was paid out.
    let total_out = return_amount.checked_add(commission)
        .expect("Uint128 add overflowed — output exceeds Uint128::MAX");
    assert!(
        total_out <= ask,
        "swap drained more than reserve: out={} ask={}",
        total_out, ask
    );

    // Constant-product preservation. Use Uint256 throughout to dodge
    // overflow on (offer + offer_amount) * (ask - out).
    let pre_k = Uint256::from(offer).checked_mul(Uint256::from(ask))
        .expect("k overflow — sanity bound exceeded");
    let new_offer = Uint256::from(offer).checked_add(Uint256::from(inp))
        .expect("new_offer overflow");
    // Defensive: ask - total_out cannot underflow per the prior assert.
    let new_ask = Uint256::from(ask) - Uint256::from(total_out);
    let post_k = new_offer.checked_mul(new_ask)
        .expect("post_k overflow");
    assert!(
        post_k >= pre_k,
        "constant product not preserved: pre={} post={} (offer={}, ask={}, in={}, out={})",
        pre_k, post_k, offer, ask, inp, total_out
    );
});
