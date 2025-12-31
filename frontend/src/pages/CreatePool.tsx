import React, { useState } from 'react';
import { Container, Typography, Box, Paper } from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import { Coin } from '@cosmjs/stargate';
import WalletConnect from '../components/WalletConnect';
import CreatePool from '../components/CreatePool';

const CreatePoolPage: React.FC = () => {
    const [client, setClient] = useState<SigningCosmWasmClient | null>(null);
    const [address, setAddress] = useState<string>('');
    const [balance, setBalance] = useState<Coin | null>(null);

    return (
        <Container>
            <Box sx={{ mb: 4, textAlign: 'center' }}>
                <WalletConnect
                    setClient={setClient}
                    setAddress={setAddress}
                    setBalance={setBalance}
                />
                {balance && (
                    <Typography variant="body1" sx={{ mt: 2 }}>
                        Balance: {balance.amount} {balance.denom}
                    </Typography>
                )}
            </Box>
            <Typography variant="h3" align="center" gutterBottom sx={{ mb: 2 }}>
                Create Your Pool
            </Typography>

            <Paper elevation={2} sx={{ p: 3, mb: 4, bgcolor: 'background.default' }}>
                <Typography variant="h6" gutterBottom sx={{ fontWeight: 'bold', color: 'primary.main' }}>
                    Launch Your Creator Token
                </Typography>
                <Typography variant="body1" sx={{ mb: 2 }}>
                    Create a new Creator Liquidity Pool with your custom token. This process will deploy a new token contract
                    and establish a trading pool paired with BlueChip tokens. This pool exists on the BlueChip chain. This
                    means BlueChip does not own your pool and has no authority to shut your pool down or discontinue your tokens.
                    Since the pool exists on chain, this allows you to bring your subscriptions everywhere you go. The "payment
                    gateway" exists on chain and any frontend can link to it. This includes linking to any website you put content on,
                    sponsorship websites, or even connecting your friends content to your pool to create mini joint channels for
                    colaborations.
                </Typography>
                <Typography variant="body1" color="text.secondary" >
                    <strong>How it works:</strong>
                </Typography>
                <Box component="ul" sx={{ pl: 2, mt: 1 }}>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        Choose a unique name and ticker symbol for your token
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        The wallet you currently have connected will be the wallet used for your token affiliation.
                        IMPORTANT: DO NOT LOSE AS WE WILL NOT BE ABLE TO RECOVER YOUR WALLET IF YOU LOSE IT!
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        The pool requires $25,000 USD in commits to activate
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        You will recieve fees from all COMMIT transactions ONLY, unless you provde liquidty to your pool
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        Once the threshold is met, your token becomes tradeable
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        Once the threshold is met, your pool can now accept liquidity and providers can collect fees
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        Once the threshold is crossed, you will receive creator rewards and initial liquidity allocation. This will happen automatically. Just pay attention to your wallet!
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        The initial crossing values are as follows:
                        <br />To you the Creator: <strong>325,000</strong>,
                        <br />To BlueChip: <strong>25,000</strong>,
                        <br />To your initial subscribers based on the % of the $25,000 they subscribed: <strong>500,000</strong>,
                        <br />Initial liquidity to your pool: <strong>350,000</strong>
                    </Typography>
                    <Typography component="li" variant="body2" color="text.secondary" sx={{ mb: 1 }}>
                        You can not mint any extra creator tokens. The <strong>1,200,000</strong> is a fixed amount.
                    </Typography>
                    <Typography component="li" variant="h5" color="text.secondary" sx={{ mb: 1 }}>
                        <strong>Good luck and welcome to BlueChip!</strong>
                    </Typography>
                </Box>
            </Paper>
            <CreatePool client={client} address={address} />
        </Container>
    );
};

export default CreatePoolPage;
