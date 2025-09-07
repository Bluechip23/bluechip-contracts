use cosmwasm_schema::cw_serde;

use crate::asset::{TokenInfo, TokenType};

use cosmwasm_std::{Addr, Binary, Decimal, QuerierWrapper, StdResult, Uint128};

#[cw_serde]
pub struct CreatePool {
    // the creator token and bluechip.The creator token will be Token and bluechip will be Native
    pub pool_token_info: [TokenType; 2],
    // CW20 contract code ID the pools use to copy into their logic.
    pub cw20_token_contract_id: u64,
    // The factory contract address being used to create the creator pool
    pub factory_to_create_pool_addr: Addr,
    //this will be fed into the factory's reply function. It is the threshold payout amounts.
    pub threshold_payout: Option<Binary>,
    //the fee amount going to the creator (5%) and bluechip (1%)
    pub commit_fee_info: CommitFeeInfo,
    // address for the newly created creator token. Autopopulated by the factory reply function
    pub creator_token_address: Addr,
    //amount of bluechip that gets seeded into creator pool
    pub commit_amount_for_threshold: Uint128,
    //the threshold limit for the contract. Once crossed, the pool mints and distributes new creator (CW20 token) and now behaves like a normal liquidity pool
    pub commit_limit_usd: Uint128,
    // the contract addr of the oracle being used to convert prices to and from dollars
    pub pyth_contract_addr_for_conversions: String,
    // the symbol the contract will be looking for for commit messages. the bluechip token's symbol
    pub pyth_atom_usd_price_feed_id: String,
}
#[cw_serde]
pub struct ThresholdPayoutAmounts {
    // once the threshold is crossed, the amount distributed directly to the creator
    pub creator_reward_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the BlueChip
    pub bluechip_reward_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the newly formed creator pool
    pub pool_seed_amount: Uint128,
    // once the threshold is crossed, the amount distributed directly to the commiters before the threshold was crossed in proportion to the amount they commited.
    pub commit_return_amount: Uint128,
}
#[cw_serde]
pub struct CommitFeeInfo {
    //addres bluechip fees from commits accumulate
    pub bluechip_wallet_address: Addr,
    //address creator fees from commits accumulate
    pub creator_wallet_address: Addr,
    // the amount bluechip earns per commit
    pub commit_fee_bluechip: Decimal,
    // the amount the creator earns per commit
    pub commit_fee_creator: Decimal,
}

#[cw_serde]
pub struct ConfigResponse {
    // Last timestamp when the cumulative prices in the pool were updated
    pub block_time_last: u64,
    // The pool's parameters
    pub params: Option<Binary>,
}

#[cw_serde]
pub struct PoolDetails {
    pub pool_id: u64,
    // information for the two tokens in the pool
    pub pool_token_info: [TokenType; 2],
    pub creator_pool_addr: Addr,
}



impl PoolDetails {
    pub fn query_pools(
        &self,
        querier: &QuerierWrapper,
        contract_addr: Addr,
    ) -> StdResult<[TokenInfo; 2]> {
        Ok([
            TokenInfo {
                amount: self.pool_token_info[0].query_pool(querier, contract_addr.clone())?,
                info: self.pool_token_info[0].clone(),
            },
            TokenInfo {
                amount: self.pool_token_info[1].query_pool(querier, contract_addr)?,
                info: self.pool_token_info[1].clone(),
            },
        ])
    }
}
