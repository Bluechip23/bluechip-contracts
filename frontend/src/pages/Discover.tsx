import React, { useState } from 'react';
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
    TextField,
    InputAdornment
} from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import { Coin } from '@cosmjs/stargate';
import AddIcon from '@mui/icons-material/Add';
import RemoveIcon from '@mui/icons-material/Remove';
import InfoOutlinedIcon from '@mui/icons-material/InfoOutlined';
import SearchIcon from '@mui/icons-material/Search';
import WalletConnect from '../components/WalletConnect';
import CommitModal from '../components/modals/CommitModal';
import BuyModal from '../components/modals/BuyModal';
import { DiscoverToken } from '../types/FrontendTypes';


interface Token {
    address: string;
    name: string;
    symbol: string;
    price: string;
    priceChange24h: number;
    volume24h: string;
    marketCap: string;
    poolAddress: string;
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

const mockTokens: DiscoverToken[] = [
    {
        tokenAddress: 'cosmos1abc...', // was 'address'
        poolAddress: 'cosmos1pool...',
        name: 'Example Token',
        symbol: 'EXT',
        decimals: 6, // added
        price: '$0.45',
        priceChange24h: 5.2,
        volume24h: '$12,500',
        marketCap: '$450,000',
        thresholdReached: true
    },
    {
        tokenAddress: 'cosmos1def...', // was 'address'
        poolAddress: 'cosmos1pool2...',
        name: 'New Creator Token',
        symbol: 'NCT',
        decimals: 6, // added
        price: '$0.12',
        priceChange24h: -2.1,
        volume24h: '$3,200',
        marketCap: '$120,000',
        thresholdReached: false
    }
];

interface TokenRowProps {
    token: DiscoverToken;
    client: SigningCosmWasmClient | null;
    address: string;
    onBuyClick: (token: DiscoverToken) => void;
    onCommitClick: (token: DiscoverToken) => void;
    onInfoClick: (token: DiscoverToken) => void;
}

const TokenRow: React.FC<TokenRowProps> = ({
    token,
    client,
    address,
    onBuyClick,
    onCommitClick,
    onInfoClick
}) => {
    const [expanded, setExpanded] = useState(false);

    return (
        <>
            <TableRow
                sx={{
                    '&:hover': { bgcolor: 'action.hover' },
                    cursor: 'pointer'
                }}
            >
                <TableCell>
                    <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
                        <Typography fontWeight="bold">{token.symbol}</Typography>
                        <Typography variant="body2" color="text.secondary">
                            {token.name}
                        </Typography>
                    </Box>
                </TableCell>
                <TableCell>{token.price}</TableCell>
                <TableCell>
                    <Typography
                        color={token.priceChange24h >= 0 ? 'success.main' : 'error.main'}
                    >
                        {token.priceChange24h >= 0 ? '+' : ''}{token.priceChange24h}%
                    </Typography>
                </TableCell>
                <TableCell>{token.volume24h}</TableCell>
                <TableCell>{token.marketCap}</TableCell>
                <TableCell>
                    <Box sx={{ display: 'flex', gap: 0.5 }}>
                        <IconButton
                            size="small"
                            onClick={(e) => {
                                e.stopPropagation();
                                onInfoClick(token);
                            }}
                            title="This will also open the accordian thing but have a graph pertaining to the tokens trading histort"
                        >
                            <InfoOutlinedIcon fontSize="small" />
                        </IconButton>
                        <IconButton
                            size="small"
                            onClick={(e) => {
                                e.stopPropagation();
                                setExpanded(!expanded);
                            }}
                            title="Actions"
                        >
                            {expanded ? <RemoveIcon fontSize="small" /> : <AddIcon fontSize="small" />}
                        </IconButton>
                    </Box>
                </TableCell>
            </TableRow>
            <TableRow>
                <TableCell colSpan={6} sx={{ py: 0, borderBottom: expanded ? undefined : 'none' }}>
                    <Collapse in={expanded} timeout="auto" unmountOnExit>
                        <Box sx={{ py: 2, display: 'flex', gap: 2, justifyContent: 'flex-end' }}>
                            <Button
                                variant="contained"
                                color="primary"
                                size="small"
                                onClick={() => onBuyClick(token)}
                                disabled={!client || !address}
                            >
                                Buy
                            </Button>
                            <Button
                                variant="contained"
                                color="secondary"
                                size="small"
                                onClick={() => onCommitClick(token)}
                                disabled={!client || !address}
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

const DiscoverPage: React.FC = () => {
    const [client, setClient] = useState<SigningCosmWasmClient | null>(null);
    const [address, setAddress] = useState<string>('');
    const [balance, setBalance] = useState<Coin | null>(null);
    const [tabValue, setTabValue] = useState(0);
    const [searchQuery, setSearchQuery] = useState('');
    const [buyModalOpen, setBuyModalOpen] = useState(false);
    const [commitModalOpen, setCommitModalOpen] = useState(false);
    const [infoModalOpen, setInfoModalOpen] = useState(false);
    const [selectedToken, setSelectedToken] = useState<DiscoverToken | null>(null);

    const handleBuyClick = (token: DiscoverToken) => {
        setSelectedToken(token);
        setBuyModalOpen(true);
    };

    const handleCommitClick = (token: DiscoverToken) => {
        setSelectedToken(token);
        setCommitModalOpen(true);
    };

    const handleInfoClick = (token: DiscoverToken) => {
        setSelectedToken(token);
        setInfoModalOpen(true);
    };

    const filteredTokens = mockTokens.filter(token =>
        token.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        token.symbol.toLowerCase().includes(searchQuery.toLowerCase())
    );

    return (
        <Container>
            <Typography variant="h3" align="center" gutterBottom sx={{ mb: 2 }}>
                Discover
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
                <Tabs
                    value={tabValue}
                    onChange={(_, newValue) => setTabValue(newValue)}
                    sx={{ borderBottom: 1, borderColor: 'divider', px: 2 }}
                >
                    <Tab label="Tokens" />
                    <Tab label="Pools" />
                </Tabs>

                <TabPanel value={tabValue} index={0}>
                    <Box sx={{ px: 2, pb: 2 }}>
                        <TextField
                            fullWidth
                            size="small"
                            placeholder="Search tokens..."
                            value={searchQuery}
                            onChange={(e) => setSearchQuery(e.target.value)}
                            InputProps={{
                                startAdornment: (
                                    <InputAdornment position="start">
                                        <SearchIcon />
                                    </InputAdornment>
                                )
                            }}
                            sx={{ mb: 2 }}
                        />

                        <TableContainer>
                            <Table>
                                <TableHead>
                                    <TableRow>
                                        <TableCell>Token</TableCell>
                                        <TableCell>Price</TableCell>
                                        <TableCell>24h Change</TableCell>
                                        <TableCell>Volume</TableCell>
                                        <TableCell>Market Cap</TableCell>
                                        <TableCell align="right">Actions</TableCell>
                                    </TableRow>
                                </TableHead>
                                <TableBody>
                                    {filteredTokens.map((token) => (
                                        <TokenRow
                                            key={token.tokenAddress}
                                            token={token}
                                            client={client}
                                            address={address}
                                            onBuyClick={handleBuyClick}
                                            onCommitClick={handleCommitClick}
                                            onInfoClick={handleInfoClick}
                                        />
                                    ))}
                                </TableBody>
                            </Table>
                        </TableContainer>
                    </Box>
                </TabPanel>

                <TabPanel value={tabValue} index={1}>
                    <Box sx={{ p: 3, textAlign: 'center' }}>
                        <Typography color="text.secondary">
                            Pools tab coming soon...
                        </Typography>
                    </Box>
                </TabPanel>
            </Paper>

            {selectedToken && (
                <>
                    <CommitModal
                        open={commitModalOpen}
                        onClose={() => setCommitModalOpen(false)}
                        token={selectedToken}
                        client={client}
                        address={address} />
                    <BuyModal
                        open={buyModalOpen}
                        onClose={() => setBuyModalOpen(false)}
                        token={selectedToken}
                        client={client}
                        address={address} />
                </>
            )}
        </Container>
    );
};

export default DiscoverPage;