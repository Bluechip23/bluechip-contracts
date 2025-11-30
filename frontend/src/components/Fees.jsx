import React, { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert } from '@mui/material';

const Fees = ({ client, address }) => {
    const [positionId, setPositionId] = useState('');
    const [targetContractAddress, setTargetContractAddress] = useState('');
    const [status, setStatus] = useState('');

    const handleCollect = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }
        try {
            setStatus('Verifying ownership...');
            const positionInfo = await client.queryContractSmart(targetContractAddress, {
                position: { position_id: positionId }
            });

            if (positionInfo.owner !== address) {
                setStatus('Error: You do not own this position');
                return;
            }

            setStatus('Collecting fees...');
            const msg = {
                collect_fees: {
                    position_id: positionId,
                }
            };
            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                "auto",
                "Collect Fees"
            );
            setStatus(`Success! Tx Hash: ${result.transactionHash}`);
        } catch (err) {
            console.error(err);
            setStatus('Error: ' + err.message);
        }
    };

    return (
        <Card sx={{ mb: 2 }}>
            <CardContent>
                <Typography variant="h6" gutterBottom>Collect Fees</Typography>
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                    <TextField
                        label="Contract Address (Creator Pool Address)"
                        value={targetContractAddress}
                        onChange={(e) => setTargetContractAddress(e.target.value)}
                        placeholder="wasm1..."
                        helperText="Address of the pool contract"
                    />
                    <TextField
                        label="Position ID"
                        value={positionId}
                        onChange={(e) => setPositionId(e.target.value)}
                    />
                    <Button variant="contained" color="success" onClick={handleCollect}>
                        Collect Fees
                    </Button>
                    {status && <Alert severity={status.includes('Success') ? 'success' : 'info'}>{status}</Alert>}
                </Box>
            </CardContent>
        </Card>
    );
};

export default Fees;
