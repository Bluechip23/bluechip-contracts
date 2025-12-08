import React, { useState } from 'react';
import { Container, Typography, Box, Grid, CssBaseline } from '@mui/material';
import WalletConnect from './components/WalletConnect';
import Swap from './components/Swap';
import Liquidity from './components/Liquidity';
import Fees from './components/Fees';
import Commit from './components/Commit';
import CreatePool from './components/CreatePool';

function App() {
  const [client, setClient] = useState(null);
  const [address, setAddress] = useState('');
  const [balance, setBalance] = useState(null);

  return (
    <>
      <CssBaseline />
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
          <CreatePool
            client={client}
            address={address}
          />
        </Grid>
        <Grid size={{ xs: 4 }}>
          <Swap
            client={client}
            address={address}
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
}

export default App;
