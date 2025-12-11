import React, { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert, Tabs, Tab, IconButton, Tooltip } from '@mui/material';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import CommitTracker from './CommitTracker';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';

interface CommitProps {
    client: SigningCosmWasmClient | null;
    address: string;
}

const Commit = ({ client, address }: CommitProps) => {
    const [tab, setTab] = useState(0);
    const [targetContractAddress, setTargetContractAddress] = useState('');
    const [amount, setAmount] = useState('');
    const [maxSpread, setMaxSpread] = useState('0.005'); // Default 0.5%
    const [deadline, setDeadline] = useState('20'); // Default 20 minutes
    const [status, setStatus] = useState('');
    const [txHash, setTxHash] = useState('');
    const [copySuccess, setCopySuccess] = useState(false);

    const handleSubscribe = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }
        try {
            setStatus('Subscribing...');
            setTxHash('');
            setCopySuccess(false);

            const amountVal = parseFloat(amount);
            if (isNaN(amountVal) || amountVal <= 0) {
                setStatus('Error: Please enter a valid positive amount');
                return;
            }
            const amountInMicroUnits = Math.floor(amountVal * 1_000_000).toString();
            const thresholdStatus = await client.queryContractSmart(targetContractAddress, {
                is_fully_commited: {}
            });
            const isThresholdCrossed = thresholdStatus === 'fully_committed';
            const commitMsg: {
                asset: {
                    info: {
                        bluechip: { denom: string }
                    },
                    amount: string
                },
                amount: string,
                transaction_deadline?: string,
                max_spread?: string
            } = {
                asset: {
                    info: {
                        bluechip: { denom: 'stake' }
                    },
                    amount: amountInMicroUnits
                },
                amount: amountInMicroUnits
            };

            if (deadline && parseFloat(deadline) > 0) {
                const deadlineInNs = (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000;
                commitMsg.transaction_deadline = deadlineInNs.toString();
            }

            if (isThresholdCrossed && maxSpread && parseFloat(maxSpread) > 0) {
                commitMsg.max_spread = maxSpread;
            }

            const msg = {
                commit: commitMsg
            };

            const funds = [{ denom: 'stake', amount: amountInMicroUnits }];

            console.log('Sending commit message:', JSON.stringify(msg, null, 2));
            console.log('With funds:', funds);

            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "600000" // Explicit gas limit
                },
                "Commit",
                funds
            );
            console.log("Transaction Hash:", result.transactionHash);
            setTxHash(result.transactionHash);
            setStatus('Success! Transaction confirmed.');
        } catch (err) {
            console.error('Full error:', err);
            setStatus('Error: ' + (err as Error).message);
            setTxHash('');
        }
    };

    const handleCopyTxHash = () => {
        navigator.clipboard.writeText(txHash);
        setCopySuccess(true);
        setTimeout(() => setCopySuccess(false), 2000);
    };

    return (
        <Card sx={{ mb: 2 }}>
            <CardContent>
                <Typography variant="h6" gutterBottom>Subscribe & Track</Typography>

                <Tabs value={tab} onChange={(e, v) => setTab(v)} sx={{ mb: 2 }}>
                    <Tab label="Subscribe" />
                    <Tab label="Progress" />
                </Tabs>

                {tab === 0 && (
                    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                        <TextField
                            label="Contract Address (Creator Pool Address)"
                            value={targetContractAddress}
                            onChange={(e) => setTargetContractAddress(e.target.value)}
                            placeholder="wasm1..."
                            helperText="Address of the pool contract"
                        />
                        <TextField
                            label="Amount (bluechip)"
                            value={amount}
                            onChange={(e) => setAmount(e.target.value)}
                            type="number"
                            helperText="Amount of bluechip tokens to commit"
                        />

                        <TextField
                            label="Max Spread (Decimal)"
                            value={maxSpread}
                            onChange={(e) => setMaxSpread(e.target.value)}
                            helperText="e.g. 0.005 for 0.5%"
                        />

                        <TextField
                            label="Deadline (minutes)"
                            value={deadline}
                            onChange={(e) => setDeadline(e.target.value)}
                            type="number"
                            helperText="Transaction deadline in minutes"
                        />

                        <Button variant="contained" color="primary" onClick={handleSubscribe}>
                            Subscribe
                        </Button>
                        {status && <Alert severity={status.includes('Success') ? 'success' : status.includes('Error') ? 'error' : 'info'}>{status}</Alert>}

                        {txHash && (
                            <Box sx={{
                                p: 2,
                                bgcolor: 'success.light',
                                borderRadius: 1,
                                border: '1px solid',
                                borderColor: 'success.main'
                            }}>
                                <Typography variant="subtitle2" sx={{ mb: 1, fontWeight: 'bold' }}>
                                    Transaction Hash:
                                </Typography>
                                <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                                    <Typography
                                        variant="body2"
                                        sx={{
                                            fontFamily: 'monospace',
                                            wordBreak: 'break-all',
                                            flex: 1,
                                            fontSize: '0.85rem'
                                        }}
                                    >
                                        {txHash}
                                    </Typography>
                                    <Tooltip title={copySuccess ? "Copied!" : "Copy to clipboard"}>
                                        <IconButton
                                            size="small"
                                            onClick={handleCopyTxHash}
                                            color={copySuccess ? "success" : "primary"}
                                        >
                                            <ContentCopyIcon fontSize="small" />
                                        </IconButton>
                                    </Tooltip>
                                </Box>
                            </Box>
                        )}
                    </Box>
                )}

                {tab === 1 && (
                    <CommitTracker client={client} address={address} contractAddress={targetContractAddress} />
                )}
            </CardContent>
        </Card>
    );
};

export default Commit;
