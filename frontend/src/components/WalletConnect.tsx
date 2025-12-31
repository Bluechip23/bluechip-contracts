import React, { useState } from 'react';
import { Button, Typography, Box } from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import AccountBalanceWalletIcon from '@mui/icons-material/AccountBalanceWallet';
import { OfflineSigner } from '@cosmjs/proto-signing';
import { Coin } from '@cosmjs/stargate';

interface WalletConnectProps {
    setClient: (client: SigningCosmWasmClient | null) => void;
    setAddress: (address: string) => void;
    setBalance: (balance: Coin) => void;
}

interface ChainConfig {
    chainId: string;
    chainName: string;
    rpc: string;
    rest: string;
    bip44: { coinType: number };
    bech32Config: {
        bech32PrefixAccAddr: string;
        bech32PrefixAccPub: string;
        bech32PrefixValAddr: string;
        bech32PrefixValPub: string;
        bech32PrefixConsAddr: string;
        bech32PrefixConsPub: string;
    };
    currencies: CurrencyConfig[];
    feeCurrencies: FeeCurrencyConfig[];
    stakeCurrency: CurrencyConfig;
}

interface CurrencyConfig {
    coinDenom: string;
    coinMinimalDenom: string;
    coinDecimals: number;
    coinGeckoId: string;
}

interface FeeCurrencyConfig extends CurrencyConfig {
    gasPriceStep: {
        low: number;
        average: number;
        high: number;
    };
}

declare global {
    interface Window {
        keplr?: {
            experimentalSuggestChain: (config: ChainConfig) => Promise<void>;
            enable: (chainId: string) => Promise<void>;
        };
        getOfflineSigner?: (chainId: string) => OfflineSigner;
    }
}
const WalletConnect: React.FC<WalletConnectProps> = ({ setClient, setAddress, setBalance }) => {
    const [walletAddress, setWalletAddress] = useState<string>('');
    const [error, setError] = useState<string>('');

    const connectToChain = async (config: ChainConfig, denom: string): Promise<void> => {
        setError('');

        if (!window.getOfflineSigner || !window.keplr) {
            setError('Please install Keplr extension');
            return;
        }

        try {
            await window.keplr.experimentalSuggestChain(config);
            await window.keplr.enable(config.chainId);

            const offlineSigner = window.getOfflineSigner(config.chainId);
            const accounts = await offlineSigner.getAccounts();
            const address = accounts[0].address;

            setWalletAddress(address);
            setAddress(address);

            const client = await SigningCosmWasmClient.connectWithSigner(
                config.rpc,
                offlineSigner
            );
            setClient(client);

            const balance = await client.getBalance(address, denom);
            setBalance(balance);

        } catch (err) {
            console.error(err);
            const message = err instanceof Error ? err.message : 'Unknown error';
            setError(`Failed to connect: ${message}`);
        }
    };

    const connectWallet = (): Promise<void> => {
        const config: ChainConfig = {
            chainId: "atlantic-2",
            chainName: "Sei Testnet",
            rpc: "https://rpc-testnet.sei-apis.com",
            rest: "https://rest-testnet.sei-apis.com",
            bip44: { coinType: 118 },
            bech32Config: {
                bech32PrefixAccAddr: "sei",
                bech32PrefixAccPub: "seipub",
                bech32PrefixValAddr: "seivaloper",
                bech32PrefixValPub: "seivaloperpub",
                bech32PrefixConsAddr: "seivalcons",
                bech32PrefixConsPub: "seivalconspub",
            },
            currencies: [{
                coinDenom: "SEI",
                coinMinimalDenom: "usei",
                coinDecimals: 6,
                coinGeckoId: "sei-network",
            }],
            feeCurrencies: [{
                coinDenom: "SEI",
                coinMinimalDenom: "usei",
                coinDecimals: 6,
                coinGeckoId: "sei-network",
                gasPriceStep: { low: 0.1, average: 0.2, high: 0.3 },
            }],
            stakeCurrency: {
                coinDenom: "SEI",
                coinMinimalDenom: "usei",
                coinDecimals: 6,
                coinGeckoId: "sei-network",
            },
        };
        return connectToChain(config, "usei");
    };

    const connectLocalWallet = (): Promise<void> => {
        const denom = "stake";
        const prefix = "cosmos";

        const config: ChainConfig = {
            chainId: "bluechipChain",
            chainName: "Bluechip Local",
            rpc: "http://localhost:26657",
            rest: "http://localhost:1317",
            bip44: { coinType: 118 },
            bech32Config: {
                bech32PrefixAccAddr: prefix,
                bech32PrefixAccPub: `${prefix}pub`,
                bech32PrefixValAddr: `${prefix}valoper`,
                bech32PrefixValPub: `${prefix}valoperpub`,
                bech32PrefixConsAddr: `${prefix}valcons`,
                bech32PrefixConsPub: `${prefix}valconspub`,
            },
            currencies: [{
                coinDenom: denom.toUpperCase(),
                coinMinimalDenom: denom,
                coinDecimals: 6,
                coinGeckoId: "unknown",
            }],
            feeCurrencies: [{
                coinDenom: denom.toUpperCase(),
                coinMinimalDenom: denom,
                coinDecimals: 6,
                coinGeckoId: "unknown",
                gasPriceStep: { low: 0.01, average: 0.025, high: 0.04 },
            }],
            stakeCurrency: {
                coinDenom: denom.toUpperCase(),
                coinMinimalDenom: denom,
                coinDecimals: 6,
                coinGeckoId: "unknown",
            },
        };
        return connectToChain(config, denom);
    };

    return (
        <Box sx={{ mb: 2 }}>
            {walletAddress ? (
                <Typography variant="h6" color="primary">
                    Connected: {walletAddress}
                </Typography>
            ) : (
                <Box sx={{ display: 'flex', gap: 2 }}>
                    <Button
                        variant="contained"
                        startIcon={<AccountBalanceWalletIcon />}
                        onClick={connectWallet}
                    >
                        Connect Sei Testnet
                    </Button>
                    <Button
                        variant="outlined"
                        startIcon={<AccountBalanceWalletIcon />}
                        onClick={connectLocalWallet}
                    >
                        Connect Local Node
                    </Button>
                </Box>
            )}
            {error && <Typography color="error">{error}</Typography>}
        </Box>
    );
};

export default WalletConnect;