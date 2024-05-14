use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Uint128};
use cw20::{Cw20Coin, MinterResponse};
use cw20_base::msg::InstantiateMarketingInfo;

use crate::pair::InstantiateMsg as PairInitMsg;
use crate::state::{Config, SubscribeInfo};

#[cw_serde]
pub struct InstantiateMsg {
    pub config: Config,
}

#[cw_serde]
pub enum ExecuteMsg {
    UpdateConfig {
        config: Config,
    },
    Create {
        pair_msg: PairInitMsg,
        token_info: TokenInfo,
    },
}

#[cw_serde]
pub enum QueryMsg {
    Config {},
    SubscribeInfo { creator: Addr },
}

#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct ConfigResponse {
    pub config: Config,
}

#[cw_serde]
pub struct SubscribeInfoResponse {
    pub subscribe_info: SubscribeInfo,
}

#[cw_serde]
pub struct TokenInstantiateMsg {
    /// Token name
    pub name: String,
    /// Token symbol
    pub symbol: String,
    /// The amount of decimals the token has
    pub decimals: u8,
    /// Initial token balances
    pub initial_balances: Vec<Cw20Coin>,
    /// Minting controls specified in a [`MinterResponse`] structure
    pub mint: Option<MinterResponse>,
    /// the marketing info of type [`InstantiateMarketingInfo`]
    pub marketing: Option<InstantiateMarketingInfo>,
}

#[cw_serde]
pub struct TokenInfo {
    pub name: String,
    pub symbol: String,
    pub decimal: u8,
}
