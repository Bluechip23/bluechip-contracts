#!/usr/bin/env bash
# Single-command fuzz runner for the bluechip contracts.
#
# Quick mode (default — used in CI):
#   ./fuzz.sh
#     Runs the proptest stateful harness + the stable-mirror pure-math
#     proptests. Exits non-zero on any failure or invariant violation.
#
# Long mode (overnight):
#   ./fuzz.sh long
#     Same as quick + 5 minutes of cargo-fuzz on each pure-math target
#     (requires nightly + cargo-fuzz; see FUZZING.md).
#
# Verbose:
#   FUZZ_DEBUG=1 ./fuzz.sh

set -euo pipefail

cd "$(dirname "$0")"

MODE="${1:-quick}"
PROPTEST_QUICK_CASES="${PROPTEST_QUICK_CASES:-32}"
PROPTEST_FULL_CASES="${PROPTEST_FULL_CASES:-256}"
CARGO_FUZZ_SECS="${CARGO_FUZZ_SECS:-300}"

echo "==> [1/3] Stateful proptest harness ($PROPTEST_QUICK_CASES quick cases)"
PROPTEST_CASES="$PROPTEST_QUICK_CASES" cargo test \
  -p fuzz-stateful --release --test fuzz_stateful fuzz_stateful_quick

echo "==> [2/3] Stateful proptest harness ($PROPTEST_FULL_CASES full cases)"
PROPTEST_CASES="$PROPTEST_FULL_CASES" cargo test \
  -p fuzz-stateful --release --test fuzz_stateful fuzz_stateful

echo "==> [3/3] Pure-math proptests (stable mirror of cargo-fuzz targets)"
cargo test -p fuzz-stateful --release --test proptest_pure_math

if [ "$MODE" = "long" ]; then
  if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "ERROR: 'cargo fuzz' subcommand not installed."
    echo "       Install with: cargo install cargo-fuzz"
    echo "       And ensure a nightly toolchain is available:"
    echo "       rustup toolchain install nightly"
    exit 2
  fi
  if ! rustup toolchain list | grep -q nightly; then
    echo "ERROR: nightly toolchain required for cargo-fuzz."
    echo "       Run: rustup toolchain install nightly"
    exit 2
  fi
  for tgt in fuzz_expand_economy_formula fuzz_swap_math fuzz_threshold_check; do
    echo "==> cargo-fuzz $tgt for ${CARGO_FUZZ_SECS}s"
    (
      cd fuzz
      cargo +nightly fuzz run "$tgt" -- -max_total_time="$CARGO_FUZZ_SECS"
    )
  done
fi

echo "==> All fuzz checks passed."
