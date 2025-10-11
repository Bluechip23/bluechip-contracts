use crate::query::{query_balance, query_token_balance};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Api, QuerierWrapper, StdResult, Uint128};
use std::fmt::{self};

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
    pub fn is_bluechip_token(&self) -> bool {
        self.info.is_bluechip_token()
    }
}

#[cw_serde]
pub enum TokenType {
    CreatorToken { contract_addr: Addr },
    Bluechip { denom: String },
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TokenType::Bluechip { denom } => write!(f, "{}", denom),
            TokenType::CreatorToken { contract_addr } => write!(f, "{}", contract_addr),
        }
    }
}

impl TokenType {
    pub fn is_bluechip_token(&self) -> bool {
        match self {
            TokenType::Bluechip { .. } => true,
            TokenType::CreatorToken { .. } => false,
        }
    }

    pub fn query_pool_token_info(&self, querier: &QuerierWrapper, pool_addr: Addr) -> StdResult<Uint128> {
        match self {
            TokenType::CreatorToken { contract_addr, .. } => {
                query_token_balance(querier, contract_addr.clone(), pool_addr)
            }
            TokenType::Bluechip { denom, .. } => {
                query_balance(querier, pool_addr, denom.to_string())
            }
        }
    }

    pub fn equal(&self, asset: &TokenType) -> bool {
        match self {
            TokenType::CreatorToken { contract_addr, .. } => {
                let self_contract_addr = contract_addr;
                match asset {
                    TokenType::CreatorToken { contract_addr, .. } => {
                        self_contract_addr == contract_addr
                    }
                    TokenType::Bluechip { .. } => false,
                }
            }
            TokenType::Bluechip { denom, .. } => {
                let self_denom = denom;
                match asset {
                    TokenType::CreatorToken { .. } => false,
                    TokenType::Bluechip { denom, .. } => self_denom == denom,
                }
            }
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            TokenType::Bluechip { denom } => denom.as_bytes(),
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

pub fn addr_opt_validate(api: &dyn Api, addr: &Option<String>) -> StdResult<Option<Addr>> {
    addr.as_ref()
        .map(|addr| api.addr_validate(addr))
        .transpose()
}

pub fn bluechip_asset(denom: String, amount: Uint128) -> TokenInfo {
    TokenInfo {
        info: TokenType::Bluechip { denom },
        amount,
    }
}

pub fn creator_token_asset(contract_addr: Addr, amount: Uint128) -> TokenInfo {
    TokenInfo {
        info: TokenType::CreatorToken { contract_addr },
        amount,
    }
}

pub fn bluechip_asset_info(denom: String) -> TokenType {
    TokenType::Bluechip { denom }
}

pub fn token_asset_info(contract_addr: Addr) -> TokenType {
    TokenType::CreatorToken { contract_addr }
}

pub trait TokenTypeWithBalance {
    fn with_balance(&self, balance: impl Into<Uint128>) -> TokenInfo;
}

impl TokenTypeWithBalance for TokenType {
    fn with_balance(&self, balance: impl Into<Uint128>) -> TokenInfo {
        TokenInfo {
            info: self.clone(),
            amount: balance.into(),
        }
    }
}
