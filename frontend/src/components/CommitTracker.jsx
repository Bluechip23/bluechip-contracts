import React, { useState, useEffect } from 'react';
import { Card, CardContent, Typography, Box, LinearProgress } from '@mui/material';
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, ReferenceLine } from 'recharts';

const CommitTracker = ({ client, address, contractAddress }) => {
    const [commits, setCommits] = useState([]);
    const [totalRaised, setTotalRaised] = useState(0);
    const [totalBluechips, setTotalBluechips] = useState(0);
    const [graphData, setGraphData] = useState([]);
    const [loading, setLoading] = useState(false);
    const THRESHOLD = 25000; // $25,000 threshold

    useEffect(() => {
        if (client && contractAddress) {
            fetchCommits();
        }
    }, [client, contractAddress]);

    const fetchCommits = async () => {
        setLoading(true);
        try {
            // Fetch all commits (pagination might be needed for large datasets, fetching simple list for now)
            const response = await client.queryContractSmart(contractAddress, {
                pool_commits: { limit: 100 }
            });

            if (response && response.commiters) {
                // Sort by timestamp (oldest first) to calculate cumulative progress
                const sortedCommits = [...response.commiters].sort((a, b) => {
                    return parseInt(a.last_commited) - parseInt(b.last_commited);
                });

                let cumulative = 0;
                let bluechipTotal = 0;
                const data = sortedCommits.map((commit, index) => {
                    // Assuming last_payment_usd is the value of the transaction in USD
                    // Note: contract returns Uint128, need to handle decimals if applicable. 
                    // Assuming standard 6 decimals for USD representation or raw integer? 
                    // Based on pool/src/msg.rs, it returns Uint128. Let's assume it's raw units for now, 
                    // but usually USD is 6 decimals.
                    // If commit_threshold_limit_usd is 25000 * 10^6, then we need to divide.
                    // Let's assume the contract handles the scale or we display raw for now.
                    // Checking `pool/src/msg.rs` -> `commit_threshold_limit_usd: Uint128`.
                    // Let's assume 1 unit = $1 for simplicity in this UI unless we see huge numbers.
                    // Actually, usually it's 6 decimals. Let's try to parse as float / 1000000 if values are huge.
                    // For safety, let's just sum them up first.

                    const value = parseInt(commit.last_payment_usd);
                    const bluechipValue = parseInt(commit.last_payment_bluechip);
                    cumulative += value;
                    bluechipTotal += bluechipValue;

                    return {
                        name: `Tx ${index + 1}`,
                        value: value,
                        total: cumulative,
                        timestamp: new Date(parseInt(commit.last_commited) / 1000000).toLocaleString() // ns to ms
                    };
                });

                setCommits(sortedCommits);
                setTotalRaised(cumulative);
                setTotalBluechips(bluechipTotal);
                setGraphData(data);
            }
        } catch (err) {
            console.error("Error fetching commits:", err);
        } finally {
            setLoading(false);
        }
    };

    // Calculate percentage for progress bar
    const displayTotal = totalRaised > 1000000 ? totalRaised / 1000000 : totalRaised;
    const progress = Math.min((displayTotal / THRESHOLD) * 100, 100);

    return (
        <Card sx={{ mb: 2 }}>
            <CardContent>
                <Typography variant="h6" gutterBottom>Subscription Tracker</Typography>

                <Box sx={{ mb: 3 }}>
                    <Box sx={{ display: 'flex', justifyContent: 'space-between', mb: 1 }}>
                        <Typography variant="body2">Raised: ${displayTotal.toLocaleString()}</Typography>
                        <Typography variant="body2">Goal: ${THRESHOLD.toLocaleString()}</Typography>
                    </Box>
                    <LinearProgress variant="determinate" value={progress} sx={{ height: 10, borderRadius: 5 }} />
                    <Box sx={{ display: 'flex', justifyContent: 'space-between', mt: 0.5 }}>
                        <Typography variant="caption" color="textSecondary">
                            bluechips Committed: {totalBluechips.toLocaleString()}
                        </Typography>
                    </Box>
                </Box>

                <Box sx={{ height: 300, width: '100%' }}>
                    <ResponsiveContainer width="100%" height="100%">
                        <LineChart data={graphData} margin={{ top: 5, right: 20, bottom: 20, left: 20 }}>
                            <CartesianGrid stroke="#ccc" strokeDasharray="5 5" />
                            <XAxis dataKey="name" label={{ value: 'Count to 25,000', position: 'insideBottom', offset: -10 }} />
                            <YAxis domain={[0, Math.max(THRESHOLD, displayTotal * 1.1)]} label={{ value: 'Subscription Amount', angle: -90, }} />
                            <Tooltip
                                contentStyle={{ backgroundColor: '#333', border: 'none', color: '#fff' }}
                                labelStyle={{ color: '#aaa' }}
                                formatter={(value, name) => [`$${value}`, name === 'total' ? 'Cumulative Total' : 'Transaction Value']}
                            />
                            <ReferenceLine y={THRESHOLD} label="Goal" stroke="red" strokeDasharray="3 3" />
                            <Line type="monotone" dataKey="total" stroke="#8884d8" strokeWidth={2} dot={false} activeDot={{ r: 8 }} />
                        </LineChart>
                    </ResponsiveContainer>
                </Box>
            </CardContent>
        </Card>
    );
};

export default CommitTracker;
