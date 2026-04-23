//! Shared asset-handling helpers. `TokenInfoPoolExt` gives `TokenInfo`
//! the pool-side message-building methods (`into_msg`,
//! `confirm_sent_native_balance`, `deduct_tax`). `PoolPairInfo` is the
//! response type for shared `query_pair_info`.
//!
//! The `pool_factory_interfaces::asset::*` glob re-export keeps
//! `TokenType`, `TokenInfo`, `PoolPairType`, `get_native_denom`, and the
//! various constructors accessible as `pool_core::asset::*` — so any
//! `use pool_core::asset::X;` in downstream crates Just Works.

pub use pool_factory_interfaces::asset::*;

use crate::state::PoolInfo;

use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Coin, CosmosMsg, Deps, MessageInfo, QuerierWrapper, StdError,
    StdResult, WasmMsg,
};
use cw20::Cw20ExecuteMsg;
use cw_utils::must_pay;

pub const UBLUECHIP_DENOM: &str = "ubluechip";

// Pool-specific extension methods for TokenInfo.
// These depend on cw20/bank message building which is only needed in the pool contract.
pub trait TokenInfoPoolExt {
    fn deduct_tax(&self, querier: &QuerierWrapper) -> StdResult<Coin>;
    fn into_msg(self, querier: &QuerierWrapper, recipient: Addr) -> StdResult<CosmosMsg>;
    fn confirm_sent_native_balance(&self, message_info: &MessageInfo) -> StdResult<()>;
}

impl TokenInfoPoolExt for TokenInfo {
    fn deduct_tax(&self, _querier: &QuerierWrapper) -> StdResult<Coin> {
        let amount = self.amount;
        if let TokenType::Native { denom } = &self.info {
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
            TokenType::Native { .. } => Ok(CosmosMsg::Bank(BankMsg::Send {
                to_address: recipient.to_string(),
                amount: vec![self.deduct_tax(querier)?],
            })),
        }
    }

    fn confirm_sent_native_balance(&self, message_info: &MessageInfo) -> StdResult<()> {
        if let TokenType::Native { denom } = &self.info {
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

pub fn call_pool_info(deps: Deps, pool_info: PoolInfo) -> StdResult<[TokenInfo; 2]> {
    let contract_addr = pool_info.pool_info.contract_addr.clone();
    pool_factory_interfaces::asset::query_pools(
        &pool_info.pool_info.asset_infos,
        &deps.querier,
        contract_addr,
    )
}
