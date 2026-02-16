import React, { useState } from 'react';
import {
    Dialog,
    DialogTitle,
    DialogContent,
    DialogActions,
    Button,
    TextField,
    Box,
    Alert,
    Typography,
    IconButton,
    Tooltip
} from '@mui/material';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import CloseIcon from '@mui/icons-material/Close';
import { TokenModalProps, toMicroUnits, DEFAULT_CHAIN_CONFIG } from '../../types/FrontendTypes';

const CommitModal: React.FC<TokenModalProps> = ({
    open,
    onClose,
    token,
    client,
    address
}) => {
    const [amount, setAmount] = useState('');
    const [maxSpread, setMaxSpread] = useState('0.005');
    const [deadline, setDeadline] = useState('20');
    const [status, setStatus] = useState('');
    const [txHash, setTxHash] = useState('');
    const [copySuccess, setCopySuccess] = useState(false);
    const [loading, setLoading] = useState(false);

    const handleCommit = async () => {
        if (!client || !address) {
            setStatus('Please connect wallet');
            return;
        }

        try {
            setLoading(true);
            setStatus('Subscribing...');
            setTxHash('');

            const amountVal = parseFloat(amount);
            if (isNaN(amountVal) || amountVal <= 0) {
                setStatus('Error: Please enter a valid positive amount');
                setLoading(false);
                return;
            }

            const amountInMicroUnits = toMicroUnits(amount, DEFAULT_CHAIN_CONFIG.coinDecimals);
            const thresholdStatus = await client.queryContractSmart(token.poolAddress, {
                is_fully_commited: {}
            });
            const isThresholdCrossed = thresholdStatus === 'fully_committed';
            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? ((Date.now() + parseFloat(deadline) * 60 * 1000) * 1_000_000).toString()
                : null;

            const commitMsg = {
                asset: {
                    info: { bluechip: { denom: DEFAULT_CHAIN_CONFIG.nativeDenom } },
                    amount: amountInMicroUnits
                },
                amount: amountInMicroUnits,
                transaction_deadline: deadlineInNs,
                belief_price: null as string | null,
                max_spread: (isThresholdCrossed && maxSpread && parseFloat(maxSpread) > 0)
                    ? maxSpread : null as string | null
            };

            const msg = { commit: commitMsg };
            const funds = [{ denom: DEFAULT_CHAIN_CONFIG.nativeDenom, amount: amountInMicroUnits }];

            const result = await client.execute(
                address,
                token.poolAddress,
                msg,
                { amount: [], gas: '600000' },
                'Commit',
                funds
            );

            setTxHash(result.transactionHash);
            setStatus('Success! Transaction confirmed.');
        } catch (err) {
            console.error('Commit error:', err);
            setStatus('Error: ' + (err as Error).message);
            setTxHash('');
        } finally {
            setLoading(false);
        }
    };

    const handleCopyTxHash = () => {
        navigator.clipboard.writeText(txHash);
        setCopySuccess(true);
        setTimeout(() => setCopySuccess(false), 2000);
    };

    const handleClose = () => {
        setAmount('');
        setStatus('');
        setTxHash('');
        onClose();
    };

    return (
        <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
            <DialogTitle>
                <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <Typography variant="h6">
                        Subscribe to {token.symbol}
                    </Typography>
                    <IconButton onClick={handleClose} size="small">
                        <CloseIcon />
                    </IconButton>
                </Box>
            </DialogTitle>

            <DialogContent>
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2, pt: 1 }}>
                    <Typography variant="body2" color="text.secondary">
                        Commit funds to {token.name} ({token.symbol})
                    </Typography>

                    <TextField
                        label="Amount (Bluechips)"
                        value={amount}
                        onChange={(e) => setAmount(e.target.value)}
                        type="number"
                        fullWidth
                        helperText="Amount of bluechips to commit"
                    />

                    <TextField
                        label="Max Spread"
                        value={maxSpread}
                        onChange={(e) => setMaxSpread(e.target.value)}
                        fullWidth
                        helperText="e.g. 0.005 for 0.5%"
                    />

                    <TextField
                        label="Deadline (minutes)"
                        value={deadline}
                        onChange={(e) => setDeadline(e.target.value)}
                        type="number"
                        fullWidth
                        helperText="Transaction deadline"
                    />

                    {status && (
                        <Alert
                            severity={
                                status.includes('Success') ? 'success' :
                                    status.includes('Error') ? 'error' : 'info'
                            }
                        >
                            {status}
                        </Alert>
                    )}

                    {txHash && (
                        <Box
                            sx={{
                                p: 2,
                                bgcolor: 'success.light',
                                borderRadius: 1,
                                border: '1px solid',
                                borderColor: 'success.main'
                            }}
                        >
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
                                <Tooltip title={copySuccess ? 'Copied!' : 'Copy'}>
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
            </DialogContent>

            <DialogActions sx={{ px: 3, pb: 2 }}>
                <Button onClick={handleClose}>Cancel</Button>
                <Button
                    variant="contained"
                    color="secondary"
                    onClick={handleCommit}
                    disabled={loading || !amount}
                >
                    {loading ? 'Processing...' : 'Subscribe'}
                </Button>
            </DialogActions>
        </Dialog>
    );
};

export default CommitModal;