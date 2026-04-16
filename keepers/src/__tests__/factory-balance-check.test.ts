import { beforeEach, describe, expect, it, vi } from "vitest";
import { checkFactoryBalance } from "../lib/oracle-loop.js";
import { MockContracts } from "./mock-contracts.js";
import { log } from "../lib/logger.js";

const FACTORY = "bluechip1factory";
const KEEPER = "bluechip1keeper";

describe("checkFactoryBalance", () => {
  let mock: MockContracts;
  // vi.spyOn's typed return doesn't simplify cleanly across vitest versions;
  // we just need the spy to capture calls + suppress output.
  let warnSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mock = new MockContracts(KEEPER, {
      now: () => 1_700_000_000_000,
      factoryAddress: FACTORY,
      initialFactoryBalance: 50_000n,
    });
    // Silence + capture log warnings.
    warnSpy = vi.fn();
    vi.spyOn(log, "warn").mockImplementation(warnSpy);
  });

  it("warns when factory balance is below the threshold", async () => {
    await checkFactoryBalance(mock, FACTORY, "ubluechip", 100_000n);

    expect(warnSpy).toHaveBeenCalledOnce();
    const call = warnSpy.mock.calls[0] as [string, Record<string, unknown>];
    expect(call[0]).toBe("factory bounty reserve below threshold — top up soon");
    expect(call[1]).toMatchObject({
      factory: FACTORY,
      balance: "50000",
      threshold: "100000",
    });
  });

  it("stays silent when factory balance is at or above the threshold", async () => {
    mock.setFactoryBalance(100_000n);

    await checkFactoryBalance(mock, FACTORY, "ubluechip", 100_000n);

    expect(warnSpy).not.toHaveBeenCalled();
  });

  it("downgrades a query failure to a warning, never throws", async () => {
    // Patch the executor to make getAddressBalance throw.
    const broken = {
      ...mock,
      address: KEEPER,
      execute: mock.execute.bind(mock),
      getBalance: mock.getBalance.bind(mock),
      getAddressBalance: vi.fn().mockRejectedValue(new Error("rpc down")),
    };

    await expect(
      checkFactoryBalance(broken as any, FACTORY, "ubluechip", 100_000n),
    ).resolves.toBeUndefined();

    expect(warnSpy).toHaveBeenCalledOnce();
    const call = warnSpy.mock.calls[0] as [string, Record<string, unknown>];
    expect(call[0]).toBe("factory balance check failed");
    expect(call[1]).toMatchObject({
      factory: FACTORY,
      detail: "rpc down",
    });
  });
});
