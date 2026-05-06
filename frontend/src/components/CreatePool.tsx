import { useState } from 'react';
import { Card, CardContent, Typography, TextField, Button, Box, Alert, IconButton, Tooltip } from '@mui/material';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import { DEFAULT_CHAIN_CONFIG } from '../types/FrontendTypes';

// Factory contract address - configured during deployment.
const FACTORY_ADDRESS =
    import.meta.env.VITE_FACTORY_ADDRESS ||
    'cosmos1yvgh8xeju5dyr0zxlkvq09htvhjj20fncp5g58np4u25g8rkpgjst8ghg8';

interface CreatePoolProps {
    client: SigningCosmWasmClient | null;
    address: string;
}

const CreatePool = ({ client, address }: CreatePoolProps) => {
    const [tokenName, setTokenName] = useState('');
    const [tokenSymbol, setTokenSymbol] = useState('');
    const [isStandardPool, setIsStandardPool] = useState(false);
    const [standardPairAddress, setStandardPairAddress] = useState('');
    const [standardPoolLabel, setStandardPoolLabel] = useState('');
    const [status, setStatus] = useState('');
    const [txHash, setTxHash] = useState('');
    const [copySuccess, setCopySuccess] = useState(false);

    const handleCreatePool = async () => {
        if (!client || !address) {
            setStatus('Please connect your wallet');
            return;
        }

        try {
            setStatus('Creating pool...');
            setTxHash('');
            setCopySuccess(false);

            let createMsg: Record<string, unknown>;
            let gas = '2000000';

            if (isStandardPool) {
                if (!standardPairAddress) {
                    setStatus('Error: Standard pool requires a CW20 pair address');
                    return;
                }
                // CreateStandardPool { pool_token_info, label }. Caller pays the
                // factory's standard_pool_creation_fee_usd in ubluechip; the
                // factory converts USD -> bluechip via the internal oracle.
                createMsg = {
                    create_standard_pool: {
                        pool_token_info: [
                            { bluechip: { denom: DEFAULT_CHAIN_CONFIG.nativeDenom } },
                            { creator_token: { contract_addr: standardPairAddress } },
                        ],
                        label:
                            standardPoolLabel ||
                            `${DEFAULT_CHAIN_CONFIG.nativeDenom}-${tokenSymbol || 'XYK'}-xyk`,
                    },
                };
            } else {
                if (!tokenName || !tokenSymbol) {
                    setStatus('Error: Creator pools require a token name and symbol');
                    return;
                }
                // Create { pool_msg, token_info }. Only `pool_token_info` and the
                // CW20 metadata are caller-supplied; commit threshold, fee splits,
                // threshold-payout amounts, lock caps, and oracle config are read
                // from factory config.
                createMsg = {
                    create: {
                        pool_msg: {
                            pool_token_info: [
                                { bluechip: { denom: DEFAULT_CHAIN_CONFIG.nativeDenom } },
                                { creator_token: { contract_addr: 'WILL_BE_CREATED_BY_FACTORY' } },
                            ],
                        },
                        token_info: {
                            name: tokenName,
                            symbol: tokenSymbol,
                            // Pool enforces 6 decimals to match hardcoded payout amounts.
                            decimal: 6,
                        },
                    },
                };
            }

            console.log('Creating pool with message:', JSON.stringify(createMsg, null, 2));

            const result = await client.execute(
                address,
                FACTORY_ADDRESS,
                createMsg,
                { amount: [], gas },
                isStandardPool ? 'CreateStandardPool' : 'Create',
            );

            console.log('Transaction Hash:', result.transactionHash);
            setTxHash(result.transactionHash);
            setStatus('Success! Pool creation transaction submitted.');

            setTokenName('');
            setTokenSymbol('');
            setStandardPairAddress('');
            setStandardPoolLabel('');
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
                    Creator pools start in commit phase and mint a fresh CW20. Standard pools wrap two pre-existing assets and are immediately tradeable.
                </Typography>

                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                    <Box sx={{
                        p: 2,
                        border: '1px solid',
                        borderColor: isStandardPool ? 'primary.main' : 'divider',
                        borderRadius: 1,
                        bgcolor: isStandardPool ? 'primary.50' : 'background.paper',
                        cursor: 'pointer',
                        transition: 'all 0.2s ease',
                    }}
                        onClick={() => setIsStandardPool(!isStandardPool)}
                    >
                        <Box sx={{ display: 'flex', alignItems: 'center', mb: 1 }}>
                            <div style={{
                                width: 20,
                                height: 20,
                                borderRadius: 4,
                                border: `2px solid ${isStandardPool ? '#1976d2' : '#757575'}`,
                                marginRight: 12,
                                display: 'flex',
                                alignItems: 'center',
                                justifyContent: 'center',
                                backgroundColor: isStandardPool ? '#1976d2' : 'transparent',
                            }}>
                                {isStandardPool && <span style={{ color: 'white', fontSize: 14 }}>OK</span>}
                            </div>
                            <Typography variant="subtitle2" sx={{ fontWeight: 'bold' }}>
                                Create as Standard Pool (xyk)
                            </Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" sx={{ display: 'block', ml: 4 }}>
                            Permissionless. Wraps an existing CW20 paired against bluechip. Caller pays the factory&#39;s USD-denominated creation fee in ubluechip. Skips commit phase and the threshold-payout mint.
                        </Typography>
                    </Box>

                    {!isStandardPool && (
                        <>
                            <TextField
                                label="Token Name"
                                value={tokenName}
                                onChange={(e) => setTokenName(e.target.value)}
                                placeholder="My Creator Token"
                                helperText="Full name of the new CW20 the factory will mint"
                                required
                            />
                            <TextField
                                label="Token Symbol (Ticker)"
                                value={tokenSymbol}
                                onChange={(e) => setTokenSymbol(e.target.value.toUpperCase())}
                                placeholder="MCT"
                                helperText="Short ticker symbol (e.g. BTC, ETH)"
                                required
                                inputProps={{ maxLength: 10 }}
                            />
                            <Box sx={{ p: 2, bgcolor: 'info.light', borderRadius: 1 }}>
                                <Typography variant="subtitle2" sx={{ fontWeight: 'bold', mb: 1 }}>
                                    Factory-Configured (read at call time)
                                </Typography>
                                <Typography variant="body2">- Commit threshold (USD)</Typography>
                                <Typography variant="body2">- Commit fee splits (bluechip / creator)</Typography>
                                <Typography variant="body2">- Threshold-payout amounts (creator / bluechip / pool seed / committers)</Typography>
                                <Typography variant="body2">- Max bluechip lock per pool & creator excess lock days</Typography>
                                <Typography variant="body2">- Pyth oracle address & price feed id</Typography>
                                <Typography variant="caption" color="text.secondary" sx={{ display: 'block', mt: 1 }}>
                                    The frontend no longer forwards these — the factory consults its own stored config. Per-address create cooldown: 1h.
                                </Typography>
                            </Box>
                        </>
                    )}

                    {isStandardPool && (
                        <>
                            <TextField
                                label="CW20 Pair Address"
                                value={standardPairAddress}
                                onChange={(e) => setStandardPairAddress(e.target.value)}
                                placeholder="cosmos1..."
                                helperText="Pre-existing CW20 to pair against bluechip"
                                required
                            />
                            <TextField
                                label="Pool Label (optional)"
                                value={standardPoolLabel}
                                onChange={(e) => setStandardPoolLabel(e.target.value)}
                                placeholder="ubluechip-MYTOKEN-xyk"
                                helperText="On-chain label used by explorers"
                            />
                            <TextField
                                label="Display Symbol (optional)"
                                value={tokenSymbol}
                                onChange={(e) => setTokenSymbol(e.target.value.toUpperCase())}
                                placeholder="MYTOKEN"
                                helperText="Used to auto-fill the label if you leave it blank"
                                inputProps={{ maxLength: 10 }}
                            />
                        </>
                    )}

                    <Button variant="contained"
                        color="primary"
                        onClick={handleCreatePool}
                        disabled={
                            !client ||
                            !address ||
                            (isStandardPool ? !standardPairAddress : !tokenName || !tokenSymbol)
                        }
                    >
                        {isStandardPool ? 'Create Standard Pool' : 'Create Creator Pool'}
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
                            borderColor: 'success.main',
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
                                        fontSize: '0.85rem',
                                    }}
                                >
                                    {txHash}
                                </Typography>
                                <Tooltip title={copySuccess ? 'Copied!' : 'Copy to clipboard'}>
                                    <IconButton
                                        size="small"
                                        onClick={handleCopyTxHash}
                                        color={copySuccess ? 'success' : 'primary'}
                                    >
                                        <ContentCopyIcon fontSize="small" />
                                    </IconButton>
                                </Tooltip>
                            </Box>
                        </Box>
                    )}
                </Box>
            </CardContent>
        </Card>
    );
};

export default CreatePool;
