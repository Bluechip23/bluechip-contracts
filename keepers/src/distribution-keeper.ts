import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient, execute, getKeeperBalance, type KeeperClient } from "./lib/client.js";
import {
  classifyBountyTx,
  isDistributionComplete,
  nextDistributionSleepMs,
  shouldContinueSamePool,
  type TxOutcome,
  type TxResult,
} from "./lib/decisions.js";
import { PoolExecContinueDistribution, isExpectedSkipError } from "./lib/types.js";
import { log } from "./lib/logger.js";
import type { ExecuteResult } from "@cosmjs/cosmwasm-stargate";

// ExecuteResult is only returned on success — failed txs throw errors
// which the outer try/catch handles.
function toTxResult(result: ExecuteResult): TxResult {
  return {
    code: 0,
    transactionHash: result.transactionHash,
    events: result.events,
  };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Drain one pool by calling ContinueDistribution repeatedly until
 * either the pool reports distribution_complete=true or a tx returns
 * a non-progress outcome (skipped/failed/NothingToRecover).
 *
 * Returns whether ANY batch was successfully processed, so the outer
 * loop can decide whether to poll again quickly or wait the full
 * interval.
 */
async function drainPool(
  client: KeeperClient,
  poolAddress: string,
  perCallDelayMs: number,
): Promise<boolean> {
  let madeProgress = false;

  // Safety: never loop forever on a single pool within one iteration.
  // 200 × 40 committers = 8_000 max per iteration, which is well above
  // a realistic single-pool distribution. If we hit this, something is
  // stuck and the operator should see it in the logs.
  const maxBatches = 200;

  for (let batch = 0; batch < maxBatches; batch++) {
    let outcome: TxOutcome;
    let complete = false;

    try {
      const cosmos = await execute(client, poolAddress, PoolExecContinueDistribution);
      const tx = toTxResult(cosmos);
      outcome = classifyBountyTx(tx);
      complete = isDistributionComplete(tx);

      switch (outcome.kind) {
        case "paid":
          madeProgress = true;
          log.info("distribution batch paid", {
            pool: poolAddress,
            batch,
            tx: cosmos.transactionHash,
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
            tx: cosmos.transactionHash,
            complete,
          });
          break;
        case "skipped":
          log.warn("distribution batch, bounty skipped", {
            pool: poolAddress,
            batch,
            tx: cosmos.transactionHash,
            reason: outcome.reason,
            complete,
          });
          // A skipped bounty with an otherwise-successful tx still
          // means distribution progressed — but we stop the inner
          // loop because something needs operator attention (factory
          // underfunded, price unavailable, etc).
          madeProgress = true;
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
      } else {
        log.error("distribution call errored", { pool: poolAddress, detail: msg });
      }
      return madeProgress;
    }

    if (!shouldContinueSamePool(outcome, complete)) {
      break;
    }
    // Small breather between consecutive batches so we don't spam the
    // RPC endpoint or the mempool.
    if (perCallDelayMs > 0) {
      await sleep(perCallDelayMs);
    }
  }

  return madeProgress;
}

async function runOnce(
  client: KeeperClient,
  poolAddresses: ReadonlyArray<string>,
  perCallDelayMs: number,
): Promise<boolean> {
  let madeProgressAny = false;
  for (const pool of poolAddresses) {
    const progress = await drainPool(client, pool, perCallDelayMs);
    if (progress) madeProgressAny = true;
  }
  return madeProgressAny;
}

async function main(): Promise<void> {
  const cfg = loadConfigFromEnv();

  if (cfg.POOL_ADDRESSES.length === 0) {
    log.error("POOL_ADDRESSES is empty — nothing for the distribution keeper to watch");
    log.error("set POOL_ADDRESSES in .env as a comma-separated list of pool contract addresses");
    process.exit(1);
  }

  log.info("distribution keeper starting", {
    rpc: cfg.RPC_ENDPOINT,
    chain: cfg.CHAIN_ID,
    pools: cfg.POOL_ADDRESSES.length,
    interval_ms: cfg.DISTRIBUTION_POLL_INTERVAL_MS,
  });

  const client = await buildKeeperClient(cfg);
  log.info("keeper wallet ready", { address: client.address });

  let stopped = false;
  const stop = () => {
    stopped = true;
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);

  while (!stopped) {
    const madeProgress = await runOnce(
      client,
      cfg.POOL_ADDRESSES,
      cfg.DISTRIBUTION_PER_POOL_DELAY_MS,
    );

    try {
      const balance = await getKeeperBalance(client, cfg.GAS_DENOM);
      if (balance < cfg.MIN_KEEPER_BALANCE_UBLUECHIP) {
        log.warn("keeper balance below threshold — top up soon", {
          address: client.address,
          balance: balance.toString(),
          threshold: cfg.MIN_KEEPER_BALANCE_UBLUECHIP.toString(),
        });
      }
    } catch (err) {
      log.warn("balance check failed", {
        detail: err instanceof Error ? err.message : String(err),
      });
    }

    const ms = nextDistributionSleepMs(cfg.DISTRIBUTION_POLL_INTERVAL_MS, madeProgress);
    log.info("sleeping", { ms, made_progress: madeProgress });
    const tick = 1_000;
    let remaining = ms;
    while (remaining > 0 && !stopped) {
      const chunk = Math.min(tick, remaining);
      await sleep(chunk);
      remaining -= chunk;
    }
  }

  log.info("distribution keeper shutting down");
  client.close();
}

main().catch((err) => {
  log.error("distribution keeper crashed", {
    detail: err instanceof Error ? err.message : String(err),
  });
  process.exit(1);
});
