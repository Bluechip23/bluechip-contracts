//! Wire-format types + helper for cross-validating the factory's
//! `bluechip_denom` against this contract's stored config. Lifted out of
//! `contract.rs` so the wire-format-sensitive plain-serde derives and
//! their justification live next to the `cross_validate_factory_denom`
//! function that uses them.

use cosmwasm_std::Deps;
use serde::{Deserialize, Serialize};

use crate::error::ContractError;
use crate::state::Config;

/// Minimal subset of the factory's query interface that this contract
/// uses to cross-validate `bluechip_denom`. Defined locally to avoid a
/// compile-time dependency on the `factory` crate (the two communicate
/// only over wasm message boundaries).
///
/// Deliberately uses plain `#[derive(serde::Serialize)]` +
/// explicit `rename_all = "snake_case"` rather than `#[cw_serde]`. The
/// query message we send to the factory must match
/// `factory::query::QueryMsg::Factory {}` on the wire — `cw_serde`
/// adds `rename_all = "snake_case"` for enums via macro magic, which
/// works today but couples our wire format to a tooling
/// implementation detail. Using plain serde derives makes the wire
/// contract explicit and resilient to future cosmwasm-schema changes.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FactoryQuery {
    Factory {},
}

/// Wire-compatible subset of `factory::msg::FactoryInstantiateResponse`
/// — only the field this contract reads.
///
/// Uses plain `#[derive(serde::Deserialize)]` rather than `#[cw_serde]`.
/// `cw_serde` is documented in cosmwasm-schema 2.x today as NOT setting
/// `deny_unknown_fields`, but that's a tooling default that could
/// reasonably flip in a future release — and if it ever does, every
/// `RequestExpansion` would fail with an opaque "unknown field"
/// deserialization error inside `query_wasm_smart`, silently bricking
/// every threshold-crossing bluechip mint.
///
/// Plain `#[derive(serde::Deserialize)]` does NOT add
/// `deny_unknown_fields`, so the "extra factory-side fields are
/// ignored" property is locked in by serde's documented default
/// behavior rather than a transitive cosmwasm-schema choice. Combined
/// with the round-trip test in `tests::factory_response_round_trip`,
/// this future-proofs the cross-validation path.
#[derive(Deserialize)]
pub(crate) struct FactoryConfigSubset {
    pub bluechip_denom: String,
}

#[derive(Deserialize)]
pub(crate) struct FactoryInstantiateResponseSubset {
    pub factory: FactoryConfigSubset,
}

/// Cross-validate the factory's `bluechip_denom` against this
/// contract's configured denom. Both fields are independently
/// admin-mutable (each contract has its own propose/apply config flow
/// with separate 48h timelocks), so they can drift if a single-side
/// update is forgotten. Drift would silently fund rewards in the wrong
/// denom — better to refuse the call and surface the mismatch loudly.
///
/// One additional cross-contract query per `RequestExpansion`. Cost is
/// negligible: the call fires only on threshold-crossing events, not
/// on hot paths.
pub(crate) fn cross_validate_factory_denom(
    deps: Deps,
    config: &Config,
) -> Result<(), ContractError> {
    let factory_resp: FactoryInstantiateResponseSubset = deps
        .querier
        .query_wasm_smart(&config.factory_address, &FactoryQuery::Factory {})
        .map_err(|e| ContractError::FactoryQueryFailed {
            reason: e.to_string(),
        })?;
    if factory_resp.factory.bluechip_denom != config.bluechip_denom {
        return Err(ContractError::BluechipDenomMismatch {
            factory: factory_resp.factory.bluechip_denom,
            expand_economy: config.bluechip_denom.clone(),
        });
    }
    Ok(())
}
