use cosmwasm_schema::cw_serde;
use cosmwasm_std::Timestamp;

// Minimal expiration type matching cw721/cw-utils Expiration.
// Only used as a type annotation in TransferOwnership; variants are never
// constructed by our contracts.
#[cw_serde]
pub enum Expiration {
    AtHeight(u64),
    AtTime(Timestamp),
    Never {},
}

// Minimal CW721 instantiate message
// This matches the wire format of cw721_base::msg::InstantiateMsg
#[cw_serde]
pub struct Cw721InstantiateMsg {
    pub name: String,
    pub symbol: String,
    pub minter: String,
}

// Minimal CW721 execute message enum
#[cw_serde]
pub enum Cw721ExecuteMsg<T> {
    Mint {
        token_id: String,
        owner: String,
        token_uri: Option<String>,
        extension: T,
    },
    UpdateOwnership(Action),
}

// Ownership actions for UpdateOwnership message
#[cw_serde]
pub enum Action {
    TransferOwnership {
        new_owner: String,
        expiry: Option<Expiration>,
    },
    AcceptOwnership,
    // Permanently relinquishes ownership. Used by the factory's
    // pool-creation cleanup path when an instantiated NFT contract becomes
    // orphaned: instead of transferring to a sentinel "burn" address (which
    // requires a chain-prefix-correct bech32 string), we just renounce so
    // no one can ever call admin-gated entry points on the orphan again.
    RenounceOwnership,
}

// Minimal CW721 query message — only the variant we use
#[cw_serde]
pub enum Cw721QueryMsg {
    OwnerOf {
        token_id: String,
        include_expired: Option<bool>,
    },
}

// Minimal CW721 query response
#[cw_serde]
pub struct OwnerOfResponse {
    pub owner: String,
    pub approvals: Vec<Approval>,
}

#[cw_serde]
pub struct Approval {
    pub spender: String,
    pub expires: Expiration,
}
