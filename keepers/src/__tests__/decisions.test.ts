import { describe, expect, it } from "vitest";
import {
  classifyBountyTx,
  isDistributionComplete,
  nextDistributionSleepMs,
  nextOracleSleepMs,
  readWasmAttribute,
  shouldContinueSamePool,
  type TxResult,
} from "../lib/decisions.js";

// ---------------------------------------------------------------------------
// Tx fixture helpers
// ---------------------------------------------------------------------------

function okTxWithWasmAttrs(attrs: Array<[string, string]>): TxResult {
  return {
    code: 0,
    transactionHash: "DEADBEEF",
    events: [
      {
        type: "wasm",
        attributes: attrs.map(([key, value]) => ({ key, value })),
      },
    ],
  };
}

function failedTx(rawLog: string): TxResult {
  return {
    code: 5,
    transactionHash: "FAILED",
    rawLog,
  };
}

// ---------------------------------------------------------------------------
// readWasmAttribute
// ---------------------------------------------------------------------------

describe("readWasmAttribute", () => {
  it("returns the value for a matching wasm event attribute", () => {
    const tx = okTxWithWasmAttrs([["bounty_paid_usd", "5000"]]);
    expect(readWasmAttribute(tx, "bounty_paid_usd")).toBe("5000");
  });

  it("returns undefined when the key isn't present", () => {
    const tx = okTxWithWasmAttrs([["action", "update_oracle"]]);
    expect(readWasmAttribute(tx, "bounty_paid_usd")).toBeUndefined();
  });

  it("returns undefined when events array is absent", () => {
    const tx: TxResult = { code: 0, transactionHash: "X" };
    expect(readWasmAttribute(tx, "anything")).toBeUndefined();
  });

  it("ignores non-wasm event types", () => {
    const tx: TxResult = {
      code: 0,
      transactionHash: "X",
      events: [
        {
          type: "transfer",
          attributes: [{ key: "bounty_paid_usd", value: "nope" }],
        },
      ],
    };
    expect(readWasmAttribute(tx, "bounty_paid_usd")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// classifyBountyTx
// ---------------------------------------------------------------------------

describe("classifyBountyTx", () => {
  it("classifies a paid tx with both USD and bluechip amounts", () => {
    const tx = okTxWithWasmAttrs([
      ["action", "update_oracle"],
      ["bounty_paid_usd", "5000"],
      ["bounty_paid_bluechip", "50000"],
    ]);
    expect(classifyBountyTx(tx)).toEqual({
      kind: "paid",
      bountyUsd: "5000",
      bountyBluechip: "50000",
    });
  });

  it("classifies an insufficient-balance skip", () => {
    const tx = okTxWithWasmAttrs([
      ["action", "update_oracle"],
      ["bounty_skipped", "insufficient_factory_balance"],
    ]);
    expect(classifyBountyTx(tx)).toEqual({
      kind: "skipped",
      reason: "insufficient_factory_balance",
    });
  });

  it("classifies a price-unavailable skip", () => {
    const tx = okTxWithWasmAttrs([
      ["bounty_skipped", "price_unavailable"],
    ]);
    expect(classifyBountyTx(tx)).toEqual({
      kind: "skipped",
      reason: "price_unavailable",
    });
  });

  it("classifies an ok tx with no bounty attributes (bounty disabled)", () => {
    const tx = okTxWithWasmAttrs([["action", "update_oracle"]]);
    expect(classifyBountyTx(tx)).toEqual({ kind: "ok" });
  });

  it("classifies a failed tx", () => {
    const tx = failedTx("out of gas");
    expect(classifyBountyTx(tx)).toEqual({ kind: "failed", rawLog: "out of gas" });
  });

  it("prefers paid over skipped if both somehow appear", () => {
    // Shouldn't happen in practice — asserts paid wins if it does.
    const tx = okTxWithWasmAttrs([
      ["bounty_paid_usd", "5000"],
      ["bounty_paid_bluechip", "50000"],
      ["bounty_skipped", "some_reason"],
    ]);
    expect(classifyBountyTx(tx).kind).toBe("paid");
  });
});

// ---------------------------------------------------------------------------
// Sleep heuristics
// ---------------------------------------------------------------------------

describe("nextOracleSleepMs", () => {
  it("adds jitter on top of the base interval", () => {
    // random() => 0.5 → jitter = 2500 (for default jitterMs = 5000)
    const sleep = nextOracleSleepMs(300_000, 5_000, () => 0.5);
    expect(sleep).toBe(300_000 + 2_500);
  });

  it("adds zero jitter when random is 0", () => {
    const sleep = nextOracleSleepMs(300_000, 5_000, () => 0);
    expect(sleep).toBe(300_000);
  });

  it("returns 0 for non-positive base interval", () => {
    expect(nextOracleSleepMs(0)).toBe(0);
    expect(nextOracleSleepMs(-1)).toBe(0);
  });
});

describe("nextDistributionSleepMs", () => {
  it("polls quickly after making progress", () => {
    const sleep = nextDistributionSleepMs(1_800_000, true, 15_000);
    expect(sleep).toBe(15_000);
  });

  it("polls at full interval when idle", () => {
    const sleep = nextDistributionSleepMs(1_800_000, false, 15_000);
    expect(sleep).toBe(1_800_000);
  });

  it("clamps fast-poll to base if base is smaller", () => {
    const sleep = nextDistributionSleepMs(5_000, true, 15_000);
    expect(sleep).toBe(5_000);
  });
});

// ---------------------------------------------------------------------------
// isDistributionComplete
// ---------------------------------------------------------------------------

describe("isDistributionComplete", () => {
  it("returns true when attribute is 'true'", () => {
    const tx = okTxWithWasmAttrs([["distribution_complete", "true"]]);
    expect(isDistributionComplete(tx)).toBe(true);
  });

  it("returns false when attribute is 'false'", () => {
    const tx = okTxWithWasmAttrs([["distribution_complete", "false"]]);
    expect(isDistributionComplete(tx)).toBe(false);
  });

  it("returns false when attribute is absent", () => {
    const tx = okTxWithWasmAttrs([["action", "continue_distribution"]]);
    expect(isDistributionComplete(tx)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// shouldContinueSamePool
// ---------------------------------------------------------------------------

describe("shouldContinueSamePool", () => {
  it("stops when distribution is complete even on a paid outcome", () => {
    const paid: ReturnType<typeof classifyBountyTx> = {
      kind: "paid",
      bountyUsd: "5000",
      bountyBluechip: "5000",
    };
    expect(shouldContinueSamePool(paid, true)).toBe(false);
  });

  it("continues on paid + incomplete", () => {
    const paid: ReturnType<typeof classifyBountyTx> = {
      kind: "paid",
      bountyUsd: "5000",
      bountyBluechip: "5000",
    };
    expect(shouldContinueSamePool(paid, false)).toBe(true);
  });

  it("continues on ok + incomplete", () => {
    expect(shouldContinueSamePool({ kind: "ok" }, false)).toBe(true);
  });

  it("stops on skipped outcomes (likely funding issue on factory)", () => {
    expect(
      shouldContinueSamePool(
        { kind: "skipped", reason: "insufficient_factory_balance" },
        false,
      ),
    ).toBe(false);
  });

  it("stops on failed outcomes", () => {
    expect(
      shouldContinueSamePool({ kind: "failed", rawLog: "x" }, false),
    ).toBe(false);
  });
});
