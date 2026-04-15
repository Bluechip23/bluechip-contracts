import { DirectSecp256k1HdWallet } from "@cosmjs/proto-signing";
import {
  SigningCosmWasmClient,
  type ExecuteResult,
} from "@cosmjs/cosmwasm-stargate";
import { GasPrice } from "@cosmjs/stargate";
import type { Config } from "./config.js";

export interface KeeperClient {
  signer: SigningCosmWasmClient;
  address: string;
  close: () => void;
}

/**
 * Derives the keeper wallet from mnemonic, connects to the chain, and
 * returns a signing client plus the keeper's own bech32 address.
 *
 * The wallet is always account 0 / index 0 of the mnemonic. Keep two
 * different mnemonics (one per keeper process) so the oracle and
 * distribution bots don't fight over sequence numbers.
 */
export async function buildKeeperClient(cfg: Config): Promise<KeeperClient> {
  const wallet = await DirectSecp256k1HdWallet.fromMnemonic(cfg.KEEPER_MNEMONIC, {
    prefix: cfg.BECH32_PREFIX,
  });
  const accounts = await wallet.getAccounts();
  const first = accounts[0];
  if (!first) {
    throw new Error("derived wallet produced no accounts");
  }
  const signer = await SigningCosmWasmClient.connectWithSigner(
    cfg.RPC_ENDPOINT,
    wallet,
    {
      gasPrice: GasPrice.fromString(cfg.GAS_PRICE),
    },
  );
  return {
    signer,
    address: first.address,
    close: () => signer.disconnect(),
  };
}

/**
 * Execute a contract call and return the raw ExecuteResult. Wrapped in
 * its own function so the keeper loops don't need to pass `funds` and
 * `memo` at every call site.
 */
export async function execute(
  client: KeeperClient,
  contract: string,
  msg: Record<string, unknown>,
): Promise<ExecuteResult> {
  return client.signer.execute(client.address, contract, msg, "auto", undefined, []);
}

/**
 * Fetch the keeper's current bluechip balance. Used to warn operators
 * when gas runway gets low.
 */
export async function getKeeperBalance(
  client: KeeperClient,
  denom: string,
): Promise<bigint> {
  const coin = await client.signer.getBalance(client.address, denom);
  return BigInt(coin.amount);
}
