import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient } from "./lib/client.js";
import { nextDistributionSleepMs } from "./lib/decisions.js";
import { runDistributionSweep } from "./lib/distribution-loop.js";
import { checkKeeperBalance } from "./lib/oracle-loop.js";
import { log } from "./lib/logger.js";

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
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
    const { madeProgress } = await runDistributionSweep(
      client,
      cfg.POOL_ADDRESSES,
      cfg.DISTRIBUTION_PER_POOL_DELAY_MS,
    );
    await checkKeeperBalance(client, cfg.GAS_DENOM, cfg.MIN_KEEPER_BALANCE_UBLUECHIP);

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
