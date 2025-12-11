import React from 'react';
import { TextField, Box, Typography } from '@mui/material';

interface ContractConfigProps {
    contractAddress: string;
    setContractAddress: (address: string) => void;
}

const ContractConfig = ({ contractAddress, setContractAddress }: ContractConfigProps) => {
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
