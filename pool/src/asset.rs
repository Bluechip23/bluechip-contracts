use cosmwasm_schema::cw_serde;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display, Formatter, Result};

use crate::msg::QueryMsg;
use crate::state::PoolInfo;

use cosmwasm_std::{
    to_json_binary, Addr, Api, BalanceResponse, BankMsg, BankQuery, Coin, CosmosMsg, Deps,
    MessageInfo, QuerierWrapper, QueryRequest, StdError, StdResult, Uint128, WasmMsg, WasmQuery,
};

use cw20::{
    BalanceResponse as Cw20BalanceResponse, Cw20ExecuteMsg, Cw20QueryMsg, MinterResponse,
    TokenInfoResponse,
};

use cw_utils::must_pay;

pub const UBLUECHIP_DENOM: &str = "stake";

#[cw_serde]
pub struct TokenInfo {
    // which token is being used (bluechip or creator token)
    pub info: TokenType,
    pub amount: Uint128,
}

impl fmt::Display for TokenInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.info)
    }
}

impl TokenInfo {
    pub fn deduct_tax(&self, _querier: &QuerierWrapper) -> StdResult<Coin> {
        let amount = self.amount;
        if let TokenType::Bluechip { denom } = &self.info {
            Ok(Coin {
                denom: denom.to_string(),
                amount,
            })
        } else {
            Err(StdError::generic_err("cannot deduct tax from token asset"))
        }
    }

    pub fn into_msg(self, querier: &QuerierWrapper, recipient: Addr) -> StdResult<CosmosMsg> {
        let amount = self.amount;

        match &self.info {
            TokenType::CreatorToken { contract_addr } => Ok(CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: contract_addr.to_string(),
                msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                    recipient: recipient.to_string(),
                    amount,
                })?,
                funds: vec![],
            })),
            TokenType::Bluechip { .. } => Ok(CosmosMsg::Bank(BankMsg::Send {
                to_address: recipient.to_string(),
                amount: vec![self.deduct_tax(querier)?],
            })),
        }
    }

    pub fn confirm_sent_bluechip_token_balance(&self, message_info: &MessageInfo) -> StdResult<()> {
        if let TokenType::Bluechip { denom } = &self.info {
            let amount = must_pay(message_info, denom)
                .map_err(|err| StdError::generic_err(err.to_string()))?;
            if self.amount == amount {
                Ok(())
            } else {
                Err(StdError::generic_err(format!(
                    "amount mismatch for denom '{}': expected {}, but received {}",
                    denom, self.amount, amount
                )))
            }
        } else {
            Err(StdError::generic_err(
                "SimpleSwap can only be used with bluechip tokens. Use CW20 Send for token swaps.",
            ))
        }
    }
}

pub trait TokenSending {
    fn assert_coins_properly_sent(
        &self,
        assets: &[TokenInfo],
        pool_asset_infos: &[TokenType],
    ) -> StdResult<()>;
}

impl TokenSending for Vec<Coin> {
    fn assert_coins_properly_sent(
        &self,
        input_assets: &[TokenInfo],
        pool_asset_infos: &[TokenType],
    ) -> StdResult<()> {
        let pool_coins = pool_asset_infos
            .iter()
            .filter_map(|asset_info| match asset_info {
                TokenType::Bluechip { denom } => Some(denom.to_string()),
                _ => None,
            })
            .collect::<HashSet<_>>();

        let input_coins = input_assets
            .iter()
            .filter_map(|asset| match &asset.info {
                TokenType::Bluechip { denom } => Some((denom.to_string(), asset.amount)),
                _ => None,
            })
            .map(|pair| {
                if pool_coins.contains(&pair.0) {
                    Ok(pair)
                } else {
                    Err(StdError::generic_err(format!(
                        "Asset {} is not in the pool",
                        pair.0
                    )))
                }
            })
            .collect::<StdResult<HashMap<_, _>>>()?;

        self.iter().try_for_each(|coin| {
            if input_coins.contains_key(&coin.denom) {
                if input_coins[&coin.denom] == coin.amount {
                    Ok(())
                } else {
                    Err(StdError::generic_err(format!(
                        "amount mismatch for denom '{}': expected {}, but received {}",
                        coin.denom, input_coins[&coin.denom], coin.amount
                    )))
                }
            } else {
                Err(StdError::generic_err(format!(
                    "Supplied coins contain {} that is not in the input asset vector",
                    coin.denom
                )))
            }
        })
    }
}

#[cw_serde]
pub enum TokenType {
    // Non-bluechip Token
    CreatorToken { contract_addr: Addr },
    // Native token
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
    pub fn is_token_an_ibc_token(&self) -> bool {
        match self {
            TokenType::Bluechip { denom } => denom.to_lowercase().starts_with("ibc/"),
            TokenType::CreatorToken { .. } => false,
        }
    }

    pub fn query_pool(&self, querier: &QuerierWrapper, pool_addr: Addr) -> StdResult<Uint128> {
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
    pub fn check(&self, api: &dyn Api) -> StdResult<()> {
        if let TokenType::CreatorToken { contract_addr } = self {
            api.addr_validate(contract_addr.as_str())?;
        }

        Ok(())
    }
}

#[cw_serde]
pub struct PoolDetails {
    // information for the two token in the pool
    pub asset_infos: [TokenType; 2],
    // Pair contract address
    pub contract_addr: Addr,
    // The pool type (xyk, stableswap etc) available in [`PairType`]
    pub pair_type: PoolPairType,
    pub assets: [TokenInfo; 2],
}

impl PoolDetails {
    pub fn query_pools(
        &self,
        querier: &QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[TokenInfo; 2]> {
        Ok([
            TokenInfo {
                amount: self.asset_infos[0].query_pool(querier, contract_addr.clone())?,
                info: self.asset_infos[0].clone(),
            },
            TokenInfo {
                amount: self.asset_infos[1].query_pool(querier, contract_addr)?,
                info: self.asset_infos[1].clone(),
            },
        ])
    }
}

// Returns a lowercased, validated address upon success if present.
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

pub fn token_asset(contract_addr: Addr, amount: Uint128) -> TokenInfo {
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

pub fn pair_info_by_pool(deps: Deps, pool: Addr) -> StdResult<PoolDetails> {
    let minter_info: MinterResponse = deps
        .querier
        .query_wasm_smart(pool, &Cw20QueryMsg::Minter {})?;

    let pair_info: PoolDetails = deps
        .querier
        .query_wasm_smart(minter_info.minter, &QueryMsg::Pair {})?;

    Ok(pair_info)
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

#[cw_serde]
pub enum PoolPairType {
    // XYK pair type
    Xyk {},
    // Stable pair type
    Stable {},
}

// Return a raw encoded string representing the name of each pool type
impl Display for PoolPairType {
    fn fmt(&self, fmt: &mut Formatter) -> Result {
        match self {
            PoolPairType::Xyk {} => fmt.write_str("xyk"),
            PoolPairType::Stable {} => fmt.write_str("stable"),
        }
    }
}

pub fn query_token_balance(
    querier: &QuerierWrapper,
    contract_addr: Addr,
    account_addr: Addr,
) -> StdResult<Uint128> {
    // load balance from the token contract
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

pub fn query_token_symbol(querier: &QuerierWrapper, contract_addr: Addr) -> StdResult<String> {
    let res: TokenInfoResponse = querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: String::from(contract_addr),
        msg: to_json_binary(&Cw20QueryMsg::TokenInfo {})?,
    }))?;

    Ok(res.symbol)
}

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

pub fn call_pool_info(deps: Deps, pool_info: PoolInfo) -> StdResult<[TokenInfo; 2]> {
    let contract_addr = pool_info.pool_info.contract_addr.clone();
    let pools: [TokenInfo; 2] = pool_info
        .pool_info
        .query_pools(&deps.querier, contract_addr)?;

    Ok(pools)
}
