import "dotenv/config";
import { loadConfigFromEnv } from "./lib/config.js";
import { buildKeeperClient, execute, getKeeperBalance, type KeeperClient } from "./lib/client.js";
import {
  classifyBountyTx,
  nextOracleSleepMs,
  type TxResult,
} from "./lib/decisions.js";
import { FactoryExecUpdateOraclePrice, isExpectedSkipError } from "./lib/types.js";
import { log } from "./lib/logger.js";
import type { ExecuteResult } from "@cosmjs/cosmwasm-stargate";

// ExecuteResult is only returned on success — failed txs throw errors
// which the outer try/catch handles. So we can hardcode code=0 here.
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
 * One iteration: try to update the oracle, classify the outcome, log,
 * check keeper balance. All errors are caught — the loop never exits
 * because a single iteration failed.
 */
async function runOnce(
  client: KeeperClient,
  factoryAddress: string,
  gasDenom: string,
  minBalance: bigint,
): Promise<void> {
  try {
    const cosmos = await execute(client, factoryAddress, FactoryExecUpdateOraclePrice);
    const outcome = classifyBountyTx(toTxResult(cosmos));

    switch (outcome.kind) {
      case "paid":
        log.info("oracle updated, bounty paid", {
          tx: cosmos.transactionHash,
          usd: outcome.bountyUsd,
          bluechip: outcome.bountyBluechip,
        });
        break;
      case "skipped":
        log.warn("oracle updated, bounty skipped", {
          tx: cosmos.transactionHash,
          reason: outcome.reason,
        });
        break;
      case "ok":
        log.info("oracle updated, no bounty configured", {
          tx: cosmos.transactionHash,
        });
        break;
      case "failed":
        log.error("oracle update tx failed", { rawLog: outcome.rawLog });
        break;
    }
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    if (isExpectedSkipError(msg)) {
      // Someone else beat us to the window, or the cooldown hasn't
      // elapsed yet. Totally normal. Logged at info, not warn.
      log.info("oracle update skipped (cooldown or beaten)", { detail: msg });
    } else {
      log.error("oracle update errored", { detail: msg });
    }
  }

  // Balance check is best-effort — never fail the loop on a balance
  // query glitch.
  try {
    const balance = await getKeeperBalance(client, gasDenom);
    if (balance < minBalance) {
      log.warn("keeper balance below threshold — top up soon", {
        address: client.address,
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

  // Graceful shutdown.
  let stopped = false;
  const stop = () => {
    stopped = true;
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);

  while (!stopped) {
    await runOnce(
      client,
      cfg.FACTORY_ADDRESS,
      cfg.GAS_DENOM,
      cfg.MIN_KEEPER_BALANCE_UBLUECHIP,
    );
    const ms = nextOracleSleepMs(cfg.ORACLE_POLL_INTERVAL_MS);
    log.info("sleeping", { ms });
    // Interruptible sleep: wake early if stop fires mid-sleep.
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
