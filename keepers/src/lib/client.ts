import { DirectSecp256k1HdWallet } from "@cosmjs/proto-signing";
import { SigningCosmWasmClient } from "@cosmjs/cosmwasm-stargate";
import { GasPrice } from "@cosmjs/stargate";
import type { Config } from "./config.js";
import type { Executor } from "./executor.js";
import type { TxResult } from "./decisions.js";

export interface KeeperClient extends Executor {
  close: () => void;
}

/**
 * Derives the keeper wallet from mnemonic, connects to the chain, and
 * returns a live Executor plus a close handle.
 *
 * Use two different mnemonics (one per keeper process) so the oracle
 * and distribution bots don't fight over sequence numbers.
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
  const address = first.address;
  return {
    address,
    async execute(contract, msg): Promise<TxResult> {
      // ExecuteResult is only returned on success — failures throw.
      const result = await signer.execute(address, contract, msg, "auto", undefined, []);
      return {
        code: 0,
        transactionHash: result.transactionHash,
        events: result.events,
      };
    },
    async getBalance(denom): Promise<bigint> {
      const coin = await signer.getBalance(address, denom);
      return BigInt(coin.amount);
    },
    close: () => signer.disconnect(),
  };
}
