import { beforeEach, describe, expect, it } from "vitest";
import { runOracleIteration } from "../lib/oracle-loop.js";
import { MockContracts } from "./mock-contracts.js";

const FACTORY = "bluechip1factory";
const KEEPER = "bluechip1keeper";

/** Mutable clock shared with the mock. */
function makeClock(initialMs: number = 1_700_000_000_000): { get: () => number; advance: (ms: number) => void } {
  let t = initialMs;
  return {
    get: () => t,
    advance: (ms: number) => {
      t += ms;
    },
  };
}

describe("oracle keeper integration", () => {
  let clock: ReturnType<typeof makeClock>;
  let mock: MockContracts;

  beforeEach(() => {
    clock = makeClock();
    mock = new MockContracts(KEEPER, {
      now: () => clock.get(),
      factoryAddress: FACTORY,
      initialFactoryBalance: 10_000_000n, // 10 bluechip
      oracleBountyUsd: 5_000n, // $0.005
      // With bluechipPerUsd = 1_000_000, $0.005 pays out 5 bluechip... wait:
      // bluechip = usd * bluechipPerUsd / 1_000_000 = 5_000 * 1_000_000 / 1_000_000 = 5_000 ubluechip
    });
  });

  it("keeper actually gets paid on a successful update", async () => {
    const factoryBefore = mock.getFactoryBalance();
    const keeperBalanceBefore = await mock.getBalance("ubluechip");

    const result = await runOracleIteration(mock, FACTORY);

    expect(result.kind).toBe("outcome");
    if (result.kind !== "outcome") return;
    expect(result.outcome.kind).toBe("paid");
    if (result.outcome.kind !== "paid") return;

    // Exact-value assertion: $0.005 USD at 1 bluechip/$1 = 5000 ubluechip.
    expect(result.outcome.bountyUsd).toBe("5000");
    expect(result.outcome.bountyBluechip).toBe("5000");

    // Factory balance went DOWN by exactly the bounty.
    expect(mock.getFactoryBalance()).toBe(factoryBefore - 5_000n);

    // Keeper balance went UP by exactly the bounty.
    const keeperBalanceAfter = await mock.getBalance("ubluechip");
    expect(keeperBalanceAfter - keeperBalanceBefore).toBe(5_000n);
  });

  it("second call inside the 5-minute cooldown is rejected", async () => {
    const first = await runOracleIteration(mock, FACTORY);
    expect(first.kind).toBe("outcome");

    // Immediately try again — no clock advance.
    const second = await runOracleIteration(mock, FACTORY);
    expect(second.kind).toBe("cooldown");
    if (second.kind !== "cooldown") return;
    expect(second.detail).toContain("UpdateTooSoon");
  });

  it("second call after the cooldown window succeeds", async () => {
    await runOracleIteration(mock, FACTORY);

    clock.advance(5 * 60 * 1000 + 1);
    const second = await runOracleIteration(mock, FACTORY);

    expect(second.kind).toBe("outcome");
    if (second.kind !== "outcome") return;
    expect(second.outcome.kind).toBe("paid");
  });

  it("three rapid calls only pay out once (abuse protection)", async () => {
    const r1 = await runOracleIteration(mock, FACTORY);
    const r2 = await runOracleIteration(mock, FACTORY);
    const r3 = await runOracleIteration(mock, FACTORY);

    const paid = [r1, r2, r3].filter(
      (r) => r.kind === "outcome" && r.outcome.kind === "paid",
    );
    expect(paid).toHaveLength(1);

    const cooldown = [r1, r2, r3].filter((r) => r.kind === "cooldown");
    expect(cooldown).toHaveLength(2);

    // Factory paid out exactly one bounty worth.
    expect(mock.getFactoryBalance()).toBe(10_000_000n - 5_000n);
  });

  it("sustained polling over 20 min pays exactly the expected number of times", async () => {
    // 20 minutes of simulated wall time. With a 5-min cooldown, the
    // keeper should collect exactly 4 bounties (at t=0, t=5min, t=10min,
    // t=15min). A keeper checking every minute would attempt 20 times
    // but only 4 should succeed — confirming the cooldown does real work.
    let paid = 0;
    let cooldowns = 0;
    const pollInterval = 60 * 1000; // 1 min
    for (let i = 0; i < 20; i++) {
      const r = await runOracleIteration(mock, FACTORY);
      if (r.kind === "outcome" && r.outcome.kind === "paid") paid++;
      if (r.kind === "cooldown") cooldowns++;
      clock.advance(pollInterval);
    }
    expect(paid).toBe(4);
    expect(cooldowns).toBe(16);
    // Factory balance: started at 10M, paid out 4 × 5000 = 20_000.
    expect(mock.getFactoryBalance()).toBe(10_000_000n - 20_000n);
  });

  it("skip-on-underfund: factory balance below bounty → skip, no payment", async () => {
    mock.setFactoryBalance(100n); // way below 5000 ubluechip needed
    const keeperBefore = await mock.getBalance("ubluechip");

    const result = await runOracleIteration(mock, FACTORY);

    expect(result.kind).toBe("outcome");
    if (result.kind !== "outcome") return;
    expect(result.outcome.kind).toBe("skipped");
    if (result.outcome.kind !== "skipped") return;
    expect(result.outcome.reason).toBe("insufficient_factory_balance");

    // No bluechip moved.
    expect(mock.getFactoryBalance()).toBe(100n);
    expect(await mock.getBalance("ubluechip")).toBe(keeperBefore);
  });

  it("skip-on-oracle-unavailable: keeper still gets the oracle updated, no payout", async () => {
    mock.starveOracle();
    const keeperBefore = await mock.getBalance("ubluechip");

    const result = await runOracleIteration(mock, FACTORY);

    expect(result.kind).toBe("outcome");
    if (result.kind !== "outcome") return;
    expect(result.outcome.kind).toBe("skipped");
    if (result.outcome.kind !== "skipped") return;
    expect(result.outcome.reason).toBe("price_unavailable");

    expect(await mock.getBalance("ubluechip")).toBe(keeperBefore);
  });

  it("bounty-disabled: keeper tx succeeds but no bounty is paid", async () => {
    mock.setOracleBounty(0n);
    const keeperBefore = await mock.getBalance("ubluechip");

    const result = await runOracleIteration(mock, FACTORY);

    expect(result.kind).toBe("outcome");
    if (result.kind !== "outcome") return;
    expect(result.outcome.kind).toBe("ok");

    expect(await mock.getBalance("ubluechip")).toBe(keeperBefore);
  });
});
