import { z } from "zod";

// ---------------------------------------------------------------------------
// Schema — keeper configuration loaded from environment variables
// ---------------------------------------------------------------------------

const nonEmptyString = z.string().min(1);

const commaSeparatedAddresses = z
  .string()
  .optional()
  .transform((raw) => {
    if (!raw) return [] as string[];
    return raw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
  });

/**
 * Full config schema. Every field here is required at runtime unless
 * marked optional. Unknown env vars are simply ignored.
 */
export const ConfigSchema = z.object({
  // Chain connection
  RPC_ENDPOINT: nonEmptyString,
  CHAIN_ID: nonEmptyString,
  BECH32_PREFIX: nonEmptyString,
  GAS_PRICE: nonEmptyString.default("0.025ubluechip"),
  GAS_DENOM: nonEmptyString.default("ubluechip"),

  // Contracts
  FACTORY_ADDRESS: nonEmptyString,
  POOL_ADDRESSES: commaSeparatedAddresses,

  // Wallet
  KEEPER_MNEMONIC: nonEmptyString,

  // Oracle keeper tuning
  ORACLE_POLL_INTERVAL_MS: z
    .string()
    .default("330000") // 5.5 min (one check slightly after the 5-min window)
    .transform((s) => Number.parseInt(s, 10)),

  // Distribution keeper tuning
  DISTRIBUTION_POLL_INTERVAL_MS: z
    .string()
    .default("1800000") // 30 minutes
    .transform((s) => Number.parseInt(s, 10)),
  DISTRIBUTION_PER_POOL_DELAY_MS: z
    .string()
    .default("2000") // 2 second breather between pool calls
    .transform((s) => Number.parseInt(s, 10)),

  // Safety
  MIN_KEEPER_BALANCE_UBLUECHIP: z
    .string()
    .default("1000000") // 1 bluechip minimum before we warn about gas
    .transform((s) => BigInt(s)),

  // Warning threshold for the FACTORY contract's bounty reserve. Distinct
  // from MIN_KEEPER_BALANCE_UBLUECHIP, which guards the keeper wallet's gas
  // runway. The factory pays both the oracle bounty (capped at $1) and the
  // distribution bounty (capped at $1 per batch) out of its native balance;
  // when the reserve runs low, bounties begin emitting `bounty_skipped =
  // insufficient_factory_balance` and the operator needs to top up. Default
  // 100 bluechip ≈ a few thousand bounties at $0.05 — set lower for tighter
  // alerting, higher to silence noise.
  MIN_FACTORY_BOUNTY_RESERVE_UBLUECHIP: z
    .string()
    .default("100000000")
    .transform((s) => BigInt(s)),

  // Mock-oracle price push (local/testnet only). When MOCK_ORACLE_ADDRESS
  // is set, the oracle keeper pushes a fresh SetPrice to the mock oracle
  // before each UpdateOraclePrice call, simulating the production flow
  // where the factory reads a live price source every 5 minutes.
  // Leave unset in production.
  MOCK_ORACLE_ADDRESS: z.string().optional(),
  MOCK_PRICE_FEED_ID: nonEmptyString.default("BLUECHIP_USD"),
  MOCK_PRICE_UBLUECHIP: nonEmptyString.default("1000000"),
});

export type Config = z.infer<typeof ConfigSchema>;

/**
 * Parse config from a raw env-like object. Throws a clear ZodError if
 * validation fails.
 */
export function parseConfig(raw: Record<string, string | undefined>): Config {
  return ConfigSchema.parse(raw);
}

/**
 * Convenience: parse the live process.env. Used by the keeper entrypoints.
 */
export function loadConfigFromEnv(): Config {
  return parseConfig(process.env);
}
