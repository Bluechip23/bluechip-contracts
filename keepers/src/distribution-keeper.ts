import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient } from "./lib/client.js";
import { nextDistributionSleepMs } from "./lib/decisions.js";
import { runDistributionSweep } from "./lib/distribution-loop.js";
import { checkFactoryBalance, checkKeeperBalance } from "./lib/oracle-loop.js";
import { runRetryNotifySweep } from "./lib/retry-notify-loop.js";
import { interruptibleSleep } from "./lib/sleep.js";
import { log } from "./lib/logger.js";

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
    // Run the retry-factory-notify sweep BEFORE the distribution sweep.
    // Order matters: a stuck factory-notify means POOL_THRESHOLD_MINTED
    // never landed on the factory side, which blocks the bluechip
    // mint reward but does NOT block distribution itself (distribution
    // mints creator-tokens, which is a pool-side action). Still,
    // settling the notify first means the same iteration can leave
    // the pool fully consistent rather than half. Each pool's
    // RetryFactoryNotify is itself permissionless and idempotent on
    // the factory side (POOL_THRESHOLD_MINTED gate), so a redundant
    // call is at worst wasted gas — never a double-mint.
    await runRetryNotifySweep(client, cfg.POOL_ADDRESSES);

    const { madeProgress } = await runDistributionSweep(
      client,
      cfg.POOL_ADDRESSES,
      cfg.DISTRIBUTION_PER_POOL_DELAY_MS,
    );
    await checkKeeperBalance(client, cfg.GAS_DENOM, cfg.MIN_KEEPER_BALANCE_UBLUECHIP);
    // Distribution bounties are paid from the factory's native reserve, not
    // the pools'. If the reserve drains, factory.PayDistributionBounty starts
    // emitting `bounty_skipped = insufficient_factory_balance` and the
    // keeper's effective compensation goes to zero. Surface a warning while
    // there's still time to top up.
    await checkFactoryBalance(
      client,
      cfg.FACTORY_ADDRESS,
      cfg.GAS_DENOM,
      cfg.MIN_FACTORY_BOUNTY_RESERVE_UBLUECHIP,
    );

    const ms = nextDistributionSleepMs(cfg.DISTRIBUTION_POLL_INTERVAL_MS, madeProgress);
    log.info("sleeping", { ms, made_progress: madeProgress });
    await interruptibleSleep(ms, () => stopped);
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
