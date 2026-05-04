import type { Executor } from "../lib/executor.js";
import type { TxEvent, TxResult } from "../lib/decisions.js";

/**
 * In-memory simulation of the on-chain factory + pools. Implements
 * the Executor interface so the real keeper loops can run against it
 * in tests. Models the contract-side invariants we care about:
 *
 *   - Factory enforces a 5-min cooldown on UpdateOraclePrice. Calls
 *     inside the window throw an UpdateTooSoon-style error.
 *   - Successful oracle updates emit a wasm event with bounty_paid_usd
 *     and bounty_paid_bluechip attributes (when bounty is configured
 *     and funded).
 *   - Underfunded factory emits bounty_skipped=insufficient_factory_balance.
 *   - Disabled bounty emits no bounty_* attributes.
 *   - Pools support ContinueDistribution. A pool not in distribution
 *     state throws a NothingToRecover-style error.
 *   - Pools with distribution state emit distribution_complete=false
 *     until drained, then true on the final batch.
 *
 * Drives clock via an injectable now() so tests can deterministically
 * fast-forward.
 */

export interface MockContractsOptions {
  /** Monotonic clock in ms. Tests advance this between iterations. */
  now: () => number;
  /** Factory contract address. */
  factoryAddress: string;
  /** Oracle cooldown in ms (maps to UPDATE_INTERVAL on-chain, 5 min). */
  oracleCooldownMs?: number;
  /** Initial factory balance in ubluechip. */
  initialFactoryBalance?: bigint;
  /** Oracle bounty setting in USD, 6 decimals. */
  oracleBountyUsd?: bigint;
  /** Distribution bounty setting in USD, 6 decimals. */
  distributionBountyUsd?: bigint;
  /** ubluechip per USD, for conversion. 1_000_000 = 1 bluechip / $1. */
  bluechipPerUsd?: bigint;
}

interface PoolState {
  isDistributing: boolean;
  batchesRemaining: number;
  /**
   * If true, each call errors synthetically with a "price_unavailable"
   * skip attribute instead of paying. Tests use this to simulate Pyth
   * outages.
   */
  oracleUnavailable?: boolean;
  /**
   * Mirrors the on-chain `PENDING_FACTORY_NOTIFY` flag. True when the
   * pool's threshold-cross commit landed but the factory-side
   * NotifyThresholdCrossed SubMsg failed. RetryFactoryNotify clears
   * this on success. Tests set it explicitly to drive the retry-notify
   * keeper.
   */
  pendingFactoryNotify?: boolean;
  /**
   * If true, RetryFactoryNotify dispatches against this pool throw
   * synthetically — e.g., simulating a still-failing factory side
   * (idempotency error). Used to exercise the "tx_skip" expected-error
   * branch of the retry-notify keeper.
   */
  retryFails?: boolean;
}

let txCounter = 0;

function nextHash(): string {
  txCounter++;
  return `TX${txCounter.toString().padStart(8, "0")}`;
}

function wasmEvent(attrs: Array<[string, string]>): TxEvent {
  return {
    type: "wasm",
    attributes: attrs.map(([key, value]) => ({ key, value })),
  };
}

export class MockContracts implements Executor {
  readonly address: string;
  private readonly now: () => number;
  private readonly factoryAddress: string;
  private readonly oracleCooldownMs: number;
  private factoryBalance: bigint;
  private keeperBalance: bigint;
  private oracleBountyUsd: bigint;
  private distributionBountyUsd: bigint;
  private bluechipPerUsd: bigint;
  private oracleLastUpdate: number; // ms; 0 means never
  private oracleStarvation = false; // if true, convert throws
  private pools = new Map<string, PoolState>();
  private unregisteredPools = new Set<string>(); // auth test hook
  // Mock-oracle push observability: every execute() call is recorded here
  // so tests can assert "keeper pushed SetPrice before UpdateOraclePrice".
  public readonly calls: Array<{ contract: string; msg: Record<string, unknown> }> = [];
  // Test hook: make the next execute() against a given address throw.
  // Used to simulate a transient mock-oracle-push failure.
  private failOnceAddresses = new Set<string>();

  constructor(address: string, opts: MockContractsOptions) {
    this.address = address;
    this.now = opts.now;
    this.factoryAddress = opts.factoryAddress;
    this.oracleCooldownMs = opts.oracleCooldownMs ?? 5 * 60 * 1000;
    this.factoryBalance = opts.initialFactoryBalance ?? 10_000_000n;
    this.keeperBalance = 1_000_000_000n;
    this.oracleBountyUsd = opts.oracleBountyUsd ?? 5_000n; // $0.005
    this.distributionBountyUsd = opts.distributionBountyUsd ?? 50_000n; // $0.05
    this.bluechipPerUsd = opts.bluechipPerUsd ?? 1_000_000n; // 1 bluechip / $1
    this.oracleLastUpdate = 0;
  }

  /** Test hook: set a pool's distribution state. */
  setupPoolDistribution(address: string, batches: number): void {
    this.pools.set(address, { isDistributing: true, batchesRemaining: batches });
  }

  /** Test hook: flip conversion into "oracle starved" mode. */
  starveOracle(): void {
    this.oracleStarvation = true;
  }

  /** Test hook: tamper with balances. */
  setFactoryBalance(amount: bigint): void {
    this.factoryBalance = amount;
  }

  getFactoryBalance(): bigint {
    return this.factoryBalance;
  }

  /** Test hook: set oracle bounty. Max-cap enforcement elsewhere. */
  setOracleBounty(usd: bigint): void {
    this.oracleBountyUsd = usd;
  }

  setDistributionBounty(usd: bigint): void {
    this.distributionBountyUsd = usd;
  }

  /** Test hook: pretend this address isn't in POOLS_BY_CONTRACT_ADDRESS. */
  deregisterPool(address: string): void {
    this.unregisteredPools.add(address);
  }

  /** Test hook: arm a pool's PENDING_FACTORY_NOTIFY flag. */
  setPendingFactoryNotify(address: string, pending: boolean): void {
    const pool = this.pools.get(address) ?? {
      isDistributing: false,
      batchesRemaining: 0,
    };
    pool.pendingFactoryNotify = pending;
    this.pools.set(address, pool);
  }

  /** Test hook: future RetryFactoryNotify on this pool throws. */
  failNextRetryNotify(address: string, fail: boolean = true): void {
    const pool = this.pools.get(address) ?? {
      isDistributing: false,
      batchesRemaining: 0,
    };
    pool.retryFails = fail;
    this.pools.set(address, pool);
  }

  /**
   * Test hook: track every PruneRateLimits call dispatched against the
   * factory so tests can assert cadence and batch_size threading.
   */
  public readonly pruneCalls: Array<{ batchSize: number }> = [];
  /**
   * Test hook: programmable counters returned by the next prune call.
   * Defaults to (0, 0) — i.e., a steady-state "nothing to prune" sweep.
   */
  private nextPruneCounters: { commit: number; standard: number } = {
    commit: 0,
    standard: 0,
  };
  setNextPruneCounters(commit: number, standard: number): void {
    this.nextPruneCounters = { commit, standard };
  }

  /** Test hook: the next execute() against `address` throws. One-shot. */
  failNextExecute(address: string): void {
    this.failOnceAddresses.add(address);
  }

  // Executor impl --------------------------------------------------------

  async execute(contract: string, msg: Record<string, unknown>): Promise<TxResult> {
    this.calls.push({ contract, msg });
    if (this.failOnceAddresses.has(contract)) {
      this.failOnceAddresses.delete(contract);
      throw new Error(`mock: forced failure on ${contract}`);
    }
    if (contract === this.factoryAddress) {
      return this.executeFactory(msg);
    }
    if ("set_price" in msg) {
      // Mock-oracle SetPrice: accept silently. Tests inspect `calls` to
      // verify the keeper pushed the expected SetPrice before calling
      // UpdateOraclePrice.
      return { code: 0, transactionHash: nextHash(), events: [] };
    }
    // Otherwise treat as pool.
    return this.executePool(contract, msg);
  }

  async getBalance(_denom: string): Promise<bigint> {
    return this.keeperBalance;
  }

  async getAddressBalance(target: string, _denom: string): Promise<bigint> {
    if (target === this.factoryAddress) {
      return this.factoryBalance;
    }
    if (target === this.address) {
      return this.keeperBalance;
    }
    return 0n;
  }

  async queryContractSmart<T>(contract: string, msg: Record<string, unknown>): Promise<T> {
    if ("factory_notify_status" in msg) {
      // Mirror creator-pool::query::query_factory_notify_status —
      // returns { pending: bool } reading from the pool's
      // PENDING_FACTORY_NOTIFY storage. We model "no pool entry" as
      // pending=false, matching the contract's `unwrap_or(false)`.
      const pool = this.pools.get(contract);
      return { pending: pool?.pendingFactoryNotify ?? false } as unknown as T;
    }
    throw new Error(`mock query unsupported: ${JSON.stringify(msg)}`);
  }

  // Factory handlers -----------------------------------------------------

  private executeFactory(msg: Record<string, unknown>): TxResult {
    if ("update_oracle_price" in msg) {
      return this.executeUpdateOraclePrice();
    }
    if ("prune_rate_limits" in msg) {
      return this.executePruneRateLimits(msg);
    }
    throw new Error(`factory: unknown message ${JSON.stringify(msg)}`);
  }

  private executePruneRateLimits(msg: Record<string, unknown>): TxResult {
    // Capture the batch_size so tests can assert it threaded through
    // from PRUNE_BATCH_SIZE config.
    const inner = (msg as { prune_rate_limits: { batch_size?: number } })
      .prune_rate_limits;
    const batchSize = inner?.batch_size ?? 100;
    this.pruneCalls.push({ batchSize });

    const counters = this.nextPruneCounters;
    // Reset for next call to the steady-state default; tests opt back
    // in via setNextPruneCounters.
    this.nextPruneCounters = { commit: 0, standard: 0 };

    return {
      code: 0,
      transactionHash: nextHash(),
      events: [
        wasmEvent([
          ["action", "prune_rate_limits"],
          ["commit_pruned", counters.commit.toString()],
          ["standard_pruned", counters.standard.toString()],
          ["stale_after_secs", "36000"],
          ["batch_size", batchSize.toString()],
        ]),
      ],
    };
  }

  private executeUpdateOraclePrice(): TxResult {
    const now = this.now();
    if (
      this.oracleLastUpdate !== 0 &&
      now - this.oracleLastUpdate < this.oracleCooldownMs
    ) {
      // Mirrors the UpdateTooSoon error thrown by the real factory.
      throw new Error(
        "UpdateTooSoon: too soon since last oracle update",
      );
    }
    this.oracleLastUpdate = now;

    const attrs: Array<[string, string]> = [
      ["action", "update_oracle"],
      ["twap_price", "10000000"],
    ];

    if (this.oracleBountyUsd === 0n) {
      // No bounty attributes emitted — classifies as "ok".
    } else if (this.oracleStarvation) {
      attrs.push(["bounty_skipped", "price_unavailable"]);
      attrs.push(["bounty_configured_usd", this.oracleBountyUsd.toString()]);
    } else {
      const bluechip = (this.oracleBountyUsd * this.bluechipPerUsd) / 1_000_000n;
      if (this.factoryBalance < bluechip) {
        attrs.push(["bounty_skipped", "insufficient_factory_balance"]);
        attrs.push(["bounty_required_bluechip", bluechip.toString()]);
        attrs.push(["factory_balance", this.factoryBalance.toString()]);
      } else {
        this.factoryBalance -= bluechip;
        this.keeperBalance += bluechip;
        attrs.push(["bounty_paid_usd", this.oracleBountyUsd.toString()]);
        attrs.push(["bounty_paid_bluechip", bluechip.toString()]);
        attrs.push(["bounty_recipient", this.address]);
      }
    }

    return {
      code: 0,
      transactionHash: nextHash(),
      events: [wasmEvent(attrs)],
    };
  }

  // Pool handlers --------------------------------------------------------

  private executePool(poolAddress: string, msg: Record<string, unknown>): TxResult {
    if ("continue_distribution" in msg) {
      return this.executeContinueDistribution(poolAddress);
    }
    if ("retry_factory_notify" in msg) {
      return this.executeRetryFactoryNotify(poolAddress);
    }
    throw new Error(`pool: unknown message ${JSON.stringify(msg)}`);
  }

  /**
   * Mirror creator-pool::contract::execute_retry_factory_notify.
   *
   * Three behaviors:
   *   - pool has no pending notify → throw the canonical
   *     "No pending factory notification to retry" error (treated as
   *     an expected skip by the keeper).
   *   - pool's `retryFails` flag is set → throw a generic factory-side
   *     error like "Bluechip mint already triggered" so we exercise
   *     the keeper's tx_skip recovery branch.
   *   - happy path → clear pendingFactoryNotify, emit attributes.
   */
  private executeRetryFactoryNotify(poolAddress: string): TxResult {
    const pool = this.pools.get(poolAddress);
    if (!pool || !pool.pendingFactoryNotify) {
      throw new Error("No pending factory notification to retry");
    }
    if (pool.retryFails) {
      pool.retryFails = false;
      throw new Error("Bluechip mint already triggered for this pool");
    }
    pool.pendingFactoryNotify = false;
    return {
      code: 0,
      transactionHash: nextHash(),
      events: [
        wasmEvent([
          ["action", "retry_factory_notify"],
          ["pool_id", "1"],
        ]),
      ],
    };
  }

  private executeContinueDistribution(poolAddress: string): TxResult {
    const pool = this.pools.get(poolAddress);
    if (!pool || !pool.isDistributing) {
      // Mirrors NothingToRecover / storage-not-found errors.
      throw new Error("NothingToRecover: distribution not in progress");
    }

    pool.batchesRemaining = Math.max(0, pool.batchesRemaining - 1);
    const complete = pool.batchesRemaining === 0;
    if (complete) {
      pool.isDistributing = false;
    }

    const attrs: Array<[string, string]> = [
      ["action", "continue_distribution"],
      ["distribution_complete", complete ? "true" : "false"],
    ];

    // The real flow: pool emits a WasmMsg to factory.PayDistributionBounty.
    // Our mock collapses that into a single response with the same final
    // attributes the factory would have emitted.
    if (this.unregisteredPools.has(poolAddress)) {
      // Factory would have rejected with Unauthorized — mirrors the
      // real behavior where the pool's whole tx reverts.
      throw new Error("Unauthorized: caller not a registered pool");
    }

    if (this.distributionBountyUsd === 0n) {
      // No bounty attributes → classifies as "ok".
    } else if (this.oracleStarvation) {
      attrs.push(["bounty_skipped", "price_unavailable"]);
    } else {
      const bluechip = (this.distributionBountyUsd * this.bluechipPerUsd) / 1_000_000n;
      if (this.factoryBalance < bluechip) {
        attrs.push(["bounty_skipped", "insufficient_factory_balance"]);
      } else {
        this.factoryBalance -= bluechip;
        this.keeperBalance += bluechip;
        attrs.push(["bounty_paid_usd", this.distributionBountyUsd.toString()]);
        attrs.push(["bounty_paid_bluechip", bluechip.toString()]);
      }
    }

    return {
      code: 0,
      transactionHash: nextHash(),
      events: [wasmEvent(attrs)],
    };
  }
}
