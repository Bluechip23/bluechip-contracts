// types/index.ts

import { SigningCosmWasmClient } from '@cosmjs/cosmwasm-stargate';


export type TokenType =
    | { creator_token: { contract_addr: string } }
    | { bluechip: { denom: string } };

export interface TokenInfo {
    name: string;
    symbol: string;
    decimals: number;
    total_supply: string;
}

export interface DiscoverToken {
    tokenAddress: string;
    poolAddress: string;
    name: string;
    symbol: string;
    decimals: number;
    price?: string;
    priceChange24h: number;
    volume24h?: string;
    marketCap?: string;
    thresholdReached: boolean;
}

// Token as displayed in Portfolio page (has balance)
export interface PortfolioToken {
    tokenAddress: string;
    poolAddress: string;
    name: string;
    symbol: string;
    decimals: number;
    balance: string;
    thresholdReached: boolean;
}

// Union type for modals that can accept either
export type ModalToken = DiscoverToken | PortfolioToken;

// Type guard to check if token has balance
export const hasBalance = (token: ModalToken): token is PortfolioToken => {
    return 'balance' in token && token.balance !== undefined;
};

export interface PoolDetails {
    asset_infos: [TokenType, TokenType];
    contract_addr: string;
    pool_type: string;
}

export interface PoolStateResponse {
    pool_contract_address: string;
    nft_ownership_accepted: boolean;
    reserve0: string;
    reserve1: string;
    total_liquidity: string;
    block_time_last: number;
    price0_cumulative_last: string;
    price1_cumulative_last: string;
}

export interface AllPoolsResponse {
    pools: [string, PoolStateResponse][];
}


export type CommitStatus =
    | { fully_committed: Record<string, never> }
    | { in_progress: { raised: string; target: string } };

export const isThresholdReached = (status: CommitStatus): boolean => {
    return 'fully_committed' in status;
};


export interface LiquidityPosition {
    positionId: string;
    poolAddress: string;
    tokenA: {
        address: string;
        symbol: string;
        amount: string;
    };
    tokenB: {
        address: string;
        symbol: string;
        amount: string;
    };
    shareOfPool: string;
    unclaimedFees: string;
    nftTokenId?: string;
}

// ============================================
// Modal Props Types
// ============================================

export interface BaseModalProps {
    open: boolean;
    onClose: () => void;
    client: SigningCosmWasmClient | null;
    address: string;
}

export interface TokenModalProps extends BaseModalProps {
    token: ModalToken;
}

export interface InfoModalProps {
    open: boolean;
    onClose: () => void;
    token: ModalToken;
}

// ============================================
// Transaction Types
// ============================================

export interface TransactionResult {
    success: boolean;
    txHash?: string;
    error?: string;
}

// ============================================
// Wallet Types
// ============================================

export interface WalletState {
    client: SigningCosmWasmClient | null;
    address: string;
    balance: {
        amount: string;
        denom: string;
    } | null;
    connected: boolean;
}

// ============================================
// Config Types
// ============================================

export interface ChainConfig {
    chainId: string;
    chainName: string;
    rpc: string;
    rest: string;
    factoryAddress: string;
    nativeDenom: string;
    coinDecimals: number;
}

// Default config - update with your values
export const DEFAULT_CHAIN_CONFIG: ChainConfig = {
    chainId: 'bluechipChain',
    chainName: 'Bluechip Local',
    rpc: 'http://localhost:26657',
    rest: 'http://localhost:1317',
    factoryAddress: 'cosmos1factory...', // Replace with actual
    nativeDenom: 'stake',
    coinDecimals: 6,
};

// ============================================
// Utility Functions
// ============================================

export const formatTokenAmount = (amount: string, decimals: number): string => {
    const num = parseInt(amount) / Math.pow(10, decimals);
    return num.toLocaleString(undefined, { maximumFractionDigits: decimals });
};

export const toMicroUnits = (amount: string, decimals: number): string => {
    const num = parseFloat(amount);
    if (isNaN(num)) return '0';
    return Math.floor(num * Math.pow(10, decimals)).toString();
};

export const fromMicroUnits = (amount: string, decimals: number): number => {
    return parseInt(amount) / Math.pow(10, decimals);
};

// Extract creator token address from pool asset_infos
export const getCreatorTokenAddress = (assetInfos: [TokenType, TokenType]): string | null => {
    const creatorToken = assetInfos.find(
        (asset): asset is { creator_token: { contract_addr: string } } =>
            'creator_token' in asset
    );
    return creatorToken?.creator_token.contract_addr ?? null;
};

// Extract bluechip denom from pool asset_infos
export const getBluechipDenom = (assetInfos: [TokenType, TokenType]): string | null => {
    const bluechip = assetInfos.find(
        (asset): asset is { bluechip: { denom: string } } =>
            'bluechip' in asset
    );
    return bluechip?.bluechip.denom ?? null;
};