import React, { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert, IconButton, Tooltip } from '@mui/material';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';

// Factory contract address - configured during deployment. Will need to make dynamic
const FACTORY_ADDRESS = 'cosmos1yvgh8xeju5dyr0zxlkvq09htvhjj20fncp5g58np4u25g8rkpgjst8ghg8';
interface CreatePoolProps {
    client: SigningCosmWasmClient | null;
    address: string;
}

const CreatePool = ({ client, address }: CreatePoolProps) => {
    const [tokenName, setTokenName] = useState('');
    const [tokenSymbol, setTokenSymbol] = useState('');
    const [status, setStatus] = useState('');
    const [txHash, setTxHash] = useState('');
    const [copySuccess, setCopySuccess] = useState(false);

    // Default configuration values based on factory settings
    const DEFAULT_CONFIG = {
        commitThresholdUsd: '25000000000', // $25,000 in micro-units
        commitAmountForThreshold: '25000000000',
        maxBluechipLock: '10000000000',
        creatorExcessLockDays: 7,
        cw20CodeId: 1,
        decimal: 6,
        // Threshold payout distribution (must sum to total mint)
        thresholdPayout: {
            creator_reward_amount: '325000000000',
            bluechip_reward_amount: '25000000000',
            pool_seed_amount: '350000000000',
            commit_return_amount: '500000000000'
        },
        commitFeeInfo: {
            commit_fee_bluechip: '0.01', // 1%
            commit_fee_creator: '0.05'   // 5%
        }
    };

    const handleCreatePool = async () => {
        if (!client || !address) {
            setStatus('Please connect your wallet');
            return;
        }

        if (!tokenName || !tokenSymbol) {
            setStatus('Error: Please enter both token name and symbol');
            return;
        }

        try {
            setStatus('Creating pool...');
            setTxHash('');
            setCopySuccess(false);

            // Encode threshold payout to base64
            const thresholdPayoutJson = JSON.stringify(DEFAULT_CONFIG.thresholdPayout);
            const thresholdPayoutB64 = btoa(thresholdPayoutJson);

            // Build the Create message for the factory
            const createMsg = {
                create: {
                    pool_msg: {
                        pool_token_info: [
                            { bluechip: { denom: 'stake' } },
                            { creator_token: { contract_addr: 'WILL_BE_CREATED_BY_FACTORY' } }
                        ],
                        cw20_token_contract_id: DEFAULT_CONFIG.cw20CodeId,
                        factory_to_create_pool_addr: FACTORY_ADDRESS,
                        threshold_payout: thresholdPayoutB64,
                        commit_fee_info: {
                            bluechip_wallet_address: address, // Use connected wallet
                            creator_wallet_address: address,   // Use connected wallet
                            commit_fee_bluechip: DEFAULT_CONFIG.commitFeeInfo.commit_fee_bluechip,
                            commit_fee_creator: DEFAULT_CONFIG.commitFeeInfo.commit_fee_creator
                        },
                        creator_token_address: address, // Placeholder, will be set by factory
                        commit_amount_for_threshold: DEFAULT_CONFIG.commitAmountForThreshold,
                        commit_limit_usd: DEFAULT_CONFIG.commitThresholdUsd,
                        pyth_contract_addr_for_conversions: 'oracle_address_placeholder', // TODO: Get from factory config
                        pyth_atom_usd_price_feed_id: 'ATOM_USD',
                        max_bluechip_lock_per_pool: DEFAULT_CONFIG.maxBluechipLock,
                        creator_excess_liquidity_lock_days: DEFAULT_CONFIG.creatorExcessLockDays
                    },
                    token_info: {
                        name: tokenName,
                        symbol: tokenSymbol,
                        decimal: DEFAULT_CONFIG.decimal
                    }
                }
            };

            console.log('Creating pool with message:', JSON.stringify(createMsg, null, 2));

            // Execute the Create message on the factory contract
            const result = await client.execute(
                address,
                FACTORY_ADDRESS,
                createMsg,
                {
                    amount: [],
                    gas: '2000000' // Higher gas limit for pool creation
                },
                'Create Pool'
            );

            console.log('Transaction Hash:', result.transactionHash);
            setTxHash(result.transactionHash);
            setStatus('Success! Pool creation transaction submitted.');

            // Clear form on success
            setTokenName('');
            setTokenSymbol('');
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
                <Typography variant="h6" gutterBottom>Create Pool</Typography>
                <Typography variant="body2" color="text.secondary" sx={{ mb: 2 }}>
                    Create a new creator pool with your custom token
                </Typography>

                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                    <TextField
                        label="Token Name"
                        value={tokenName}
                        onChange={(e) => setTokenName(e.target.value)}
                        placeholder="My Creator Token"
                        helperText="Full name of your token"
                        required
                    />

                    <TextField
                        label="Token Symbol (Ticker)"
                        value={tokenSymbol}
                        onChange={(e) => setTokenSymbol(e.target.value.toUpperCase())}
                        placeholder="MCT"
                        helperText="Short ticker symbol (e.g., BTC, ETH)"
                        required
                        inputProps={{ maxLength: 10 }}
                    />

                    <Box sx={{ p: 2, bgcolor: 'info.light', borderRadius: 1 }}>
                        <Typography variant="subtitle2" sx={{ fontWeight: 'bold', mb: 1 }}>
                            Pool Configuration (Pre-set)
                        </Typography>
                        <Typography variant="body2">• Commit Threshold: $25,000 USD</Typography>
                        <Typography variant="body2">• Commit Fee: 1% BlueChip, 5% Creator</Typography>
                        <Typography variant="body2">• Max BlueChip Lock: 10,000 tokens</Typography>
                        <Typography variant="body2">• Liquidity Lock: 7 days</Typography>
                    </Box>

                    <Button
                        variant="contained"
                        color="primary"
                        onClick={handleCreatePool}
                        disabled={!client || !address || !tokenName || !tokenSymbol}
                    >
                        Create Pool
                    </Button>

                    {status && (
                        <Alert severity={status.includes('Success') ? 'success' : status.includes('Error') ? 'error' : 'info'}>
                            {status}
                        </Alert>
                    )}

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
                            <Typography
                                variant="caption"
                                component="a"
                                href={`https://www.mintscan.io/cosmwasm-testnet/tx/${txHash}`}
                                target="_blank"
                                rel="noopener noreferrer"
                                sx={{
                                    display: 'block',
                                    mt: 1,
                                    color: 'primary.dark',
                                    textDecoration: 'underline',
                                    '&:hover': {
                                        color: 'primary.main'
                                    }
                                }}
                            >
                                View on Mintscan →
                            </Typography>
                        </Box>
                    )}
                </Box>
            </CardContent>
        </Card>
    );
};

export default CreatePool;
