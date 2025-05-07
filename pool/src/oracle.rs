use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;


#[cw_serde]
pub enum PythQueryMsg {
    /// e.g. price_id = "SEI_USD"
    GetPrice { price_id: String },
}

#[cw_serde]
pub struct PriceResponse {
    pub price: Uint128,   
}
