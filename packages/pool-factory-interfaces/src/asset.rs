use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    to_json_binary, Addr, Api, BalanceResponse, BankQuery, QuerierWrapper, QueryRequest, StdError,
    StdResult, Uint128, WasmQuery,
};
use cw20::{BalanceResponse as Cw20BalanceResponse, Cw20QueryMsg};
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

    /// Strict variant of `query_pool`: propagates the underlying CW20
    /// query error instead of swallowing it as a zero balance.
    ///
    /// Used by callers where a silent zero on a failed CW20 balance
    /// query would corrupt downstream accounting — e.g. the router's
    /// slippage assertion (`router::execution`), where pre/post balance
    /// reads of the recipient's CW20 holdings need to fail-closed: a
    /// swallowed pre-balance error would let the user's pre-existing
    /// CW20 holdings count toward the post-route "received" total and
    /// silently weaken slippage protection by up to that amount.
    /// Native bank queries already propagate via the `?` in
    /// `query_balance`, so the only behavioural difference is on the
    /// CW20 side.
    pub fn query_pool_strict(
        &self,
        querier: &QuerierWrapper,
        pool_addr: Addr,
    ) -> StdResult<Uint128> {
        match self {
            TokenType::CreatorToken { contract_addr, .. } => {
                query_token_balance_strict(querier, contract_addr, &pool_addr)
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

    /// Validate the per-side shape of a `pool_token_info` entry.
    ///
    /// - `Native { denom }`: rejects empty / whitespace-only denoms. The
    ///   bank module on-chain would reject the same shape later, but
    ///   doing the check here surfaces operator typos at the contract
    ///   boundary rather than 48h later when an apply lands a malformed
    ///   denom and every subsequent BankMsg reverts inside the bank
    ///   module with an error nobody is watching for. (Cosmos-SDK's
    ///   stricter `^[a-zA-Z][a-zA-Z0-9/:._-]{2,127}$` regex is enforced
    ///   by the factory's `validate_pool_token_info` for commit pools;
    ///   here we only check the lowest bar so this trait method stays
    ///   meaningful for both standard- and creator-pool entry points.)
    /// - `CreatorToken { contract_addr }`: rejects malformed bech32 via
    ///   `api.addr_validate`.
    ///
    /// Centralized here so every caller (creator-pool and standard-pool
    /// `instantiate`) gets the same guard set without an asymmetric
    /// inline empty-denom check at one call site only.
    pub fn check(&self, api: &dyn Api) -> StdResult<()> {
        match self {
            TokenType::Native { denom } => {
                if denom.trim().is_empty() {
                    return Err(cosmwasm_std::StdError::generic_err(
                        "Native denom must be non-empty",
                    ));
                }
            }
            TokenType::CreatorToken { contract_addr } => {
                api.addr_validate(contract_addr.as_str())?;
            }
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

/// Strict variant of `query_token_balance`: propagates the underlying
/// query error instead of swallowing it as a zero balance.
///
/// Used by the deposit balance-verification path. There, swallowing
/// a failed pre-balance query as zero would let the post-balance query's
/// full pool reserve appear as a "delta" — silently masking the very
/// fee-on-transfer / rebasing CW20 corruption the verification is meant
/// to detect.
pub fn query_token_balance_strict(
    querier: &QuerierWrapper,
    contract_addr: &Addr,
    account_addr: &Addr,
) -> StdResult<Uint128> {
    let res: Cw20BalanceResponse = querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: contract_addr.to_string(),
        msg: to_json_binary(&Cw20QueryMsg::Balance {
            address: account_addr.to_string(),
        })?,
    }))?;
    Ok(res.balance)
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
