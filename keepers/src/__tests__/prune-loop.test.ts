import { beforeEach, describe, expect, it } from "vitest";
import { MockContracts } from "./mock-contracts.js";
import { runPruneIteration } from "../lib/prune-loop.js";

const FACTORY = "bluechip1factory";
const KEEPER = "bluechip1keeper";

function makeClock() {
  return { get: () => 1_700_000_000_000 };
}

describe("prune-loop iteration", () => {
  let mock: MockContracts;

  beforeEach(() => {
    mock = new MockContracts(KEEPER, {
      now: makeClock().get,
      factoryAddress: FACTORY,
    });
  });

  it("dispatches PruneRateLimits with the configured batch_size", async () => {
    const outcome = await runPruneIteration(mock, FACTORY, 250);

    expect(outcome.kind).toBe("pruned");
    if (outcome.kind === "pruned") {
      expect(outcome.txHash).toMatch(/^TX/);
    }
    expect(mock.pruneCalls).toHaveLength(1);
    expect(mock.pruneCalls[0]?.batchSize).toBe(250);
  });

  it("parses commit_pruned + standard_pruned from response events", async () => {
    mock.setNextPruneCounters(7, 3);

    const outcome = await runPruneIteration(mock, FACTORY, 100);

    expect(outcome.kind).toBe("pruned");
    if (outcome.kind === "pruned") {
      expect(outcome.commitPruned).toBe(7);
      expect(outcome.standardPruned).toBe(3);
    }
  });

  it("steady-state sweep returns zero counters without erroring", async () => {
    // Most calls hit the steady state where there's nothing to prune.
    // The keeper must treat "0 pruned" as a successful no-op, NOT as an
    // error condition that warrants escalation.
    const outcome = await runPruneIteration(mock, FACTORY, 100);
    expect(outcome.kind).toBe("pruned");
    if (outcome.kind === "pruned") {
      expect(outcome.commitPruned).toBe(0);
      expect(outcome.standardPruned).toBe(0);
    }
  });

  it("survives an unexpected RPC error without throwing", async () => {
    // Force the next factory execute to throw with a non-skip error.
    mock.failNextExecute(FACTORY);

    const outcome = await runPruneIteration(mock, FACTORY, 100);

    // Critical: the prune sweep must NEVER bubble up — a failed prune
    // must not break the oracle keeper's main loop. The implementation
    // returns `errored` (logged at warn level) rather than throwing.
    expect(outcome.kind).toBe("errored");
    if (outcome.kind === "errored") {
      expect(outcome.detail).toContain("forced failure");
    }
  });
});
