import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient } from "./lib/client.js";
import { nextOracleSleepMs } from "./lib/decisions.js";
import { checkKeeperBalance, runOracleIteration } from "./lib/oracle-loop.js";
import { log } from "./lib/logger.js";

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

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

  while (!stopped) {
    await runOracleIteration(client, cfg.FACTORY_ADDRESS);
    await checkKeeperBalance(client, cfg.GAS_DENOM, cfg.MIN_KEEPER_BALANCE_UBLUECHIP);

    const ms = nextOracleSleepMs(cfg.ORACLE_POLL_INTERVAL_MS);
    log.info("sleeping", { ms });
    const tick = 1_000;
    let remaining = ms;
    while (remaining > 0 && !stopped) {
      const chunk = Math.min(tick, remaining);
      await sleep(chunk);
      remaining -= chunk;
    }
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
