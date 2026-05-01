use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Uint128};
pub use pool_factory_interfaces::ExpandEconomyMsg;

#[cw_serde]
pub struct InstantiateMsg {
    pub factory_address: String,
    pub owner: Option<String>,
    /// Native bank denom to mint in `RequestExpansion`. When `None`, falls
    /// back to `DEFAULT_BLUECHIP_DENOM` ("ubluechip") — matching prior
    /// hardcoded behavior for existing deployments.
    #[serde(default)]
    pub bluechip_denom: Option<String>,
}

#[cw_serde]
pub enum ExecuteMsg {
    ExpandEconomy(ExpandEconomyMsg),

    // 48-hour timelocked config changes.
    ProposeConfigUpdate {
        factory_address: Option<String>,
        owner: Option<String>,
        /// Optional new value for `Config.bluechip_denom`. Unset means
        /// "leave the denom alone".
        #[serde(default)]
        bluechip_denom: Option<String>,
    },
    ExecuteConfigUpdate {},
    CancelConfigUpdate {},

    // 48hr timelock
    ProposeWithdrawal {
        amount: Uint128,
        denom: String,
        recipient: Option<String>,
    },

    ExecuteWithdrawal {},

    CancelWithdrawal {},
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(ConfigResponse)]
    GetConfig {},

    #[returns(cosmwasm_std::Coin)]
    GetBalance { denom: String },
}

#[cw_serde]
pub struct ConfigResponse {
    pub factory_address: Addr,
    pub owner: Addr,
    pub bluechip_denom: String,
}

/// C-EE-1 — migrate handler shape. Today there's only `UpdateVersion {}`,
/// which bumps the cw2 stored version. The downgrade guard inside the
/// handler rejects any migrate where the stored version is newer than
/// `CONTRACT_VERSION` regardless of which variant is sent — same M-3
/// invariant the pool / factory contracts enforce.
///
/// Future variants (e.g. `MigrateConfig { … }`) can land here without a
/// wire-format break: the enum is closed but new variants are additive.
#[cw_serde]
pub enum MigrateMsg {
    UpdateVersion {},
}
