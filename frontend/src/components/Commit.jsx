import React, { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert, Tabs, Tab } from '@mui/material';
import CommitTracker from './CommitTracker';

const Commit = ({ client, address }) => {
    const [tab, setTab] = useState(0);
    const [targetContractAddress, setTargetContractAddress] = useState('');
    const [amount, setAmount] = useState('');
    const [maxSpread, setMaxSpread] = useState('0.005'); // Default 0.5%
    const [deadline, setDeadline] = useState('20'); // Default 20 minutes
    const [status, setStatus] = useState('');

    const handleSubscribe = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }
        try {
            setStatus('Subscribing...');

            // Convert amount to micro-units (ustake)
            const amountVal = parseFloat(amount);
            if (isNaN(amountVal) || amountVal <= 0) {
                setStatus('Error: Please enter a valid positive amount');
                return;
            }
            const amountInMicroUnits = Math.floor(amountVal * 1_000_000).toString();

            // Build the commit message - only include optional fields if they have values
            const commitMsg = {
                asset: {
                    info: {
                        bluechip: { denom: 'stake' }
                    },
                    amount: amountInMicroUnits
                },
                amount: amountInMicroUnits
            };

            // Only add optional fields if they have actual values
            if (deadline && parseFloat(deadline) > 0) {
                const deadlineInNs = (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000;
                commitMsg.transaction_deadline = deadlineInNs.toString();
            }

            if (maxSpread && parseFloat(maxSpread) > 0) {
                commitMsg.max_spread = maxSpread;
            }

            const msg = {
                commit: commitMsg
            };

            const funds = [{ denom: 'stake', amount: amountInMicroUnits }];

            console.log('Sending commit message:', JSON.stringify(msg, null, 2));
            console.log('With funds:', funds);

            // Use explicit gas limit instead of "auto" for better reliability
            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "500000" // Explicit gas limit
                },
                "Commit",
                funds
            );
            console.log("Transaction Hash:", result.transactionHash);
            setStatus(`Success! Tx Hash: ${result.transactionHash}`);
        } catch (err) {
            console.error('Full error:', err);
            setStatus('Error: ' + err.message);
        }
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
                        {status && <Alert severity={status.includes('Success') ? 'success' : 'info'}>{status}</Alert>}
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
