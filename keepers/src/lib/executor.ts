import type { TxResult } from "./decisions.js";

/**
 * Minimal interface the keeper loops need to interact with the chain.
 * Defined as an interface (not a concrete class) so tests can provide
 * an in-memory mock that simulates contract behavior — cooldowns,
 * bounty events, skip attributes — without spinning up a real chain.
 *
 * The real implementation wraps a CosmJS SigningCosmWasmClient; see
 * client.ts.
 */
export interface Executor {
  /** Keeper's own address. Used both as tx sender and as bounty recipient. */
  readonly address: string;

  /**
   * Execute a contract message. Resolves to a TxResult on success.
   * Rejects with an Error on contract error (UpdateTooSoon,
   * NothingToRecover, Unauthorized, etc) or RPC failure.
   */
  execute(contract: string, msg: Record<string, unknown>): Promise<TxResult>;

  /** Query the keeper's own bluechip balance. Used to warn on low runway. */
  getBalance(denom: string): Promise<bigint>;
}
