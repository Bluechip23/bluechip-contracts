import { beforeEach, describe, expect, it } from "vitest";
import { runOracleIteration } from "../lib/oracle-loop.js";
import { MockContracts } from "./mock-contracts.js";

const FACTORY = "bluechip1factory";
const KEEPER = "bluechip1keeper";
const MOCK_ORACLE = "bluechip1mockoracle";

function makeClock(initialMs: number = 1_700_000_000_000) {
  let t = initialMs;
  return {
    get: () => t,
    advance: (ms: number) => {
      t += ms;
    },
  };
}

describe("oracle keeper mock-price push", () => {
  let clock: ReturnType<typeof makeClock>;
  let mock: MockContracts;

  beforeEach(() => {
    clock = makeClock();
    mock = new MockContracts(KEEPER, {
      now: () => clock.get(),
      factoryAddress: FACTORY,
      initialFactoryBalance: 10_000_000n,
      oracleBountyUsd: 5_000n,
    });
  });

  it("pushes SetPrice before UpdateOraclePrice when mockPush is configured", async () => {
    const result = await runOracleIteration(mock, FACTORY, {
      oracleAddress: MOCK_ORACLE,
      feedId: "BLUECHIP_USD",
      priceUbluechip: "1000000",
    });

    // Two calls in order: SetPrice on mock oracle, then UpdateOraclePrice.
    expect(mock.calls).toEqual([
      {
        contract: MOCK_ORACLE,
        msg: {
          set_price: { price_id: "BLUECHIP_USD", price: "1000000" },
        },
      },
      {
        contract: FACTORY,
        msg: { update_oracle_price: {} },
      },
    ]);

    // UpdateOraclePrice still succeeds.
    expect(result.kind).toBe("outcome");
  });

  it("does NOT push SetPrice when mockPush is undefined", async () => {
    await runOracleIteration(mock, FACTORY);

    expect(mock.calls).toEqual([
      { contract: FACTORY, msg: { update_oracle_price: {} } },
    ]);
  });

  it("continues to UpdateOraclePrice when the SetPrice push fails", async () => {
    mock.failNextExecute(MOCK_ORACLE);

    const result = await runOracleIteration(mock, FACTORY, {
      oracleAddress: MOCK_ORACLE,
      feedId: "BLUECHIP_USD",
      priceUbluechip: "1000000",
    });

    // Both calls were attempted — the failed SetPrice didn't block the main tx.
    expect(mock.calls.map((c) => c.contract)).toEqual([MOCK_ORACLE, FACTORY]);
    expect(result.kind).toBe("outcome");
  });
});
