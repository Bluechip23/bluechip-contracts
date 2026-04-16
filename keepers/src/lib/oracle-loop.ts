import { classifyBountyTx, type TxOutcome } from "./decisions.js";
import type { Executor } from "./executor.js";
import { FactoryExecUpdateOraclePrice, isExpectedSkipError } from "./types.js";
import { log } from "./logger.js";

/**
 * Result of a single oracle-keeper iteration. Returned so the caller
 * (the loop entrypoint OR a test) can observe what happened.
 */
export type OracleIterationResult =
  | { kind: "outcome"; outcome: TxOutcome; txHash: string }
  | { kind: "cooldown"; detail: string }
  | { kind: "error"; detail: string };

/**
 * One iteration of the oracle keeper. Factored out of the entrypoint
 * so tests can exercise it against a mock Executor without needing a
 * running chain.
 */
/**
 * Mock-oracle price-push config. Set on local/testnet deployments where the
 * factory is built with `--features mock`; the keeper refreshes the mock
 * oracle's BLUECHIP_USD feed before each UpdateOraclePrice call. Unset in
 * production — the production factory derives price from pool TWAPs and
 * does not need a keeper-side push.
 */
export interface MockPricePushConfig {
  oracleAddress: string;
  feedId: string;
  priceUbluechip: string;
}

export async function runOracleIteration(
  executor: Executor,
  factoryAddress: string,
  mockPush?: MockPricePushConfig,
): Promise<OracleIterationResult> {
  if (mockPush) {
    try {
      await executor.execute(mockPush.oracleAddress, {
        set_price: {
          price_id: mockPush.feedId,
          price: mockPush.priceUbluechip,
        },
      });
    } catch (err) {
      // Per design: a failed mock-price push is a warning, not a blocker.
      // UpdateOraclePrice will still read whatever's currently in the mock
      // oracle and attempt the bounty. Losing a single refresh is cheaper
      // than halting keeper progress.
      log.warn("mock price push failed; continuing to UpdateOraclePrice", {
        detail: err instanceof Error ? err.message : String(err),
      });
    }
  }
  try {
    const tx = await executor.execute(factoryAddress, FactoryExecUpdateOraclePrice);
    const outcome = classifyBountyTx(tx);

    switch (outcome.kind) {
      case "paid":
        log.info("oracle updated, bounty paid", {
          tx: tx.transactionHash,
          usd: outcome.bountyUsd,
          bluechip: outcome.bountyBluechip,
        });
        break;
      case "skipped":
        log.warn("oracle updated, bounty skipped", {
          tx: tx.transactionHash,
          reason: outcome.reason,
        });
        break;
      case "ok":
        log.info("oracle updated, no bounty configured", {
          tx: tx.transactionHash,
        });
        break;
      case "failed":
        log.error("oracle update tx failed", { rawLog: outcome.rawLog });
        break;
    }
    return { kind: "outcome", outcome, txHash: tx.transactionHash };
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    if (isExpectedSkipError(detail)) {
      log.info("oracle update skipped (cooldown or beaten)", { detail });
      return { kind: "cooldown", detail };
    }
    log.error("oracle update errored", { detail });
    return { kind: "error", detail };
  }
}

/**
 * Best-effort balance check. Never throws — a failed balance query
 * returns a warning but doesn't break the loop.
 */
export async function checkKeeperBalance(
  executor: Executor,
  denom: string,
  minBalance: bigint,
): Promise<void> {
  try {
    const balance = await executor.getBalance(denom);
    if (balance < minBalance) {
      log.warn("keeper balance below threshold — top up soon", {
        address: executor.address,
        balance: balance.toString(),
        threshold: minBalance.toString(),
      });
    }
  } catch (err) {
    log.warn("balance check failed", {
      detail: err instanceof Error ? err.message : String(err),
    });
  }
}

/**
 * Best-effort check on the FACTORY contract's bounty reserve. Once it
 * dips below the operator-configured threshold, both the oracle and
 * distribution bounties will start emitting `bounty_skipped =
 * insufficient_factory_balance`. We surface this as a warning *before*
 * payouts dry up so the operator has time to refill from the bluechip
 * main wallet. Never throws — failure to query is itself only a warning.
 */
export async function checkFactoryBalance(
  executor: Executor,
  factoryAddress: string,
  denom: string,
  minBalance: bigint,
): Promise<void> {
  try {
    const balance = await executor.getAddressBalance(factoryAddress, denom);
    if (balance < minBalance) {
      log.warn("factory bounty reserve below threshold — top up soon", {
        factory: factoryAddress,
        balance: balance.toString(),
        threshold: minBalance.toString(),
      });
    }
  } catch (err) {
    log.warn("factory balance check failed", {
      factory: factoryAddress,
      detail: err instanceof Error ? err.message : String(err),
    });
  }
}
