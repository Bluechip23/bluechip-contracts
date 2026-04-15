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

/** Per-pool drain result, exposed for observability / testing. */
export interface DrainResult {
  /** True if at least one batch was successfully processed for this pool. */
  madeProgress: boolean;
  /** Number of batches attempted. Capped by maxBatches. */
  batches: number;
  /** True if we saw distribution_complete=true. */
  complete: boolean;
  /** Last outcome observed — useful in tests. */
  lastOutcome: TxOutcome | "not_running" | "not_started";
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
  let lastOutcome: TxOutcome | "not_running" | "not_started" = "not_started";

  for (let batch = 0; batch < maxBatches; batch++) {
    let outcome: TxOutcome;

    try {
      const tx = await executor.execute(poolAddress, PoolExecContinueDistribution);
      outcome = classifyBountyTx(tx);
      complete = isDistributionComplete(tx);
      batches++;
      lastOutcome = outcome;

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
          // A skipped bounty with an otherwise-successful tx still
          // means distribution progressed this batch — but we stop
          // the inner loop because something needs operator attention
          // (factory underfunded, price unavailable, etc).
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
      const msg = err instanceof Error ? err.message : String(err);
      if (isExpectedSkipError(msg)) {
        // Distribution not running for this pool. Normal case — many
        // pools in the config list will not be distributing right now.
        log.info("no distribution running", { pool: poolAddress, detail: msg });
        lastOutcome = "not_running";
      } else {
        log.error("distribution call errored", { pool: poolAddress, detail: msg });
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
