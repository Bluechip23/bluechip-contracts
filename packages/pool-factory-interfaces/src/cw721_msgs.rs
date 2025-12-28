use cosmwasm_schema::cw_serde;
use cw721::Expiration;

/// Minimal CW721 instantiate message
/// This matches the wire format of cw721_base::msg::InstantiateMsg
#[cw_serde]
pub struct Cw721InstantiateMsg {
    /// Name of the NFT contract
    pub name: String,
    /// Symbol of the NFT contract
    pub symbol: String,
    /// The minter is the only one who can create new NFTs
    pub minter: String,
}

/// Minimal CW721 execute message enum
/// Only includes the variants we actually use in our contracts
#[cw_serde]
pub enum Cw721ExecuteMsg<T> {
    /// Mint a new NFT, can only be called by the contract minter
    Mint {
        /// Unique ID of the NFT
        token_id: String,
        /// The owner of the newly minted NFT
        owner: String,
        /// Universal resource identifier for this NFT
        token_uri: Option<String>,
        /// Any custom extension used by this contract
        extension: T,
    },
    /// Update the contract's ownership
    UpdateOwnership(Action),
}

/// Ownership actions for UpdateOwnership message
/// This matches cw_ownable::Action
#[cw_serde]
pub enum Action {
    /// Propose to transfer the contract's ownership to another account
    TransferOwnership {
        new_owner: String,
        expiry: Option<Expiration>,
    },
    /// Accept the pending ownership transfer
    AcceptOwnership,
}
