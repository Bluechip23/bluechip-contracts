import { beforeEach, describe, expect, it } from "vitest";
import { drainPool, runDistributionSweep } from "../lib/distribution-loop.js";
import { MockContracts } from "./mock-contracts.js";

const FACTORY = "bluechip1factory";
const POOL_A = "bluechip1poolA";
const POOL_B = "bluechip1poolB";
const POOL_C = "bluechip1poolC";
const KEEPER = "bluechip1keeper";

function makeClock(initialMs: number = 1_700_000_000_000) {
  let t = initialMs;
  return {
    get: () => t,
    advance: (ms: number) => {
      t += ms;
    },
  };
}

describe("distribution keeper integration", () => {
  let clock: ReturnType<typeof makeClock>;
  let mock: MockContracts;

  beforeEach(() => {
    clock = makeClock();
    mock = new MockContracts(KEEPER, {
      now: () => clock.get(),
      factoryAddress: FACTORY,
      initialFactoryBalance: 10_000_000_000n, // 10k bluechip
      distributionBountyUsd: 50_000n, // $0.05
      bluechipPerUsd: 1_000_000n, // 1 bluechip = $1
    });
  });

  it("drains an actively distributing pool across multiple batches", async () => {
    // Pool has 5 batches of work queued.
    mock.setupPoolDistribution(POOL_A, 5);
    const factoryBefore = mock.getFactoryBalance();
    const keeperBefore = await mock.getBalance("ubluechip");

    const result = await drainPool(mock, POOL_A, /* perCallDelayMs */ 0);

    expect(result.madeProgress).toBe(true);
    expect(result.batches).toBe(5);
    expect(result.complete).toBe(true);

    // $0.05 × 5 batches = $0.25 USD = 50_000 × 5 = 250_000 ubluechip.
    const expectedPayout = 250_000n;
    expect(mock.getFactoryBalance()).toBe(factoryBefore - expectedPayout);
    const keeperAfter = await mock.getBalance("ubluechip");
    expect(keeperAfter - keeperBefore).toBe(expectedPayout);
  });

  it("returns early when a pool is not distributing (NothingToRecover)", async () => {
    // No setup — pool is not in distribution state.
    const factoryBefore = mock.getFactoryBalance();

    const result = await drainPool(mock, POOL_A, 0);

    expect(result.madeProgress).toBe(false);
    expect(result.batches).toBe(0);
    expect(result.complete).toBe(false);
    expect(result.lastOutcome).toBe("not_running");
    expect(mock.getFactoryBalance()).toBe(factoryBefore);
  });

  it("exactly one bounty per batch (no abuse)", async () => {
    mock.setupPoolDistribution(POOL_A, 10);
    const result = await drainPool(mock, POOL_A, 0);

    expect(result.batches).toBe(10);
    // 10 × $0.05 = $0.50 → 500_000 ubluechip exactly.
    expect(mock.getFactoryBalance()).toBe(10_000_000_000n - 500_000n);
    expect(await mock.getBalance("ubluechip")).toBe(1_000_000_000n + 500_000n);
  });

  it("sweep skips non-distributing pools, drains the one that is", async () => {
    mock.setupPoolDistribution(POOL_B, 3);
    // POOL_A and POOL_C are not distributing.
    const pools = [POOL_A, POOL_B, POOL_C];

    const sweep = await runDistributionSweep(mock, pools, 0);

    expect(sweep.madeProgress).toBe(true);
    expect(sweep.pools[POOL_A]?.lastOutcome).toBe("not_running");
    expect(sweep.pools[POOL_B]?.batches).toBe(3);
    expect(sweep.pools[POOL_B]?.complete).toBe(true);
    expect(sweep.pools[POOL_C]?.lastOutcome).toBe("not_running");
  });

  it("sweep handles multiple distributing pools in one pass", async () => {
    mock.setupPoolDistribution(POOL_A, 2);
    mock.setupPoolDistribution(POOL_B, 4);
    const pools = [POOL_A, POOL_B];

    const sweep = await runDistributionSweep(mock, pools, 0);

    expect(sweep.madeProgress).toBe(true);
    expect(sweep.pools[POOL_A]?.batches).toBe(2);
    expect(sweep.pools[POOL_B]?.batches).toBe(4);
    // Total bounty: (2+4) × 50_000 = 300_000 ubluechip
    expect(mock.getFactoryBalance()).toBe(10_000_000_000n - 300_000n);
  });

  it("honors the inner-loop safety cap (maxBatches)", async () => {
    // Pool has 1000 batches queued, but we pass maxBatches=50.
    mock.setupPoolDistribution(POOL_A, 1000);

    const result = await drainPool(mock, POOL_A, 0, /* maxBatches */ 50);

    expect(result.batches).toBe(50);
    expect(result.complete).toBe(false); // we bailed early
    expect(result.madeProgress).toBe(true);
  });

  it("stops the inner loop when bounty is skipped (factory underfunded)", async () => {
    mock.setupPoolDistribution(POOL_A, 10);
    // Factory too poor to pay even one bounty.
    mock.setFactoryBalance(100n);

    const result = await drainPool(mock, POOL_A, 0);

    // shouldContinueSamePool stops on skipped outcomes, so we process
    // exactly one batch and exit.
    expect(result.batches).toBe(1);
    expect(result.lastOutcome).toEqual({
      kind: "skipped",
      reason: "insufficient_factory_balance",
    });
    // The batch still processed (distribution progressed) but no bounty.
    expect(result.madeProgress).toBe(true);
  });

  it("stops the inner loop on oracle starvation (Pyth outage)", async () => {
    mock.setupPoolDistribution(POOL_A, 10);
    mock.starveOracle();

    const result = await drainPool(mock, POOL_A, 0);

    expect(result.batches).toBe(1);
    expect(result.lastOutcome).toEqual({
      kind: "skipped",
      reason: "price_unavailable",
    });
  });

  it("rejects unregistered pools (auth boundary)", async () => {
    mock.setupPoolDistribution(POOL_A, 3);
    // Factory no longer sees POOL_A in POOLS_BY_CONTRACT_ADDRESS.
    mock.deregisterPool(POOL_A);

    const result = await drainPool(mock, POOL_A, 0);

    expect(result.madeProgress).toBe(false);
    expect(result.batches).toBe(0);
    // The error path classifies this as a non-skip error since
    // "Unauthorized" doesn't match our expected-skip markers.
    expect(result.lastOutcome).toBe("not_started");
    // No balance movement.
    expect(mock.getFactoryBalance()).toBe(10_000_000_000n);
  });

  it("end-to-end: full sweep → complete → idempotent re-sweep", async () => {
    mock.setupPoolDistribution(POOL_A, 5);

    // First sweep drains the pool.
    const first = await runDistributionSweep(mock, [POOL_A], 0);
    expect(first.madeProgress).toBe(true);
    expect(first.pools[POOL_A]?.complete).toBe(true);

    // Second sweep: pool is no longer distributing, no-op.
    const second = await runDistributionSweep(mock, [POOL_A], 0);
    expect(second.madeProgress).toBe(false);
    expect(second.pools[POOL_A]?.lastOutcome).toBe("not_running");

    // Exactly 5 bounties paid total.
    expect(mock.getFactoryBalance()).toBe(10_000_000_000n - 250_000n);
  });
});
