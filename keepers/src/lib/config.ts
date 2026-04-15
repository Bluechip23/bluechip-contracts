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
