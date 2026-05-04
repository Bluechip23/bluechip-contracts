// Retry-factory-notify keeper.
//
// Every commit pool, on threshold-cross, dispatches a NotifyThresholdCrossed
// SubMsg to the factory using `reply_on_error`. If the factory rejects
// (e.g., expand-economy reservoir is empty AND skip-with-Ok was configured
// off, or some transient factory bug), the pool sets PENDING_FACTORY_NOTIFY
// = true and the threshold-mint reward is held until somebody calls
// pool.RetryFactoryNotify {}.
//
// The contract handler is permissionless on purpose — anyone can settle
// the stuck mint. We add a keeper that polls each pool's
// `FactoryNotifyStatus` query and dispatches RetryFactoryNotify only on
// pools that report `pending = true`. Querying first avoids blasting
// every pool every round (most pools have nothing pending most of the
// time) so this loop is dirt-cheap on RPC.
//
// No bounty exists for this action — the contract doesn't pay one and
// we don't need one (the keeper that runs this is the same operator
// running the oracle/distribution keepers, who already absorbs the gas
// cost of "running keepers" as part of running the protocol).
//
// Folded into the distribution-keeper process (rather than running as a
// third long-lived bot) because:
//   - It iterates the same POOL_ADDRESSES list.
//   - Cadence (~ once per distribution sweep, every 30 min) is plenty:
//     the failure mode it recovers from is rare and not time-sensitive,
//     and 30-min-stale recovery is fine vs the operator-attention
//     alternative of "wait for an alert and manually retry."

import type { Executor } from "./executor.js";
import {
  PoolExecRetryFactoryNotify,
  PoolQueryFactoryNotifyStatus,
  isExpectedSkipError,
  type FactoryNotifyStatusResponse,
} from "./types.js";
import { log } from "./logger.js";

/** Per-pool outcome of one retry attempt. */
export type RetryNotifyOutcome =
  /** Query said `pending = false`; we did nothing. */
  | { kind: "skipped"; pool: string; reason: "not_pending" }
  /** Query said `pending = true`; tx succeeded. */
  | { kind: "retried"; pool: string; txHash: string }
  /**
   * Tx errored with one of the expected skip markers (e.g., the pool
   * reports "No pending factory notification to retry" because state
   * changed between query and tx, or the factory reports
   * "Bluechip mint already triggered" because the pending flag was
   * stale). Treated as a clean no-op.
   */
  | { kind: "skipped"; pool: string; reason: "tx_skip"; detail: string }
  /** Query failed (RPC issue, malformed response). Pool is left as-is. */
  | { kind: "query_failed"; pool: string; detail: string }
  /** Tx failed with an unexpected error. Operator action may be needed. */
  | { kind: "errored"; pool: string; detail: string };

/**
 * Run the retry-notify check + dispatch for one pool.
 *
 * Two-phase: query first, only execute if the query says pending=true.
 * This keeps the cost asymptotically O(N pools) queries per sweep
 * rather than O(N) txs — the latter would cost real gas on every pool
 * every round even when nothing needs doing.
 */
export async function checkAndRetryPool(
  executor: Executor,
  poolAddress: string,
): Promise<RetryNotifyOutcome> {
  let status: FactoryNotifyStatusResponse;
  try {
    status = await executor.queryContractSmart<FactoryNotifyStatusResponse>(
      poolAddress,
      PoolQueryFactoryNotifyStatus,
    );
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    // Don't escalate query failures — a single pool's RPC blip
    // shouldn't break the sweep. Log warn and move on.
    log.warn("factory_notify_status query failed", { pool: poolAddress, detail });
    return { kind: "query_failed", pool: poolAddress, detail };
  }

  if (!status.pending) {
    return { kind: "skipped", pool: poolAddress, reason: "not_pending" };
  }

  log.info("pending factory notify detected; retrying", { pool: poolAddress });

  try {
    const tx = await executor.execute(poolAddress, PoolExecRetryFactoryNotify);
    log.info("retry_factory_notify dispatched", {
      pool: poolAddress,
      tx: tx.transactionHash,
    });
    return { kind: "retried", pool: poolAddress, txHash: tx.transactionHash };
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    if (isExpectedSkipError(detail)) {
      log.info("retry_factory_notify skipped (state changed or already minted)", {
        pool: poolAddress,
        detail,
      });
      return { kind: "skipped", pool: poolAddress, reason: "tx_skip", detail };
    }
    log.error("retry_factory_notify errored", { pool: poolAddress, detail });
    return { kind: "errored", pool: poolAddress, detail };
  }
}

/**
 * Aggregate result of a sweep across every configured pool.
 * Exposed for observability + tests so callers can assert what
 * happened without re-iterating the per-pool log lines.
 */
export interface RetryNotifySweepResult {
  /** Per-pool outcomes in input order. */
  outcomes: RetryNotifyOutcome[];
  /** Convenience counters for at-a-glance dashboards. */
  totals: {
    retried: number;
    skipped: number;
    queryFailed: number;
    errored: number;
  };
}

/**
 * One sweep across every configured pool. Outcomes are independent —
 * a single pool's failure does not stop the sweep.
 */
export async function runRetryNotifySweep(
  executor: Executor,
  poolAddresses: ReadonlyArray<string>,
): Promise<RetryNotifySweepResult> {
  const outcomes: RetryNotifyOutcome[] = [];
  const totals = { retried: 0, skipped: 0, queryFailed: 0, errored: 0 };

  for (const pool of poolAddresses) {
    const outcome = await checkAndRetryPool(executor, pool);
    outcomes.push(outcome);
    switch (outcome.kind) {
      case "retried":
        totals.retried += 1;
        break;
      case "skipped":
        totals.skipped += 1;
        break;
      case "query_failed":
        totals.queryFailed += 1;
        break;
      case "errored":
        totals.errored += 1;
        break;
    }
  }

  if (totals.retried > 0 || totals.errored > 0) {
    // Logger fields are flat (string | number | boolean), so flatten
    // the totals struct rather than passing it as a nested object —
    // keeps the per-line JSON aggregator-friendly.
    log.info("retry-notify sweep complete", {
      retried: totals.retried,
      skipped: totals.skipped,
      query_failed: totals.queryFailed,
      errored: totals.errored,
    });
  }
  return { outcomes, totals };
}
