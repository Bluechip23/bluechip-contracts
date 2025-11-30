import React from 'react';
import { TextField, Box, Typography } from '@mui/material';

const ContractConfig = ({ contractAddress, setContractAddress }) => {
    return (
        <Box sx={{ mb: 3 }}>
            <Typography variant="h6" gutterBottom>
                Contract Configuration
            </Typography>
            <TextField
                fullWidth
                label="Contract Address"
                variant="outlined"
                value={contractAddress}
                onChange={(e) => setContractAddress(e.target.value)}
                placeholder="wasm1..."
            />
        </Box>
    );
};

export default ContractConfig;
