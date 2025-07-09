use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, Uint128};

use cw20::{Cw20Coin, MinterResponse};
use cw20_base::msg::InstantiateMarketingInfo;

use crate::asset::AssetInfo;
use crate::pair::PairInstantiateMsg as PairInitMsg;
use crate::state::{Config};

#[cw_serde]
pub struct OfficialInstantiateMsg {
    pub config: Config,
}
#[cw_serde]
pub struct FeeInfo {
    pub bluechip_address: Addr,
    pub creator_address: Addr,
    pub bluechip_fee: Decimal,
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct CreatePoolInstantiateMsg {
    /// Information about the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    /// The token contract code ID used for the tokens in the pool
    pub token_code_id: u64,
    /// The factory contract address
    pub factory_addr: Addr,
    /// Optional binary serialised parameters for custom pool types
    pub init_params: Option<Binary>,
    pub fee_info: FeeInfo,
    pub commit_limit: Uint128,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
    pub available_payment: Vec<Uint128>,
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
pub struct MigrateMsg {}

#[cw_serde]
pub struct ConfigResponse {
    pub config: Config,
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
