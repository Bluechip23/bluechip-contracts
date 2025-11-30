import React, { useState, useEffect } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert } from '@mui/material';
import { coins } from '@cosmjs/stargate';

const Swap = ({ client, address, contractAddress }) => {
    const [offerAsset, setOfferAsset] = useState('');
    const [amount, setAmount] = useState('');
    const [maxSpread, setMaxSpread] = useState('0.005'); // Default 0.5%
    const [deadline, setDeadline] = useState('20'); // Default 20 minutes
    const [targetContractAddress, setTargetContractAddress] = useState(contractAddress || '');
    const [status, setStatus] = useState('');

    // Sync with global contract address if it changes
    useEffect(() => {
        if (contractAddress) {
            setTargetContractAddress(contractAddress);
        }
    }, [contractAddress]);

    const handleSwap = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }

        try {
            setStatus('Swapping...');

            // Convert amount to micro-units
            const amountVal = parseFloat(amount);
            if (isNaN(amountVal) || amountVal <= 0) {
                setStatus('Error: Please enter a valid positive amount');
                return;
            }
            const amountInMicroUnits = Math.floor(amountVal * 1_000_000).toString();

            const isNative = !offerAsset.startsWith('wasm');

            const tokenInfo = isNative
                ? { native_token: { denom: offerAsset } }
                : { token: { contract_addr: offerAsset } };

            // Calculate deadline in nanoseconds (optional - use null if not provided)
            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000
                : null;

            const msg = {
                simple_swap: {
                    offer_asset: {
                        info: tokenInfo,
                        amount: amountInMicroUnits
                    },
                    belief_price: null, // Optional
                    max_spread: maxSpread || null, // Optional - defaults to contract's default
                    to: null, // Optional recipient
                    transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                }
            };

            const funds = isNative ? coins(amountInMicroUnits, offerAsset) : [];

            // Use explicit gas limit instead of "auto" for better reliability
            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "500000" // Explicit gas limit
                },
                "Swap",
                funds
            );

            console.log("Transaction Hash:", result.transactionHash);
            setStatus(`Success! Tx Hash: ${result.transactionHash}`);
        } catch (err) {
            console.error(err);
            setStatus('Error: ' + err.message);
        }
    };

    return (
        <Card sx={{ mb: 2 }}>
            <CardContent>
                <Typography variant="h6" gutterBottom>Standard Swap</Typography>
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                    <TextField
                        label="Swap Contract Address"
                        value={targetContractAddress}
                        onChange={(e) => setTargetContractAddress(e.target.value)}
                        placeholder="wasm1..."
                        helperText="Address of the pool contract to swap with"
                    />
                    <TextField
                        label="Offer Asset (Denom)"
                        value={offerAsset}
                        onChange={(e) => setOfferAsset(e.target.value)}
                        helperText="e.g. ucosm"
                    />
                    <TextField
                        label="Amount"
                        value={amount}
                        onChange={(e) => setAmount(e.target.value)}
                        type="number"
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
                    <Button variant="contained" color="secondary" onClick={handleSwap}>
                        Swap
                    </Button>
                    {status && <Alert severity={status.includes('Success') ? 'success' : 'info'}>{status}</Alert>}
                </Box>
            </CardContent>
        </Card>
    );
};

export default Swap;
