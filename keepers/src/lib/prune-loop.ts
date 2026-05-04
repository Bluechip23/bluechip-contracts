// PruneRateLimits sweep — folded into the oracle keeper.
//
// Background: the factory's `LAST_COMMIT_POOL_CREATE_AT` and
// `LAST_STANDARD_POOL_CREATE_AT` maps are keyed on `info.sender`. They
// grow by one entry every successful Create / CreateStandardPool call
// and never shrink on their own. Over years of operation this becomes
// soft storage bloat — entries from addresses that created one pool
// and disappeared.
//
// `factory.PruneRateLimits { batch_size }` is permissionless and removes
// entries older than 10× the cooldown window (so 10h today). We don't
// run it as its own bot because:
//   - It has no bounty (no economic incentive to spin up a dedicated process).
//   - Cadence is wildly relaxed (daily would be plenty).
//   - The oracle keeper already runs every ~5 min and is the natural
//     place to tack on a low-frequency sweep.
//
// We dispatch the prune once every `ORACLE_PRUNE_EVERY_N` iterations of
// the oracle keeper. With the default 5.5min poll * 200 = ~18h, the
// sweep fires roughly once a day per process.

import type { Executor } from "./executor.js";
import { factoryExecPruneRateLimits, isExpectedSkipError } from "./types.js";
import { log } from "./logger.js";

/**
 * Decision shape of one PruneRateLimits attempt.
 *
 * `pruned_*` counts are extracted from the contract's response
 * attributes (`commit_pruned`, `standard_pruned`). They will both be
 * zero on most calls — that's the steady-state when nothing has gone
 * stale yet.
 */
export type PruneOutcome =
  | { kind: "pruned"; txHash: string; commitPruned: number; standardPruned: number }
  | { kind: "skipped"; detail: string }
  | { kind: "errored"; detail: string };

/**
 * One PruneRateLimits dispatch. Returns the outcome for tests /
 * observability.
 *
 * `batchSize` caps the per-call work the contract does (default 100,
 * hard-capped at 500 contract-side); we pass it through so operators
 * can tune for chain-specific gas limits without redeploying.
 */
export async function runPruneIteration(
  executor: Executor,
  factoryAddress: string,
  batchSize: number,
): Promise<PruneOutcome> {
  try {
    const tx = await executor.execute(
      factoryAddress,
      factoryExecPruneRateLimits(batchSize),
    );
    // Extract the contract-emitted counters. The factory's handler
    // always emits these two attributes, so missing values default to
    // zero rather than failing the parse.
    // tx.events is optional in the TxResult shape (some legacy
    // CosmJS responses omit it); default to an empty array so the
    // parse never blows up.
    const { commitPruned, standardPruned } = parsePruneCounters(tx.events ?? []);
    log.info("rate-limit prune complete", {
      tx: tx.transactionHash,
      commit_pruned: commitPruned,
      standard_pruned: standardPruned,
    });
    return {
      kind: "pruned",
      txHash: tx.transactionHash,
      commitPruned,
      standardPruned,
    };
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    if (isExpectedSkipError(detail)) {
      log.info("rate-limit prune skipped", { detail });
      return { kind: "skipped", detail };
    }
    // Don't escalate — a failed prune doesn't break the protocol.
    // Log and continue; next round will retry.
    log.warn("rate-limit prune errored (non-fatal)", { detail });
    return { kind: "errored", detail };
  }
}

/** Internal: scrape `commit_pruned` and `standard_pruned` out of the tx events. */
function parsePruneCounters(
  events: ReadonlyArray<{ type: string; attributes: ReadonlyArray<{ key: string; value: string }> }>,
): { commitPruned: number; standardPruned: number } {
  let commitPruned = 0;
  let standardPruned = 0;
  for (const ev of events) {
    // wasm event carries the contract-emitted attributes in CosmWasm 2.x.
    if (ev.type !== "wasm") continue;
    for (const attr of ev.attributes) {
      if (attr.key === "commit_pruned") {
        const n = Number.parseInt(attr.value, 10);
        if (Number.isFinite(n)) commitPruned = n;
      } else if (attr.key === "standard_pruned") {
        const n = Number.parseInt(attr.value, 10);
        if (Number.isFinite(n)) standardPruned = n;
      }
    }
  }
  return { commitPruned, standardPruned };
}
