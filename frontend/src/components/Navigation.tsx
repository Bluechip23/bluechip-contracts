import React from 'react';
import { AppBar, Toolbar, Button, Box } from '@mui/material';
import { Link as RouterLink } from 'react-router-dom';

const Navigation: React.FC = () => {
    return (
        <AppBar position="static" sx={{ mb: 4 }}>
            <Toolbar>
                <Box sx={{ flexGrow: 1, display: 'flex', gap: 2 }}>
                    <Button
                        color="inherit"
                        component={RouterLink}
                        to="/createpool"
                        sx={{ fontWeight: 'bold' }}
                    >
                        Create Pool
                    </Button>
                    <Button
                        color="inherit"
                        component={RouterLink}
                        to="/discover"
                        sx={{ fontWeight: 'bold' }}
                    >
                        Discover
                    </Button>
                    <Button
                        color="inherit"
                        component={RouterLink}
                        to="/portfolio"
                        sx={{ fontWeight: 'bold' }}
                    >
                        Portfolio
                    </Button>
                    <Button
                        color="inherit"
                        component={RouterLink}
                        to="/"
                        sx={{ fontWeight: 'bold' }}
                    >
                        Dashboard
                    </Button>
                </Box>
            </Toolbar>
        </AppBar>
    );
};

export default Navigation;
