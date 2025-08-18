use cosmwasm_schema::{cw_serde, QueryResponses};

use crate::asset::{AssetInfo, PairInfo};

use cosmwasm_std::{Addr, Binary, Decimal, Uint128};

#[cw_serde]
pub struct CreatePool {
    /// the creator token and bluechip.The creator token will be Token and bluechip will be Native
    pub asset_infos: [AssetInfo; 2],
    /// CW20 contract code ID the pools use to copy into their logic.
    pub token_code_id: u64,
    /// The factory contract address being used to create the creator pool
    pub factory_addr: Addr,
    //this will be fed into the factory's reply function. It is the threshold payout amounts.
    pub init_params: Option<Binary>,
    //the fee amount going to the creator (5%) and bluechip (1%)
    pub fee_info: FeeInfo,
    // address for the newly created creator token. Autopopulated by the factory reply function
    pub token_address: Addr,
    //the threshold limit for the contract. Once crossed, the pool mints and distributes new creator (CW20 token) and now behaves like a normal liquidity pool
    pub commit_limit_usd: Uint128,
    // the contract of the oracle being used to convert prices to and from dollars
    pub oracle_addr: Addr,
    // the symbol the contract will be looking for for commit messages. the bluechip token's symbol
    pub oracle_symbol: String,
}
#[cw_serde]
pub struct ThresholdPayout {
    // once the threshold is crossed, the amount distributed directly to the creator
    pub creator_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the BlueChip
    pub bluechip_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the newly formed creator pool
    pub pool_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the commiters before the threshold was crossed in proportion to the amount they commited.
    pub commit_amount: Uint128,
}
#[cw_serde]
pub struct FeeInfo {
    //addres bluechip fees from commits accumulate
    pub bluechip_address: Addr,
    //address creator fees from commits accumulate
    pub creator_address: Addr,
    // the amount bluechip earns per commit
    pub bluechip_fee: Decimal,
    // the amount the creator earns per commit
    pub creator_fee: Decimal,
}

#[cw_serde]
pub enum ExecuteMsg {
    /// Update the pair configuration
    UpdateConfig { params: Binary },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    // factory config
    #[returns(ConfigResponse)]
    Config {},
    #[returns(PairInfo)]
    Pair {},
}

#[cw_serde]
pub struct ConfigResponse {
    /// Last timestamp when the cumulative prices in the pool were updated
    pub block_time_last: u64,
    /// The pool's parameters
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct MigrateMsg {}
