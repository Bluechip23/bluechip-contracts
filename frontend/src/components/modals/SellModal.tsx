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
import { TokenModalProps, hasBalance, toMicroUnits, formatTokenAmount } from '../../types/FrontendTypes';

const SellModal: React.FC<TokenModalProps> = ({
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

    const tokenBalance = hasBalance(token) ? token.balance : '0';

    const handleSell = async () => {
        if (!client || !address) {
            setStatus('Please connect wallet');
            return;
        }

        try {
            setLoading(true);
            setStatus('Processing swap...');
            setTxHash('');

            const amountVal = parseFloat(amount);
            if (isNaN(amountVal) || amountVal <= 0) {
                setStatus('Error: Please enter a valid positive amount');
                setLoading(false);
                return;
            }

            const amountInMicroUnits = toMicroUnits(amount, token.decimals);

            if (BigInt(amountInMicroUnits) > BigInt(tokenBalance)) {
                setStatus('Error: Amount exceeds balance');
                setLoading(false);
                return;
            }

            const deadlineInNs = deadline && parseFloat(deadline) > 0
                ? (Date.now() + parseFloat(deadline) * 60 * 1000) * 1_000_000
                : null;

            const hookMsg = {
                swap: {
                    belief_price: null,
                    max_spread: maxSpread || null,
                    to: null,
                    transaction_deadline: deadlineInNs ? deadlineInNs.toString() : null
                }
            };

            const encodedMsg = btoa(JSON.stringify(hookMsg));

            const msg = {
                send: {
                    contract: token.poolAddress,
                    amount: amountInMicroUnits,
                    msg: encodedMsg
                }
            };

            const result = await client.execute(
                address,
                token.tokenAddress,
                msg,
                { amount: [], gas: '500000' },
                'Sell Token',
                []
            );

            setTxHash(result.transactionHash);
            setStatus('Success! Transaction confirmed.');
        } catch (err) {
            console.error('Sell error:', err);
            setStatus('Error: ' + (err as Error).message);
            setTxHash('');
        } finally {
            setLoading(false);
        }
    };

    const handleMaxClick = () => {
        const maxAmount = parseInt(tokenBalance) / Math.pow(10, token.decimals);
        setAmount(maxAmount.toString());
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
                        Sell {token.symbol}
                    </Typography>
                    <IconButton onClick={handleClose} size="small">
                        <CloseIcon />
                    </IconButton>
                </Box>
            </DialogTitle>

            <DialogContent>
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2, pt: 1 }}>
                    <Typography variant="body2" color="text.secondary">
                        Swap {token.name} ({token.symbol}) for bluechips
                    </Typography>

                    {hasBalance(token) && (
                        <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                            <Typography variant="body2" color="text.secondary">
                                Available: {formatTokenAmount(token.balance, token.decimals)} {token.symbol}
                            </Typography>
                            <Button size="small" onClick={handleMaxClick}>
                                Max
                            </Button>
                        </Box>
                    )}

                    <TextField
                        label={`Amount (${token.symbol})`}
                        value={amount}
                        onChange={(e) => setAmount(e.target.value)}
                        type="number"
                        fullWidth
                        helperText="Amount of tokens to sell"
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
                    color="error"
                    onClick={handleSell}
                    disabled={loading || !amount}
                >
                    {loading ? 'Processing...' : 'Sell'}
                </Button>
            </DialogActions>
        </Dialog>
    );
};

export default SellModal;