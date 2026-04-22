use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    to_json_binary, Addr, Api, BalanceResponse, BankQuery, QuerierWrapper, QueryRequest, StdError,
    StdResult, Uint128, WasmQuery,
};
use cw20::{BalanceResponse as Cw20BalanceResponse, Cw20QueryMsg, TokenInfoResponse};
use std::fmt::{self, Display, Formatter, Result};

#[cw_serde]
pub struct TokenInfo {
    pub info: TokenType,
    pub amount: Uint128,
}

impl fmt::Display for TokenInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.info)
    }
}

impl TokenInfo {
    pub fn is_native_token(&self) -> bool {
        self.info.is_native_token()
    }
}

#[cw_serde]
pub enum TokenType {
    CreatorToken { contract_addr: Addr },
    /// Any native bank denom on the chain — bluechip itself (`ubluechip`),
    /// IBC-wrapped remote assets (e.g. `ibc/...` for ATOM), tokenfactory
    /// denoms, etc. Name was formerly `Bluechip`, which was semantically
    /// misleading because a `Native { denom: "ibc/..." }` entry represents
    /// an IBC asset, not bluechip. The wire tag stays `"bluechip"` via
    /// `#[serde(rename = ...)]` so on-chain serialized state, deploy
    /// scripts, and frontend integrations continue to round-trip without
    /// a coordinated migration.
    #[serde(rename = "bluechip")]
    Native { denom: String },
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TokenType::Native { denom } => write!(f, "{}", denom),
            TokenType::CreatorToken { contract_addr } => write!(f, "{}", contract_addr),
        }
    }
}

impl TokenType {
    pub fn is_native_token(&self) -> bool {
        match self {
            TokenType::Native { .. } => true,
            TokenType::CreatorToken { .. } => false,
        }
    }

    pub fn is_token_an_ibc_token(&self) -> bool {
        match self {
            TokenType::Native { denom } => denom.to_lowercase().starts_with("ibc/"),
            TokenType::CreatorToken { .. } => false,
        }
    }

    pub fn query_pool(&self, querier: &QuerierWrapper, pool_addr: Addr) -> StdResult<Uint128> {
        match self {
            TokenType::CreatorToken { contract_addr, .. } => {
                query_token_balance(querier, contract_addr.clone(), pool_addr)
            }
            TokenType::Native { denom, .. } => {
                query_balance(querier, pool_addr, denom.to_string())
            }
        }
    }

    pub fn equal(&self, asset: &TokenType) -> bool {
        match (self, asset) {
            (
                TokenType::CreatorToken { contract_addr: a },
                TokenType::CreatorToken { contract_addr: b },
            ) => a == b,
            (TokenType::Native { denom: a }, TokenType::Native { denom: b }) => a == b,
            _ => false,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            TokenType::Native { denom } => denom.as_bytes(),
            TokenType::CreatorToken { contract_addr } => contract_addr.as_bytes(),
        }
    }

    pub fn check(&self, api: &dyn Api) -> StdResult<()> {
        if let TokenType::CreatorToken { contract_addr } = self {
            api.addr_validate(contract_addr.as_str())?;
        }
        Ok(())
    }
}

#[cw_serde]
pub enum PoolPairType {
    Xyk {},
    Stable {},
}

impl Display for PoolPairType {
    fn fmt(&self, fmt: &mut Formatter) -> Result {
        match self {
            PoolPairType::Xyk {} => fmt.write_str("xyk"),
            PoolPairType::Stable {} => fmt.write_str("stable"),
        }
    }
}

// Returns a lowercased, validated address upon success if present.
pub fn addr_opt_validate(api: &dyn Api, addr: &Option<String>) -> StdResult<Option<Addr>> {
    addr.as_ref()
        .map(|addr| api.addr_validate(addr))
        .transpose()
}

pub fn native_asset(denom: String, amount: Uint128) -> TokenInfo {
    TokenInfo {
        info: TokenType::Native { denom },
        amount,
    }
}

pub fn token_asset(contract_addr: Addr, amount: Uint128) -> TokenInfo {
    TokenInfo {
        info: TokenType::CreatorToken { contract_addr },
        amount,
    }
}

pub fn native_asset_info(denom: String) -> TokenType {
    TokenType::Native { denom }
}

pub fn token_asset_info(contract_addr: Addr) -> TokenType {
    TokenType::CreatorToken { contract_addr }
}

// Extracts the native bluechip denom from a pool's asset_infos array.
pub fn get_native_denom(asset_infos: &[TokenType; 2]) -> StdResult<String> {
    for asset in asset_infos {
        if let TokenType::Native { denom } = asset {
            return Ok(denom.clone());
        }
    }
    Err(StdError::generic_err(
        "No bluechip (native) asset found in pool asset_infos",
    ))
}

pub trait TokenTypeExt {
    fn with_balance(&self, balance: impl Into<Uint128>) -> TokenInfo;
}

impl TokenTypeExt for TokenType {
    fn with_balance(&self, balance: impl Into<Uint128>) -> TokenInfo {
        TokenInfo {
            info: self.clone(),
            amount: balance.into(),
        }
    }
}

/// Queries a CW20 token balance for a given account.
pub fn query_token_balance(
    querier: &QuerierWrapper,
    contract_addr: Addr,
    account_addr: Addr,
) -> StdResult<Uint128> {
    let res: Cw20BalanceResponse = querier
        .query(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: String::from(contract_addr),
            msg: to_json_binary(&Cw20QueryMsg::Balance {
                address: String::from(account_addr),
            })?,
        }))
        .unwrap_or_else(|_| Cw20BalanceResponse {
            balance: Uint128::zero(),
        });
    Ok(res.balance)
}

/// Queries a CW20 token's symbol.
pub fn query_token_symbol(querier: &QuerierWrapper, contract_addr: Addr) -> StdResult<String> {
    let res: TokenInfoResponse = querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: String::from(contract_addr),
        msg: to_json_binary(&Cw20QueryMsg::TokenInfo {})?,
    }))?;
    Ok(res.symbol)
}

/// Queries a native bank balance for a given account and denom.
pub fn query_balance(
    querier: &QuerierWrapper,
    account_addr: Addr,
    denom: String,
) -> StdResult<Uint128> {
    let balance: BalanceResponse = querier.query(&QueryRequest::Bank(BankQuery::Balance {
        address: String::from(account_addr),
        denom,
    }))?;
    Ok(balance.amount.amount)
}

/// Queries the current token balances for a pair of asset types at a given contract address.
pub fn query_pools(
    asset_infos: &[TokenType; 2],
    querier: &QuerierWrapper,
    contract_addr: Addr,
) -> StdResult<[TokenInfo; 2]> {
    Ok([
        TokenInfo {
            amount: asset_infos[0].query_pool(querier, contract_addr.clone())?,
            info: asset_infos[0].clone(),
        },
        TokenInfo {
            amount: asset_infos[1].query_pool(querier, contract_addr)?,
            info: asset_infos[1].clone(),
        },
    ])
}
