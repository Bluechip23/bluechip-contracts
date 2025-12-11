// pages/PortfolioPage.tsx
import React, { useState, useEffect, useCallback } from 'react';
import {
    Container,
    Typography,
    Box,
    Paper,
    Tabs,
    Tab,
    Table,
    TableBody,
    TableCell,
    TableContainer,
    TableHead,
    TableRow,
    IconButton,
    Collapse,
    Button,
    CircularProgress,
    Alert
} from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import { Coin } from '@cosmjs/stargate';
import AddIcon from '@mui/icons-material/Add';
import RemoveIcon from '@mui/icons-material/Remove';
import InfoOutlinedIcon from '@mui/icons-material/InfoOutlined';
import RefreshIcon from '@mui/icons-material/Refresh';
import WalletConnect from '../components/WalletConnect';
import BuyModal from '../components/modals/BuyModal';
import CommitModal from '../components/modals/CommitModal';
import TokenInfoModal from '../components/modals/TokenInfoModal';
import SellModal from '../components/modals/SellModal';

const FACTORY_ADDRESS = 'cosmos1factory...'; // Replace with your factory address

interface TokenType {
    creator_token?: { contract_addr: string };
    bluechip?: { denom: string };
}

interface PoolDetails {
    asset_infos: [TokenType, TokenType];
    contract_addr: string;
    pool_type: string;
}

interface PortfolioToken {
    tokenAddress: string;
    poolAddress: string;
    name: string;
    symbol: string;
    decimals: number;
    balance: string;
    thresholdReached: boolean;
}

interface TabPanelProps {
    children?: React.ReactNode;
    index: number;
    value: number;
}

const TabPanel: React.FC<TabPanelProps> = ({ children, value, index }) => (
    <div role="tabpanel" hidden={value !== index}>
        {value === index && <Box sx={{ py: 3 }}>{children}</Box>}
    </div>
);

interface TokenRowProps {
    token: PortfolioToken;
    onBuyClick: (token: PortfolioToken) => void;
    onSellClick: (token: PortfolioToken) => void;
    onSubscribeClick: (token: PortfolioToken) => void;
    onInfoClick: (token: PortfolioToken) => void;
}

const TokenRow: React.FC<TokenRowProps> = ({
    token,
    onBuyClick,
    onSellClick,
    onSubscribeClick,
    onInfoClick
}) => {
    const [expanded, setExpanded] = useState(false);

    const formatBalance = (balance: string, decimals: number): string => {
        const num = parseInt(balance) / Math.pow(10, decimals);
        return num.toLocaleString(undefined, { maximumFractionDigits: decimals });
    };

    return (
        <>
            <TableRow sx={{ '&:hover': { bgcolor: 'action.hover' } }}>
                <TableCell>
                    <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                        <Typography fontWeight="bold">{token.symbol}</Typography>
                        <Typography variant="body2" color="text.secondary">
                            {token.name}
                        </Typography>
                    </Box>
                </TableCell>
                <TableCell>
                    {formatBalance(token.balance, token.decimals)}
                </TableCell>
                <TableCell>
                    <Typography
                        variant="body2"
                        sx={{
                            color: token.thresholdReached ? 'success.main' : 'warning.main'
                        }}
                    >
                        {token.thresholdReached ? 'Active' : 'Pre-launch'}
                    </Typography>
                </TableCell>
                <TableCell>
                    <Box sx={{ display: 'flex', gap: 0.5 }}>
                        <IconButton
                            size="small"
                            onClick={() => onInfoClick(token)}
                            title="Token Info"
                        >
                            <InfoOutlinedIcon fontSize="small" />
                        </IconButton>
                        <IconButton
                            size="small"
                            onClick={() => setExpanded(!expanded)}
                            title="Actions"
                        >
                            {expanded ? <RemoveIcon fontSize="small" /> : <AddIcon fontSize="small" />}
                        </IconButton>
                    </Box>
                </TableCell>
            </TableRow>
            <TableRow>
                <TableCell colSpan={4} sx={{ py: 0, borderBottom: expanded ? undefined : 'none' }}>
                    <Collapse in={expanded} timeout="auto" unmountOnExit>
                        <Box sx={{ py: 2, display: 'flex', gap: 2, justifyContent: 'flex-end' }}>
                            <Button
                                variant="contained"
                                color="primary"
                                size="small"
                                onClick={() => onBuyClick(token)}
                            >
                                Buy
                            </Button>
                            <Button
                                variant="contained"
                                color="error"
                                size="small"
                                onClick={() => onSellClick(token)}
                                disabled={!token.thresholdReached}
                                title={!token.thresholdReached ? 'Pool must reach threshold before selling' : ''}
                            >
                                Sell
                            </Button>
                            <Button
                                variant="contained"
                                color="secondary"
                                size="small"
                                onClick={() => onSubscribeClick(token)}
                            >
                                Subscribe
                            </Button>
                        </Box>
                    </Collapse>
                </TableCell>
            </TableRow>
        </>
    );
};

const PortfolioPage: React.FC = () => {
    const [client, setClient] = useState<SigningCosmWasmClient | null>(null);
    const [address, setAddress] = useState<string>('');
    const [balance, setBalance] = useState<Coin | null>(null);
    const [tabValue, setTabValue] = useState(0);

    // Token data
    const [tokens, setTokens] = useState<PortfolioToken[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string>('');

    // Modal states
    const [buyModalOpen, setBuyModalOpen] = useState(false);
    const [sellModalOpen, setSellModalOpen] = useState(false);
    const [commitModalOpen, setCommitModalOpen] = useState(false);
    const [infoModalOpen, setInfoModalOpen] = useState(false);
    const [selectedToken, setSelectedToken] = useState<PortfolioToken | null>(null);

    const fetchPortfolioTokens = useCallback(async () => {
        if (!client || !address) return;

        setLoading(true);
        setError('');

        try {
            // 1. Get all pools from factory
            const allPoolsResponse = await client.queryContractSmart(FACTORY_ADDRESS, {
                get_all_pools: {}
            });

            const pools: [string, { pool_contract_address: string }][] = allPoolsResponse.pools;

            // 2. For each pool, get details and token info in parallel
            const tokenPromises = pools.map(async ([_poolId, poolState]) => {
                try {
                    const poolAddress = poolState.pool_contract_address;

                    // Get pool details to find creator token address
                    const poolDetails: PoolDetails = await client.queryContractSmart(poolAddress, {
                        pair_info: {}
                    });

                    // Find the creator token in asset_infos
                    const creatorTokenInfo = poolDetails.asset_infos.find(
                        (asset): asset is { creator_token: { contract_addr: string } } =>
                            'creator_token' in asset
                    );

                    if (!creatorTokenInfo) return null;

                    const tokenAddress = creatorTokenInfo.creator_token.contract_addr;

                    // Query token balance, token info, and threshold status in parallel
                    const [balanceResponse, tokenInfo, thresholdStatus] = await Promise.all([
                        client.queryContractSmart(tokenAddress, {
                            balance: { address }
                        }),
                        client.queryContractSmart(tokenAddress, {
                            token_info: {}
                        }),
                        client.queryContractSmart(poolAddress, {
                            check_threshold_limit: {}
                        })
                    ]);

                    // Only include if user has balance
                    if (balanceResponse.balance === '0') return null;

                    const thresholdReached = 'fully_committed' in thresholdStatus;

                    return {
                        tokenAddress,
                        poolAddress,
                        name: tokenInfo.name,
                        symbol: tokenInfo.symbol,
                        decimals: tokenInfo.decimals,
                        balance: balanceResponse.balance,
                        thresholdReached
                    } as PortfolioToken;

                } catch (err) {
                    console.error('Error fetching pool data:', err);
                    return null;
                }
            });

            const results = await Promise.all(tokenPromises);
            const validTokens = results.filter((t): t is PortfolioToken => t !== null);

            setTokens(validTokens);
        } catch (err) {
            console.error('Error fetching portfolio:', err);
            setError('Failed to load portfolio: ' + (err as Error).message);
        } finally {
            setLoading(false);
        }
    }, [client, address]);

    // Fetch tokens when wallet connects
    useEffect(() => {
        if (client && address) {
            fetchPortfolioTokens();
        }
    }, [client, address, fetchPortfolioTokens]);

    const handleBuyClick = (token: PortfolioToken) => {
        setSelectedToken(token);
        setBuyModalOpen(true);
    };

    const handleSellClick = (token: PortfolioToken) => {
        setSelectedToken(token);
        setSellModalOpen(true);
    };

    const handleSubscribeClick = (token: PortfolioToken) => {
        setSelectedToken(token);
        setCommitModalOpen(true);
    };

    const handleInfoClick = (token: PortfolioToken) => {
        setSelectedToken(token);
        setInfoModalOpen(true);
    };

    return (
        <Container maxWidth="lg" sx={{ py: 4 }}>
            <Typography variant="h3" align="center" gutterBottom sx={{ mb: 2 }}>
                Portfolio
            </Typography>

            <Box sx={{ mb: 4, textAlign: 'center' }}>
                <WalletConnect
                    setClient={setClient}
                    setAddress={setAddress}
                    setBalance={setBalance}
                />
                {balance && (
                    <Typography variant="body1" sx={{ mt: 2 }}>
                        Balance: {(parseInt(balance.amount) / 1_000_000).toFixed(2)} {balance.denom}
                    </Typography>
                )}
            </Box>

            <Paper sx={{ width: '100%' }}>
                <Box sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', px: 2, pt: 1 }}>
                    <Tabs
                        value={tabValue}
                        onChange={(_, newValue) => setTabValue(newValue)}
                        sx={{ borderBottom: 1, borderColor: 'divider' }}
                    >
                        <Tab label="Tokens" />
                        <Tab label="Liquidity Positions" />
                    </Tabs>
                    {client && address && (
                        <IconButton onClick={fetchPortfolioTokens} disabled={loading} title="Refresh">
                            <RefreshIcon />
                        </IconButton>
                    )}
                </Box>

                <TabPanel value={tabValue} index={0}>
                    <Box sx={{ px: 2, pb: 2 }}>
                        {!client || !address ? (
                            <Alert severity="info">Connect your wallet to view your portfolio</Alert>
                        ) : loading ? (
                            <Box sx={{ display: 'flex', justifyContent: 'center', py: 4 }}>
                                <CircularProgress />
                            </Box>
                        ) : error ? (
                            <Alert severity="error">{error}</Alert>
                        ) : tokens.length === 0 ? (
                            <Alert severity="info">No tokens found in your portfolio</Alert>
                        ) : (
                            <TableContainer>
                                <Table>
                                    <TableHead>
                                        <TableRow>
                                            <TableCell>Token</TableCell>
                                            <TableCell>Balance</TableCell>
                                            <TableCell>Status</TableCell>
                                            <TableCell align="right">Actions</TableCell>
                                        </TableRow>
                                    </TableHead>
                                    <TableBody>
                                        {tokens.map((token) => (
                                            <TokenRow
                                                key={token.tokenAddress}
                                                token={token}
                                                onBuyClick={handleBuyClick}
                                                onSellClick={handleSellClick}
                                                onSubscribeClick={handleSubscribeClick}
                                                onInfoClick={handleInfoClick}
                                            />
                                        ))}
                                    </TableBody>
                                </Table>
                            </TableContainer>
                        )}
                    </Box>
                </TabPanel>

                <TabPanel value={tabValue} index={1}>
                    <Box sx={{ p: 3, textAlign: 'center' }}>
                        <Typography color="text.secondary">
                            Liquidity positions coming soon...
                        </Typography>
                    </Box>
                </TabPanel>
            </Paper>

            {selectedToken && (
                <>
                    <BuyModal
                        open={buyModalOpen}
                        onClose={() => setBuyModalOpen(false)}
                        token={selectedToken}
                        client={client}
                        address={address}
                    />
                    <SellModal
                        open={sellModalOpen}
                        onClose={() => setSellModalOpen(false)}
                        token={selectedToken}
                        client={client}
                        address={address}
                    />
                    <CommitModal
                        open={commitModalOpen}
                        onClose={() => setCommitModalOpen(false)}
                        token={selectedToken}
                        client={client}
                        address={address}
                    />
                    <TokenInfoModal
                        open={infoModalOpen}
                        onClose={() => setInfoModalOpen(false)}
                        token={selectedToken}
                    />
                </>
            )}
        </Container>
    );
};

export default PortfolioPage;