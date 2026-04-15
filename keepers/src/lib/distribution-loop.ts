import {
  classifyBountyTx,
  isDistributionComplete,
  shouldContinueSamePool,
  type TxOutcome,
} from "./decisions.js";
import type { Executor } from "./executor.js";
import { sleep } from "./sleep.js";
import { PoolExecContinueDistribution, isExpectedSkipError } from "./types.js";
import { log } from "./logger.js";

/**
 * Discriminated union of every possible final state for drainPool.
 * Replaces the previous `TxOutcome | "not_running" | "not_started"`
 * shape so consumers (tests + observability) see a uniform `kind`
 * field across all variants.
 */
export type DrainOutcome =
  /** Loop never executed a batch (initial value, should only appear in DrainResult.lastOutcome if maxBatches=0). */
  | { kind: "not_started" }
  /** Pool reported NothingToRecover / not-found — distribution is not running. */
  | { kind: "not_running" }
  /** An expected on-chain outcome from a successful batch tx. */
  | { kind: "tx"; outcome: TxOutcome }
  /** An unexpected error (RPC failure, deserialization, etc). */
  | { kind: "errored"; detail: string };

/** Per-pool drain result, exposed for observability / testing. */
export interface DrainResult {
  /** True if at least one batch was successfully processed for this pool. */
  madeProgress: boolean;
  /** Number of batches attempted. Capped by maxBatches. */
  batches: number;
  /** True if we saw distribution_complete=true. */
  complete: boolean;
  /** Final state of the inner loop. */
  lastOutcome: DrainOutcome;
}

/**
 * Drain one pool by calling ContinueDistribution repeatedly until
 * distribution_complete=true or a non-progress outcome.
 *
 * maxBatches is a safety valve — we never loop infinitely on one pool
 * within one iteration even if the chain misbehaves.
 */
export async function drainPool(
  executor: Executor,
  poolAddress: string,
  perCallDelayMs: number,
  maxBatches: number = 200,
): Promise<DrainResult> {
  let madeProgress = false;
  let complete = false;
  let batches = 0;
  let lastOutcome: DrainOutcome = { kind: "not_started" };

  for (let batch = 0; batch < maxBatches; batch++) {
    let outcome: TxOutcome;

    try {
      const tx = await executor.execute(poolAddress, PoolExecContinueDistribution);
      outcome = classifyBountyTx(tx);
      complete = isDistributionComplete(tx);
      batches++;
      lastOutcome = { kind: "tx", outcome };

      switch (outcome.kind) {
        case "paid":
          madeProgress = true;
          log.info("distribution batch paid", {
            pool: poolAddress,
            batch,
            tx: tx.transactionHash,
            bounty_usd: outcome.bountyUsd,
            bounty_bluechip: outcome.bountyBluechip,
            complete,
          });
          break;
        case "ok":
          madeProgress = true;
          log.info("distribution batch processed, no bounty configured", {
            pool: poolAddress,
            batch,
            tx: tx.transactionHash,
            complete,
          });
          break;
        case "skipped":
          // Bounty skipped but distribution still progressed this batch.
          // We stop the inner loop because something needs operator
          // attention (factory underfunded, price unavailable, etc).
          madeProgress = true;
          log.warn("distribution batch, bounty skipped", {
            pool: poolAddress,
            batch,
            tx: tx.transactionHash,
            reason: outcome.reason,
            complete,
          });
          break;
        case "failed":
          log.error("distribution batch tx failed", {
            pool: poolAddress,
            batch,
            rawLog: outcome.rawLog,
          });
          break;
      }
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      if (isExpectedSkipError(detail)) {
        log.info("no distribution running", { pool: poolAddress, detail });
        lastOutcome = { kind: "not_running" };
      } else {
        log.error("distribution call errored", { pool: poolAddress, detail });
        lastOutcome = { kind: "errored", detail };
      }
      return { madeProgress, batches, complete, lastOutcome };
    }

    if (!shouldContinueSamePool(outcome, complete)) {
      break;
    }
    if (perCallDelayMs > 0) {
      await sleep(perCallDelayMs);
    }
  }

  return { madeProgress, batches, complete, lastOutcome };
}

/**
 * Run a full sweep of all configured pools.
 */
export async function runDistributionSweep(
  executor: Executor,
  poolAddresses: ReadonlyArray<string>,
  perCallDelayMs: number,
): Promise<{ madeProgress: boolean; pools: Record<string, DrainResult> }> {
  const pools: Record<string, DrainResult> = {};
  let madeProgress = false;
  for (const pool of poolAddresses) {
    const result = await drainPool(executor, pool, perCallDelayMs);
    pools[pool] = result;
    if (result.madeProgress) madeProgress = true;
  }
  return { madeProgress, pools };
}
