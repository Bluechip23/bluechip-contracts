import React from 'react';
import {
    Dialog,
    DialogTitle,
    DialogContent,
    Box,
    Typography,
    IconButton,
    Divider,
    Chip
} from '@mui/material';
import CloseIcon from '@mui/icons-material/Close';
import { InfoModalProps, hasBalance, formatTokenAmount } from '../../types/FrontendTypes';

const TokenInfoModal: React.FC<InfoModalProps> = ({
    open,
    onClose,
    token
}) => {
    return (
        <Dialog open={open} onClose={onClose} maxWidth="sm" fullWidth>
            <DialogTitle>
                <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                        <Typography variant="h6">
                            {token.name}
                        </Typography>
                        <Chip label={token.symbol} size="small" />
                    </Box>
                    <IconButton onClick={onClose} size="small">
                        <CloseIcon />
                    </IconButton>
                </Box>
            </DialogTitle>

            <DialogContent>
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                    {hasBalance(token) && (
                        <>
                            <Box>
                                <Typography variant="body2" color="text.secondary">
                                    Your Balance
                                </Typography>
                                <Typography variant="h5" fontWeight="bold">
                                    {formatTokenAmount(token.balance, token.decimals)} {token.symbol}
                                </Typography>
                            </Box>
                            <Divider />
                        </>
                    )}

                    {'price' in token && token.price && (
                        <>
                            <Box>
                                <Typography variant="h4" sx={{ fontWeight: 'bold' }}>
                                    {token.price}
                                </Typography>
                                {'priceChange24h' in token && token.priceChange24h !== undefined && (
                                    <Typography
                                        variant="body1"
                                        color={token.priceChange24h >= 0 ? 'success.main' : 'error.main'}
                                    >
                                        {token.priceChange24h >= 0 ? '+' : ''}{token.priceChange24h}% (24h)
                                    </Typography>
                                )}
                            </Box>
                            <Divider />
                        </>
                    )}

                    <Box sx={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 2 }}>
                        {'marketCap' in token && token.marketCap && (
                            <Box>
                                <Typography variant="body2" color="text.secondary">
                                    Market Cap
                                </Typography>
                                <Typography variant="body1" fontWeight="medium">
                                    {token.marketCap}
                                </Typography>
                            </Box>
                        )}
                        {'volume24h' in token && token.volume24h && (
                            <Box>
                                <Typography variant="body2" color="text.secondary">
                                    24h Volume
                                </Typography>
                                <Typography variant="body1" fontWeight="medium">
                                    {token.volume24h}
                                </Typography>
                            </Box>
                        )}
                        <Box>
                            <Typography variant="body2" color="text.secondary">
                                Pool Status
                            </Typography>
                            <Chip
                                label={token.thresholdReached ? 'Active' : 'Pre-launch'}
                                color={token.thresholdReached ? 'success' : 'warning'}
                                size="small"
                            />
                        </Box>
                        <Box>
                            <Typography variant="body2" color="text.secondary">
                                Decimals
                            </Typography>
                            <Typography variant="body1" fontWeight="medium">
                                {token.decimals}
                            </Typography>
                        </Box>
                    </Box>

                    <Divider />

                    <Box>
                        <Typography variant="body2" color="text.secondary" gutterBottom>
                            Token Address
                        </Typography>
                        <Typography
                            variant="body2"
                            sx={{ fontFamily: 'monospace', wordBreak: 'break-all' }}
                        >
                            {token.tokenAddress}
                        </Typography>
                    </Box>

                    <Box>
                        <Typography variant="body2" color="text.secondary" gutterBottom>
                            Pool Address
                        </Typography>
                        <Typography
                            variant="body2"
                            sx={{ fontFamily: 'monospace', wordBreak: 'break-all' }}
                        >
                            {token.poolAddress}
                        </Typography>
                    </Box>

                    <Box
                        sx={{
                            height: 200,
                            bgcolor: 'action.hover',
                            borderRadius: 1,
                            display: 'flex',
                            alignItems: 'center',
                            justifyContent: 'center'
                        }}
                    >
                        <Typography color="text.secondary">
                            Price chart coming soon
                        </Typography>
                    </Box>
                </Box>
            </DialogContent>
        </Dialog>
    );
};

export default TokenInfoModal;