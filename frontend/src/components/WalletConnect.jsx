import React, { useState } from 'react';
import { Button, Typography, Box } from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import AccountBalanceWalletIcon from '@mui/icons-material/AccountBalanceWallet';

const WalletConnect = ({ setClient, setAddress, setBalance }) => {
    const [walletAddress, setWalletAddress] = useState('');
    const [error, setError] = useState('');

    const connectWallet = async () => {
        setError('');
        if (!window.getOfflineSigner || !window.keplr) {
            setError('Please install Keplr extension');
            return;
        }

        const chainId = "atlantic-2";
        const rpcEndpoint = "https://rpc-testnet.sei-apis.com";
        const restEndpoint = "https://rest-testnet.sei-apis.com";

        try {
            // Suggest the Sei Testnet chain to Keplr
            await window.keplr.experimentalSuggestChain({
                chainId: chainId,
                chainName: "Sei Testnet",
                rpc: rpcEndpoint,
                rest: restEndpoint,
                bip44: {
                    coinType: 118,
                },
                bech32Config: {
                    bech32PrefixAccAddr: "sei",
                    bech32PrefixAccPub: "seipub",
                    bech32PrefixValAddr: "seivaloper",
                    bech32PrefixValPub: "seivaloperpub",
                    bech32PrefixConsAddr: "seivalcons",
                    bech32PrefixConsPub: "seivalconspub",
                },
                currencies: [
                    {
                        coinDenom: "SEI",
                        coinMinimalDenom: "usei",
                        coinDecimals: 6,
                        coinGeckoId: "sei-network",
                    },
                ],
                feeCurrencies: [
                    {
                        coinDenom: "SEI",
                        coinMinimalDenom: "usei",
                        coinDecimals: 6,
                        coinGeckoId: "sei-network",
                        gasPriceStep: {
                            low: 0.1,
                            average: 0.2,
                            high: 0.3,
                        },
                    },
                ],
                stakeCurrency: {
                    coinDenom: "SEI",
                    coinMinimalDenom: "usei",
                    coinDecimals: 6,
                    coinGeckoId: "sei-network",
                },
            });

            await window.keplr.enable(chainId);
            const offlineSigner = window.getOfflineSigner(chainId);
            const accounts = await offlineSigner.getAccounts();
            setWalletAddress(accounts[0].address);
            setAddress(accounts[0].address);

            const client = await SigningCosmWasmClient.connectWithSigner(
                rpcEndpoint,
                offlineSigner
            );
            setClient(client);

            const balance = await client.getBalance(accounts[0].address, "usei");
            setBalance(balance);

        } catch (err) {
            console.error(err);
            setError('Failed to connect: ' + err.message);
        }
    };

    const connectLocalWallet = async () => {
        setError('');
        if (!window.getOfflineSigner || !window.keplr) {
            setError('Please install Keplr extension');
            return;
        }

        const chainId = "bluechipChain";
        const rpcEndpoint = "http://localhost:26657";
        const restEndpoint = "http://localhost:1317";
        const denom = "stake";
        const prefix = "cosmos";

        try {
            await window.keplr.experimentalSuggestChain({
                chainId: chainId,
                chainName: "Bluechip Local",
                rpc: rpcEndpoint,
                rest: restEndpoint,
                bip44: {
                    coinType: 118,
                },
                bech32Config: {
                    bech32PrefixAccAddr: prefix,
                    bech32PrefixAccPub: prefix + "pub",
                    bech32PrefixValAddr: prefix + "valoper",
                    bech32PrefixValPub: prefix + "valoperpub",
                    bech32PrefixConsAddr: prefix + "valcons",
                    bech32PrefixConsPub: prefix + "valconspub",
                },
                currencies: [
                    {
                        coinDenom: denom.toUpperCase(),
                        coinMinimalDenom: denom,
                        coinDecimals: 6,
                        coinGeckoId: "unknown",
                    },
                ],
                feeCurrencies: [
                    {
                        coinDenom: denom.toUpperCase(),
                        coinMinimalDenom: denom,
                        coinDecimals: 6,
                        coinGeckoId: "unknown",
                        gasPriceStep: {
                            low: 0.01,
                            average: 0.025,
                            high: 0.04,
                        },
                    },
                ],
                stakeCurrency: {
                    coinDenom: denom.toUpperCase(),
                    coinMinimalDenom: denom,
                    coinDecimals: 6,
                    coinGeckoId: "unknown",
                },
            });

            await window.keplr.enable(chainId);
            const offlineSigner = window.getOfflineSigner(chainId);
            const accounts = await offlineSigner.getAccounts();
            setWalletAddress(accounts[0].address);
            setAddress(accounts[0].address);

            const client = await SigningCosmWasmClient.connectWithSigner(
                rpcEndpoint,
                offlineSigner
            );
            setClient(client);

            const balance = await client.getBalance(accounts[0].address, denom);
            setBalance(balance);

        } catch (err) {
            console.error(err);
            setError('Failed to connect local: ' + err.message);
        }
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
