// Contract message and response types that mirror the on-chain
// definitions in factory/src/msg.rs, factory/src/query.rs, and
// pool/src/msg.rs. Keep these in sync with the Rust side if those
// evolve.

// ---------------------------------------------------------------------------
// Factory queries
// ---------------------------------------------------------------------------

export interface BountyResponse {
  /** Bounty amount denominated in USD with 6 decimals: 1_000_000 = $1.00 */
  bounty_usd: string;
}

export const FactoryQueryOracleBounty = { oracle_update_bounty: {} } as const;
export const FactoryQueryDistributionBounty = { distribution_bounty: {} } as const;

// ---------------------------------------------------------------------------
// Factory execute messages
// ---------------------------------------------------------------------------

export const FactoryExecUpdateOraclePrice = { update_oracle_price: {} } as const;

// ---------------------------------------------------------------------------
// Pool execute messages
// ---------------------------------------------------------------------------

export const PoolExecContinueDistribution = { continue_distribution: {} } as const;

// ---------------------------------------------------------------------------
// Error sniffing — contract errors surface as strings in tx responses
// ---------------------------------------------------------------------------

/** The factory returns this when the cooldown window has not elapsed. */
export const UPDATE_TOO_SOON_MARKER = "UpdateTooSoon";

/** The pool returns this when distribution is not running for a pool. */
export const NOTHING_TO_RECOVER_MARKER = "NothingToRecover";

/** CosmWasm generic "not found" surface when DISTRIBUTION_STATE was never saved. */
export const NOT_FOUND_MARKER = "not found";

/**
 * Heuristic: returns true if the error string indicates the action was
 * a no-op that the keeper should treat as a normal skip rather than a
 * real failure.
 *
 * We check marker substrings rather than exact matches because the full
 * error wrapper varies across cosmjs versions and chain forks.
 */
export function isExpectedSkipError(message: string): boolean {
  return (
    message.includes(UPDATE_TOO_SOON_MARKER) ||
    message.includes(NOTHING_TO_RECOVER_MARKER) ||
    message.includes(NOT_FOUND_MARKER)
  );
}
