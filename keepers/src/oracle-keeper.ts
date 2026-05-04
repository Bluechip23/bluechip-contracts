import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient } from "./lib/client.js";
import { nextOracleSleepMs } from "./lib/decisions.js";
import {
  checkFactoryBalance,
  checkKeeperBalance,
  runOracleIteration,
} from "./lib/oracle-loop.js";
import { runPruneIteration } from "./lib/prune-loop.js";
import { interruptibleSleep } from "./lib/sleep.js";
import { log } from "./lib/logger.js";

async function main(): Promise<void> {
  const cfg = loadConfigFromEnv();
  log.info("oracle keeper starting", {
    rpc: cfg.RPC_ENDPOINT,
    chain: cfg.CHAIN_ID,
    factory: cfg.FACTORY_ADDRESS,
    interval_ms: cfg.ORACLE_POLL_INTERVAL_MS,
  });

  const client = await buildKeeperClient(cfg);
  log.info("keeper wallet ready", { address: client.address });

  let stopped = false;
  const stop = () => {
    stopped = true;
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);

  const mockPush = cfg.MOCK_ORACLE_ADDRESS
    ? {
        oracleAddress: cfg.MOCK_ORACLE_ADDRESS,
        feedId: cfg.MOCK_PRICE_FEED_ID,
        priceUbluechip: cfg.MOCK_PRICE_UBLUECHIP,
      }
    : undefined;
  if (mockPush) {
    log.info("mock price push enabled", {
      mock_oracle: mockPush.oracleAddress,
      feed_id: mockPush.feedId,
      price: mockPush.priceUbluechip,
    });
  }

  // Iteration counter for the rate-limit prune sweep. We dispatch
  // PruneRateLimits once every ORACLE_PRUNE_EVERY_N iterations of the
  // oracle keeper. A counter (rather than wall-clock cadence) is used
  // so the prune always runs immediately after a successful oracle
  // iteration on the same wallet — keeping the sequence-number tx
  // ordering simple. ORACLE_PRUNE_EVERY_N == 0 disables the sweep.
  let pruneCounter = 0;
  if (cfg.ORACLE_PRUNE_EVERY_N === 0) {
    log.info("rate-limit prune sweep disabled (ORACLE_PRUNE_EVERY_N=0)");
  } else {
    log.info("rate-limit prune sweep enabled", {
      every_n_iterations: cfg.ORACLE_PRUNE_EVERY_N,
      batch_size: cfg.PRUNE_BATCH_SIZE,
    });
  }

  while (!stopped) {
    await runOracleIteration(client, cfg.FACTORY_ADDRESS, mockPush);
    await checkKeeperBalance(client, cfg.GAS_DENOM, cfg.MIN_KEEPER_BALANCE_UBLUECHIP);
    await checkFactoryBalance(
      client,
      cfg.FACTORY_ADDRESS,
      cfg.GAS_DENOM,
      cfg.MIN_FACTORY_BOUNTY_RESERVE_UBLUECHIP,
    );

    // Once every ORACLE_PRUNE_EVERY_N iterations, also run the
    // rate-limit prune. Independent of the oracle iteration's success
    // — a failed UpdateOraclePrice doesn't mean we shouldn't also
    // prune; the two are unrelated chain operations.
    if (cfg.ORACLE_PRUNE_EVERY_N > 0) {
      pruneCounter += 1;
      if (pruneCounter >= cfg.ORACLE_PRUNE_EVERY_N) {
        pruneCounter = 0;
        await runPruneIteration(client, cfg.FACTORY_ADDRESS, cfg.PRUNE_BATCH_SIZE);
      }
    }

    const ms = nextOracleSleepMs(cfg.ORACLE_POLL_INTERVAL_MS);
    log.info("sleeping", { ms });
    await interruptibleSleep(ms, () => stopped);
  }

  log.info("oracle keeper shutting down");
  client.close();
}

main().catch((err) => {
  log.error("oracle keeper crashed", {
    detail: err instanceof Error ? err.message : String(err),
  });
  process.exit(1);
});
