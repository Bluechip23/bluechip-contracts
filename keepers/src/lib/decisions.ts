// Pure decision logic — keeping this separate from CosmJS I/O makes it
// trivial to unit-test. Every function in here takes plain inputs and
// returns plain outputs; no chain, no wallet, no side effects.

/**
 * A transaction response we care about. This is the minimal shape we
 * need from CosmJS's DeliverTxResponse — defined as an interface rather
 * than importing from CosmJS so tests don't need to construct full
 * response objects.
 */
export interface TxResult {
  code: number;
  transactionHash: string;
  rawLog?: string | undefined;
  events?: ReadonlyArray<TxEvent>;
}

export interface TxEvent {
  type: string;
  attributes: ReadonlyArray<{ key: string; value: string }>;
}

/**
 * Scrape a specific attribute value from a wasm event in a tx result.
 * Returns undefined if the attribute isn't present. Used by the keeper
 * to log "did we actually get paid?" after each tx.
 */
export function readWasmAttribute(
  result: TxResult,
  key: string,
): string | undefined {
  if (!result.events) return undefined;
  for (const event of result.events) {
    if (event.type !== "wasm") continue;
    for (const attr of event.attributes) {
      if (attr.key === key) return attr.value;
    }
  }
  return undefined;
}

/**
 * Classify the outcome of a keeper tx. Used both to drive logging and
 * to decide whether to alert the operator.
 */
export type TxOutcome =
  | { kind: "paid"; bountyUsd: string; bountyBluechip: string }
  | { kind: "skipped"; reason: string }
  | { kind: "ok" } // tx succeeded but emitted no bounty attributes
  | { kind: "failed"; rawLog: string };

export function classifyBountyTx(result: TxResult): TxOutcome {
  if (result.code !== 0) {
    return { kind: "failed", rawLog: result.rawLog ?? "tx failed" };
  }
  const paidUsd = readWasmAttribute(result, "bounty_paid_usd");
  const paidBluechip = readWasmAttribute(result, "bounty_paid_bluechip");
  if (paidUsd && paidBluechip) {
    return { kind: "paid", bountyUsd: paidUsd, bountyBluechip: paidBluechip };
  }
  const skipped = readWasmAttribute(result, "bounty_skipped");
  if (skipped) {
    return { kind: "skipped", reason: skipped };
  }
  return { kind: "ok" };
}

/**
 * Compute how long to sleep before the next oracle-keeper poll.
 *
 * Called after a loop iteration completes. Adds a small jitter so
 * multiple keeper instances don't all wake up at the same instant.
 */
export function nextOracleSleepMs(
  baseIntervalMs: number,
  jitterMs: number = 5_000,
  random: () => number = Math.random,
): number {
  if (baseIntervalMs <= 0) return 0;
  const jitter = Math.floor(random() * jitterMs);
  return baseIntervalMs + jitter;
}

/**
 * Compute the next sleep for the distribution keeper. If the previous
 * iteration made progress on at least one pool, poll sooner (pools in
 * distribution state can have many batches queued). Otherwise use the
 * full interval.
 */
export function nextDistributionSleepMs(
  baseIntervalMs: number,
  madeProgress: boolean,
  fastPollMs: number = 15_000,
): number {
  if (madeProgress) return Math.min(fastPollMs, baseIntervalMs);
  return baseIntervalMs;
}

/**
 * Extract the `distribution_complete` attribute to decide whether we
 * should keep hammering a specific pool or move on.
 */
export function isDistributionComplete(result: TxResult): boolean {
  const complete = readWasmAttribute(result, "distribution_complete");
  return complete === "true";
}

/**
 * Decide whether the keeper should keep calling the same pool in a
 * tight loop during a single iteration. We keep going as long as:
 *   - the last tx was a successful `paid` or `ok` outcome
 *   - AND distribution_complete is still false
 *
 * Putting this logic in a pure function lets tests pin the exact
 * conditions that drive or stop the inner loop.
 */
export function shouldContinueSamePool(
  lastOutcome: TxOutcome,
  complete: boolean,
): boolean {
  if (complete) return false;
  return lastOutcome.kind === "paid" || lastOutcome.kind === "ok";
}
