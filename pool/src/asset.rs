// Re-export shared types from the interfaces crate.
// TokenType, TokenInfo, PoolPairType, query helpers, constructors, etc.
// are now defined once in pool-factory-interfaces::asset.
pub use pool_factory_interfaces::asset::*;

use std::collections::{HashMap, HashSet};

use crate::msg::QueryMsg;
use crate::state::PoolInfo;

use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, MessageInfo, QuerierWrapper, StdError,
    StdResult, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, MinterResponse};
use cw_utils::must_pay;

pub const UBLUECHIP_DENOM: &str = "ubluechip";

// Pool-specific extension methods for TokenInfo.
// These depend on cw20/bank message building which is only needed in the pool contract.
pub trait TokenInfoPoolExt {
    fn deduct_tax(&self, querier: &QuerierWrapper) -> StdResult<Coin>;
    fn into_msg(self, querier: &QuerierWrapper, recipient: Addr) -> StdResult<CosmosMsg>;
    fn confirm_sent_bluechip_token_balance(&self, message_info: &MessageInfo) -> StdResult<()>;
}

impl TokenInfoPoolExt for TokenInfo {
    fn deduct_tax(&self, _querier: &QuerierWrapper) -> StdResult<Coin> {
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

    fn into_msg(self, querier: &QuerierWrapper, recipient: Addr) -> StdResult<CosmosMsg> {
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

    fn confirm_sent_bluechip_token_balance(&self, message_info: &MessageInfo) -> StdResult<()> {
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

// Pool-specific: PoolPairInfo includes current balances alongside pair config.
// Renamed from PoolDetails to PoolPairInfo to avoid collision with state::PoolDetails.
#[cosmwasm_schema::cw_serde]
pub struct PoolPairInfo {
    pub asset_infos: [TokenType; 2],
    pub contract_addr: Addr,
    pub pair_type: PoolPairType,
    pub assets: [TokenInfo; 2],
}

impl PoolPairInfo {
    pub fn query_pools(
        &self,
        querier: &QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[TokenInfo; 2]> {
        pool_factory_interfaces::asset::query_pools(&self.asset_infos, querier, contract_addr)
    }
}

pub fn pair_info_by_pool(deps: Deps, pool: Addr) -> StdResult<PoolPairInfo> {
    let minter_info: MinterResponse = deps
        .querier
        .query_wasm_smart(pool, &cw20::Cw20QueryMsg::Minter {})?;

    let pair_info: PoolPairInfo = deps
        .querier
        .query_wasm_smart(minter_info.minter, &QueryMsg::Pair {})?;

    Ok(pair_info)
}

pub fn call_pool_info(deps: Deps, pool_info: PoolInfo) -> StdResult<[TokenInfo; 2]> {
    let contract_addr = pool_info.pool_info.contract_addr.clone();
    pool_factory_interfaces::asset::query_pools(
        &pool_info.pool_info.asset_infos,
        &deps.querier,
        contract_addr,
    )
}
