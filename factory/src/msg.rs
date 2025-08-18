use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Binary, Decimal, Uint128};

use cw20::{Cw20Coin, MinterResponse};
use cw20_base::msg::InstantiateMarketingInfo;

use crate::asset::AssetInfo;
use crate::pair::CreatePool;
use crate::state::{FactoryInstantiate};

#[cw_serde]
pub struct FeeInfo {
    pub bluechip_address: Addr,
    pub creator_address: Addr,
    pub bluechip_fee: Decimal,
    pub creator_fee: Decimal,
}

#[cw_serde]
pub struct CreatePoolReplyMsg {
    pub pool_id: u64,
    /// Information about the two assets in the pool
    pub asset_infos: [AssetInfo; 2],
    /// The token contract code ID used for the tokens in the pool
    pub token_code_id: u64,
    /// The factory contract address
    pub factory_addr: Addr,
    /// Optional binary serialised parameters for custom pool types
    pub init_params: Option<Binary>,
    pub fee_info: FeeInfo,
    pub commit_limit_usd: Uint128,
    pub oracle_addr: Addr,
    pub oracle_symbol: String,
    pub token_address: Addr,
    //address called by the pool to mint new liquidity position NFTs.
    pub position_nft_address: Addr,
}

#[cw_serde]
pub enum ExecuteMsg {
    UpdateConfig {
        config: FactoryInstantiate,
    },
    Create {
        create_pool_msg: CreatePool,
        token_info: TokenInfo,
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
