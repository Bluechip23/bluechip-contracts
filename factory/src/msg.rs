use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Uint128};

use cw20::{Cw20Coin, MinterResponse};
use cw20_base::msg::InstantiateMarketingInfo;

use crate::asset::AssetInfo;
use crate::pair::{CreatePool, FeeInfo};
use crate::state::{FactoryInstantiate};

#[cw_serde]
pub enum ExecuteMsg {
    UpdateConfig {
        config: FactoryInstantiate,
    },

}

#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct ConfigResponse {
    pub config: FactoryInstantiate,
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
    //token minting information
    pub mint: Option<MinterResponse>,

    pub marketing: Option<InstantiateMarketingInfo>,
}

#[cw_serde]
pub struct TokenInfo {
    //name of creator token - set by creator
    pub name: String,
    //symbol of creator token - set by
    pub symbol: String,
    // number of decimals 100000000 =$1 for 8 decimals
    pub decimal: u8,
}
