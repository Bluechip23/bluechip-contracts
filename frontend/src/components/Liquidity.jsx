import React, { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert, Tabs, Tab } from '@mui/material';
import { coins } from '@cosmjs/stargate';

const Liquidity = ({ client, address }) => {
    const [tab, setTab] = useState(0);
    const [amount0, setAmount0] = useState('');
    const [amount1, setAmount1] = useState('');
    const [positionId, setPositionId] = useState('');
    const [removeAmount, setRemoveAmount] = useState('');
    const [slippage, setSlippage] = useState('1'); // Default 1%
    const [deadline, setDeadline] = useState('20'); // Default 20 minutes
    const [removeMode, setRemoveMode] = useState('amount'); // 'amount' or 'percent'
    const [removePercent, setRemovePercent] = useState('');
    const [targetContractAddress, setTargetContractAddress] = useState('');
    const [status, setStatus] = useState('');

    const handleDeposit = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }
        try {
            setStatus('Depositing...');

            // Convert amounts to micro-units
            const amount0Val = parseFloat(amount0);
            const amount1Val = parseFloat(amount1);
            if (isNaN(amount0Val) || amount0Val <= 0 || isNaN(amount1Val) || amount1Val <= 0) {
                setStatus('Error: Please enter valid positive amounts');
                return;
            }
            const amount0Micro = Math.floor(amount0Val * 1_000_000).toString();
            const amount1Micro = Math.floor(amount1Val * 1_000_000).toString();

            // Calculate min amounts based on slippage (optional)
            const slipFactor = slippage && parseFloat(slippage) > 0
                ? 1 - (parseFloat(slippage) / 100)
                : 0.99; // Default 1% slippage
            const minAmount0 = Math.floor(parseFloat(amount0Micro) * slipFactor).toString();
            const minAmount1 = Math.floor(parseFloat(amount1Micro) * slipFactor).toString();

            // Calculate deadline in nanoseconds (optional)
            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000
                : null;

            const msg = {
                deposit_liquidity: {
                    amount0: amount0Micro,
                    amount1: amount1Micro,
                    min_amount0: minAmount0 || null,
                    min_amount1: minAmount1 || null,
                    transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                }
            };

            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "500000" // Explicit gas limit
                },
                "Deposit Liquidity",
                []
            );
            console.log("Transaction Hash:", result.transactionHash);
            setStatus(`Success! Tx Hash: ${result.transactionHash}`);
        } catch (err) {
            console.error(err);
            setStatus('Error: ' + err.message);
        }
    };

    const handleRemove = async () => {
        if (!client || !address || !targetContractAddress) {
            setStatus('Please connect wallet and set contract address');
            return;
        }
        try {
            setStatus('Removing...');
            // Convert slippage % to BPS (basis points). 1% = 100 bps (optional)
            const deviationBps = slippage && parseFloat(slippage) > 0
                ? Math.floor(parseFloat(slippage) * 100)
                : null;

            setStatus('Verifying ownership...');
            const positionInfo = await client.queryContractSmart(targetContractAddress, {
                position: { position_id: positionId }
            });

            if (positionInfo.owner !== address) {
                setStatus('Error: You do not own this position');
                return;
            }

            setStatus('Removing...');

            // Calculate deadline in nanoseconds (optional)
            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000
                : null;

            let msg;
            if (removeMode === 'amount') {
                // Convert remove amount to micro-units
                const removeVal = parseFloat(removeAmount);
                if (isNaN(removeVal) || removeVal <= 0) {
                    setStatus('Error: Please enter a valid positive amount to remove');
                    return;
                }
                const removeMicro = Math.floor(removeVal * 1_000_000).toString();

                msg = {
                    remove_partial_liquidity: {
                        position_id: positionId,
                        liquidity_to_remove: removeMicro,
                        min_amount0: null,
                        min_amount1: null,
                        max_ratio_deviation_bps: deviationBps,
                        transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                    }
                };
            } else {
                msg = {
                    remove_partial_liquidity_by_percent: {
                        position_id: positionId,
                        percentage: parseInt(removePercent),
                        min_amount0: null,
                        min_amount1: null,
                        max_ratio_deviation_bps: deviationBps,
                        transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                    }
                };
            }

            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "500000" // Explicit gas limit
                },
                "Remove Liquidity"
            );
            console.log("Transaction Hash:", result.transactionHash);
            setStatus(`Success! Tx Hash: ${result.transactionHash}`);
        } catch (err) {
            console.error(err);
            setStatus('Error: ' + err.message);
        }
    };

    const handleAddToPosition = async () => {
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

            setStatus('Adding to position...');

            // Convert amounts to micro-units
            const amount0Val = parseFloat(amount0);
            const amount1Val = parseFloat(amount1);
            if (isNaN(amount0Val) || amount0Val <= 0 || isNaN(amount1Val) || amount1Val <= 0) {
                setStatus('Error: Please enter valid positive amounts');
                return;
            }
            const amount0Micro = Math.floor(amount0Val * 1_000_000).toString();
            const amount1Micro = Math.floor(amount1Val * 1_000_000).toString();

            // Calculate min amounts based on slippage (optional)
            const slipFactor = slippage && parseFloat(slippage) > 0
                ? 1 - (parseFloat(slippage) / 100)
                : 0.99; // Default 1% slippage
            const minAmount0 = Math.floor(parseFloat(amount0Micro) * slipFactor).toString();
            const minAmount1 = Math.floor(parseFloat(amount1Micro) * slipFactor).toString();

            // Calculate deadline in nanoseconds (optional)
            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? (Date.now() + (parseFloat(deadline) * 60 * 1000)) * 1000000
                : null;

            const msg = {
                add_to_position: {
                    position_id: positionId,
                    amount0: amount0Micro,
                    amount1: amount1Micro,
                    min_amount0: minAmount0 || null,
                    min_amount1: minAmount1 || null,
                    transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                }
            };

            const result = await client.execute(
                address,
                targetContractAddress,
                msg,
                {
                    amount: [],
                    gas: "500000" // Explicit gas limit
                },
                "Add to Position",
                []
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
                <Typography variant="h6" gutterBottom>Liquidity Management</Typography>

                <TextField
                    fullWidth
                    label="Contract Address"
                    value={targetContractAddress}
                    onChange={(e) => setTargetContractAddress(e.target.value)}
                    placeholder="wasm1..."
                    helperText="Address of the pool contract"
                    sx={{ mb: 2 }}
                />

                <Tabs value={tab} onChange={(e, v) => setTab(v)} sx={{ mb: 2 }}>
                    <Tab label="Provide Liquidity" />
                    <Tab label="Add to Position" />
                    <Tab label="Remove Liquidity" />
                </Tabs>

                {tab === 0 && (
                    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                        <TextField
                            label="Amount 0"
                            value={amount0}
                            onChange={(e) => setAmount0(e.target.value)}
                            type="number"
                        />
                        <TextField
                            label="Amount 1"
                            value={amount1}
                            onChange={(e) => setAmount1(e.target.value)}
                            type="number"
                        />
                        <TextField
                            label="Slippage Tolerance (%)"
                            value={slippage}
                            onChange={(e) => setSlippage(e.target.value)}
                            type="number"
                            helperText="e.g. 1 for 1%"
                        />
                        <TextField
                            label="Deadline (minutes)"
                            value={deadline}
                            onChange={(e) => setDeadline(e.target.value)}
                            type="number"
                            helperText="Transaction deadline in minutes"
                        />
                        <Alert severity="info">Note: Ensure you have approved CW20 tokens or attached native funds (not fully supported in this simple UI yet).</Alert>
                        <Button variant="contained" onClick={handleDeposit}>
                            Provide Liquidity
                        </Button>
                    </Box>
                )}

                {tab === 1 && (
                    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                        <TextField
                            label="Position ID"
                            value={positionId}
                            onChange={(e) => setPositionId(e.target.value)}
                        />
                        <TextField
                            label="Amount 0"
                            value={amount0}
                            onChange={(e) => setAmount0(e.target.value)}
                            type="number"
                        />
                        <TextField
                            label="Amount 1"
                            value={amount1}
                            onChange={(e) => setAmount1(e.target.value)}
                            type="number"
                        />
                        <TextField
                            label="Slippage Tolerance (%)"
                            value={slippage}
                            onChange={(e) => setSlippage(e.target.value)}
                            type="number"
                            helperText="e.g. 1 for 1%"
                        />
                        <TextField
                            label="Deadline (minutes)"
                            value={deadline}
                            onChange={(e) => setDeadline(e.target.value)}
                            type="number"
                            helperText="Transaction deadline in minutes"
                        />
                        <Button variant="contained" color="primary" onClick={handleAddToPosition}>
                            Add to Position
                        </Button>
                    </Box>
                )}

                {tab === 2 && (
                    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                        <TextField
                            label="Position ID"
                            value={positionId}
                            onChange={(e) => setPositionId(e.target.value)}
                        />

                        <Box sx={{ display: 'flex', gap: 2, mb: 1 }}>
                            <Button
                                variant={removeMode === 'amount' ? 'contained' : 'outlined'}
                                onClick={() => setRemoveMode('amount')}
                            >
                                Amount
                            </Button>
                            <Button
                                variant={removeMode === 'percent' ? 'contained' : 'outlined'}
                                onClick={() => setRemoveMode('percent')}
                            >
                                Percentage
                            </Button>
                        </Box>

                        {removeMode === 'amount' ? (
                            <TextField
                                label="Liquidity to Remove"
                                value={removeAmount}
                                onChange={(e) => setRemoveAmount(e.target.value)}
                                type="number"
                            />
                        ) : (
                            <TextField
                                label="Percentage to Remove (0-100)"
                                value={removePercent}
                                onChange={(e) => setRemovePercent(e.target.value)}
                                type="number"
                                inputProps={{ min: 0, max: 100 }}
                            />
                        )}

                        <TextField
                            label="Max Ratio Deviation (%)"
                            value={slippage}
                            onChange={(e) => setSlippage(e.target.value)}
                            type="number"
                            helperText="e.g. 1 for 1%"
                        />
                        <TextField
                            label="Deadline (minutes)"
                            value={deadline}
                            onChange={(e) => setDeadline(e.target.value)}
                            type="number"
                            helperText="Transaction deadline in minutes"
                        />
                        <Button variant="contained" color="error" onClick={handleRemove}>
                            Remove Liquidity
                        </Button>
                    </Box>
                )}

                {status && <Alert severity={status.includes('Success') ? 'success' : 'info'} sx={{ mt: 2 }}>{status}</Alert>}
            </CardContent>
        </Card>
    );
};

export default Liquidity;
