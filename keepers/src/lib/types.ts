// Contract message shapes that mirror the on-chain definitions in
// factory/src/msg.rs and pool/src/msg.rs. Keep these in sync with the
// Rust side if those evolve.

// ---------------------------------------------------------------------------
// Execute messages — the only thing the keeper actually constructs
// ---------------------------------------------------------------------------

export const FactoryExecUpdateOraclePrice = { update_oracle_price: {} } as const;
export const PoolExecContinueDistribution = { continue_distribution: {} } as const;

// ---------------------------------------------------------------------------
// Error sniffing — contract errors surface as strings in tx responses
// ---------------------------------------------------------------------------

// Substring markers for contract errors the keeper should treat as a
// normal "no-op" rather than a real failure. We match substrings rather
// than exact strings because the cosmjs error wrapper varies across
// versions and chain forks.
// Match both the Rust variant name AND the user-facing #[error(...)] display
// string that propagates over RPC — the CosmWasm client sees the display
// form, not the variant name. Keeping both protects against either form
// appearing in future error payloads.
const SKIP_MARKERS = [
  "UpdateTooSoon",
  "too quickly",
  "NothingToRecover",
  "not found",
] as const;

export function isExpectedSkipError(message: string): boolean {
  return SKIP_MARKERS.some((m) => message.includes(m));
}
