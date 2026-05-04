# Bluechip Keepers

Off-chain bots that keep the Bluechip protocol running:

- **Oracle keeper** — periodically calls `factory.UpdateOraclePrice` to refresh
  the internal TWAP. Commits reject stale prices, so without this the protocol
  effectively stops.
- **Distribution keeper** — calls each pool's `ContinueDistribution` when it
  has an active post-threshold distribution, so committers receive their
  creator tokens in a reasonable timeframe.

Both earn USD-denominated bounties out of the factory's native balance. The
factory converts USD → bluechip at payout using the internal oracle, so your
keeper compensation stays roughly constant in real terms as bluechip's price
moves.

## Prerequisites

- Node 20+
- A funded keeper wallet (two, actually — one per process)
- The factory contract address
- A comma-separated list of pool contract addresses (for the distribution
  keeper; skippable for an oracle-only deploy)

## One-time setup

```sh
cd keepers
npm install
cp .env.example .env
# edit .env — fill in RPC_ENDPOINT, FACTORY_ADDRESS, KEEPER_MNEMONIC, etc.
npm test          # run the unit tests to confirm the build is sane
npm run typecheck # confirm nothing's broken at the type level
```

## Running

```sh
npm run oracle-keeper          # never exits; runs until SIGTERM
npm run distribution-keeper    # same
```

In production, run each under `systemd` (or Docker, or Cloud Run). Each
process is stateless — crash recovery is just "restart it." Do not run
two instances of the **same** keeper with the **same** mnemonic (they'll
fight on sequence numbers); two different keepers on two different
mnemonics is fine.

### systemd example

```ini
# /etc/systemd/system/bluechip-oracle-keeper.service
[Unit]
Description=Bluechip oracle keeper
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/bluechip-keepers
Environment=NODE_ENV=production
EnvironmentFile=/opt/bluechip-keepers/.env
ExecStart=/usr/bin/npm run oracle-keeper
Restart=always
RestartSec=10
User=bluechip

[Install]
WantedBy=multi-user.target
```

## Funding the factory (one-time by admin, not the keeper)

The factory contract pays bounties out of its **own** native balance. Before
keepers will earn anything, the admin has to:

1. Send bluechip from the main wallet to the factory contract address. This
   is a normal `BankMsg::Send`. Size the reserve for your expected keeper
   throughput; $100 of bluechip covers roughly 20k oracle updates at a
   $0.005 bounty.

2. Enable the bounties by calling the factory as the admin:

   ```sh
   # Oracle keeper: $0.005 per call (6-decimal USD → 5000)
   wasmd tx wasm execute $FACTORY_ADDRESS \
     '{"set_oracle_update_bounty":{"new_bounty":"5000"}}' \
     --from admin ...

   # Distribution keeper: $0.05 per batch
   wasmd tx wasm execute $FACTORY_ADDRESS \
     '{"set_distribution_bounty":{"new_bounty":"50000"}}' \
     --from admin ...
   ```

Caps are $1 per call for both (6-decimal USD = 1_000_000). Above-cap values
are rejected by the contract.

## Funding the keeper wallets (one-time, per deployment)

Each keeper wallet needs enough bluechip for initial gas before bounties
start flowing. ~100 bluechip covers plenty of runway. After that, each
successful call pays the bounty into this same wallet, so it self-replenishes
as long as bounty > gas cost.

## How the keepers decide when to act

### Oracle keeper

```
every ORACLE_POLL_INTERVAL_MS (default 5.5 min):
  submit factory.UpdateOraclePrice {}
  classify response:
    paid      → log success, bluechip received
    skipped   → log reason (disabled | underfunded | price_unavailable)
    ok        → log success, no bounty configured
    failed    → log error, keep going
  catch cooldown / beaten-to-the-punch errors as info-level
  warn if wallet balance < MIN_KEEPER_BALANCE_UBLUECHIP

  # Folded-in maintenance sweep — see "Rate-limit prune" below.
  every ORACLE_PRUNE_EVERY_N iterations (default 200 ≈ once per 18h):
    submit factory.PruneRateLimits { batch_size: PRUNE_BATCH_SIZE }
```

#### Rate-limit prune

The factory tracks per-address create cooldowns in
`LAST_COMMIT_POOL_CREATE_AT` and `LAST_STANDARD_POOL_CREATE_AT`. These
maps grow monotonically — every new creator address adds an entry that
is never removed by the cooldown logic itself. `PruneRateLimits` is a
permissionless handler that removes entries older than 10× the cooldown
(currently 10 hours). The oracle keeper dispatches it once every
`ORACLE_PRUNE_EVERY_N` iterations rather than running a separate bot
because:

- there's no bounty (no economic reason to spin up a third process)
- the cadence is wildly relaxed (daily is plenty)
- the oracle keeper already runs on a fast loop and absorbs the gas
  cost as part of "running keepers"

Set `ORACLE_PRUNE_EVERY_N=0` to disable the sweep entirely (e.g., on a
testnet where storage growth doesn't matter, or if you'd rather run
prune from a separate cron).

### Distribution keeper

```
every DISTRIBUTION_POLL_INTERVAL_MS (default 30 min):
  # Pre-sweep: settle any stuck factory-notify state — see "Retry-notify" below.
  for each pool in POOL_ADDRESSES:
    query pool.FactoryNotifyStatus {}
    if pending=true → submit pool.RetryFactoryNotify {}

  # Main sweep: process pending committer payouts.
  for each pool in POOL_ADDRESSES:
    loop (bounded):
      submit pool.ContinueDistribution {}
      if NothingToRecover → move to next pool
      if paid/ok + distribution_complete=false → continue same pool
      otherwise → move to next pool
  if any pool made progress this sweep, re-sweep in ~15s instead of 30 min
```

The per-pool inner loop is capped at 200 batches per sweep as a safety valve.
Exceeding that is logged loudly — it means something is stuck.

#### Retry-notify

When a commit pool crosses its threshold, it dispatches
`NotifyThresholdCrossed` to the factory as a `reply_on_error` SubMsg.
If the factory rejects (e.g., transient expand-economy issue),
`PENDING_FACTORY_NOTIFY` flips to `true` on the pool and the bluechip
mint reward is held until somebody calls `RetryFactoryNotify`.

The contract handler is permissionless on purpose — anyone can settle
the stuck mint. The distribution keeper polls each pool's
`FactoryNotifyStatus` query first and only spends gas on the (rare)
pools that report `pending=true`. The factory's
`POOL_THRESHOLD_MINTED` idempotency gate makes a redundant retry
harmless: at worst the keeper wastes its own gas, never a double-mint.

No bounty exists for this action; it's folded into the distribution
keeper rather than running as a third bot because it iterates the same
`POOL_ADDRESSES` list at a similar cadence and a 30-minute recovery
latency on a stuck notify is fine — the failure mode is rare and not
time-sensitive.

## Monitoring

Every log line is structured JSON. Pipe into your aggregator of choice (Loki,
Datadog, CloudWatch). The events you want to alert on:

| Level | Message | Action |
|-------|---------|--------|
| `error` | `oracle keeper crashed` | Page. Restart the process. |
| `error` | `distribution keeper crashed` | Page. Restart the process. |
| `warn` | `keeper balance below threshold` | Top up the wallet. |
| `warn` | `bounty skipped`, reason=`insufficient_factory_balance` | Top up the factory. |
| `warn` | `bounty skipped`, reason=`price_unavailable` | Pyth outage. Check upstream. |
| `error` | `retry_factory_notify errored` | Investigate — pool reported pending notify but retry failed for an unexpected reason. |
| `warn` | `factory_notify_status query failed` | Single-pool RPC blip; ignore unless persistent. |
| `warn` | `rate-limit prune errored (non-fatal)` | Investigate — prune is best-effort; persistent failures should be looked at. |

And a liveness check: if you haven't seen an `oracle keeper starting` or
`sleeping` log line from the oracle keeper in >15 minutes, assume it's
hung and restart it.

## Testing

```sh
npm test          # unit tests — decision logic + config
npm run typecheck # type safety
```

The unit tests cover every pure function driving the loops. They do not
require a running chain. Before deploying to mainnet, also smoke-test
against a testnet:

1. Deploy factory + pool to a Cosmos testnet.
2. Point `.env` at the testnet endpoint.
3. Run both keepers for at least a week.
4. Verify the log stream shows the expected mix of paid/skipped/ok outcomes.

## Failure modes you should know about

- **Factory runs out of bluechip.** Bounties start reporting `skipped:
  insufficient_factory_balance`. Oracle keeps updating (not gated on
  bounty). Distribution keeps processing. You just stop earning.

- **Pyth is down for >5 minutes.** Oracle TWAP still updates (it doesn't
  need Pyth). Bounty payout reports `skipped: price_unavailable` because
  the USD → bluechip conversion needs Pyth for the ATOM price. You pay
  gas but don't earn until Pyth recovers.

- **Keeper wallet runs out of gas.** Txs stop going through. You'll see
  `keeper balance below threshold` warnings before this happens if you
  kept `MIN_KEEPER_BALANCE_UBLUECHIP` set to something sane.

- **Both your keeper instances crash simultaneously.** Oracle goes stale
  after ~10 minutes (pool's `MAX_ORACLE_STALENESS_SECONDS`), commits
  start rejecting. Running on two separate VPS's is the cheapest defense.

## Layout

```
src/
├── lib/
│   ├── config.ts             # env parsing (zod-validated)
│   ├── client.ts             # CosmJS wallet + signing client
│   ├── decisions.ts          # pure tx-outcome classification
│   ├── logger.ts             # structured JSON output
│   ├── types.ts              # contract message + query shapes
│   ├── oracle-loop.ts        # one UpdateOraclePrice iteration
│   ├── distribution-loop.ts  # per-pool ContinueDistribution drain
│   ├── prune-loop.ts         # one PruneRateLimits iteration
│   └── retry-notify-loop.ts  # query-then-retry per pool
├── __tests__/
│   ├── config.test.ts
│   ├── decisions.test.ts
│   ├── oracle-keeper.mock-push.test.ts
│   ├── distribution-keeper.integration.test.ts
│   ├── prune-loop.test.ts
│   ├── retry-notify-loop.test.ts
│   └── factory-balance-check.test.ts
├── oracle-keeper.ts          # entrypoint: oracle update + prune sweep
└── distribution-keeper.ts    # entrypoint: retry-notify + distribution drain
```
