import React, { useState } from 'react';
import { BrowserRouter as Router, Routes, Route } from 'react-router-dom';
import { Container, Typography, Box, Grid, CssBaseline } from '@mui/material';
import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';
import { Coin } from '@cosmjs/stargate';
import Navigation from './components/Navigation';
import WalletConnect from './components/WalletConnect';
import Swap from './components/Swap';
import Liquidity from './components/Liquidity';
import Fees from './components/Fees';
import Commit from './components/Commit';
import CreatePoolPage from './pages/CreatePool';
import DiscoverPage from './pages/Discover';
import PortfolioPage from './pages/Portfolio';

function App() {
  const [client, setClient] = useState<SigningCosmWasmClient | null>(null);
  const [address, setAddress] = useState<string>('');
  const [balance, setBalance] = useState<Coin | null>(null);
  const [contractAddress, setContractAddress] = useState<string>('');

  // Dashboard component (original App content)
  const Dashboard = () => (
    <>
      <Typography variant="h3" align="center" gutterBottom>
        Bluechip Interface
      </Typography>

      <Box sx={{ mb: 4, textAlign: 'center' }}>
        <WalletConnect
          setClient={setClient}
          setAddress={setAddress}
          setBalance={setBalance}
        />
        {balance && (
          <Typography variant="body1">
            Balance: {balance.amount} {balance.denom}
          </Typography>
        )}
      </Box>

      <Grid container spacing={4}>
        <Grid size={{ xs: 6 }}>
          <Swap
            client={client}
            address={address}
            contractAddress={contractAddress}
          />
        </Grid>
        <Grid size={{ xs: 6 }}>
          <Liquidity
            client={client}
            address={address}
          />
        </Grid>
        <Grid size={{ xs: 6 }}>
          <Fees
            client={client}
            address={address}
          />
        </Grid>
        <Grid size={{ xs: 6 }}>
          <Commit
            client={client}
            address={address}
          />
        </Grid>
      </Grid>
    </>
  );

  return (
    <Router>
      <CssBaseline />
      <Navigation />
      <Container maxWidth="lg">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/createpool" element={<CreatePoolPage />} />
          <Route path="/discover" element={<DiscoverPage />} />
          <Route path="/portfolio" element={<PortfolioPage />} />
        </Routes>
      </Container>
    </Router>
  );
}

export default App;

