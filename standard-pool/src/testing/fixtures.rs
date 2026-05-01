//! Shared test fixtures for standard-pool integration tests.
//!
//! The pattern: use `mock_dependencies` + the standard-pool `instantiate`
//! entry point to build a fresh pool, then exercise pool-core handlers
//! through standard-pool's `execute` / `query` dispatch. This gives us
//! end-to-end coverage of pool-core's shared logic without writing
//! custom storage fixtures inside pool-core itself.

use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage},
    to_json_binary, Addr, ContractResult, MessageInfo, OwnedDeps, SystemResult, WasmQuery,
};
use pool_core::asset::TokenType;
use pool_factory_interfaces::cw721_msgs::{Cw721QueryMsg, OwnerOfResponse};
use pool_factory_interfaces::StandardPoolInstantiateMsg;

use crate::contract::instantiate;

pub const BLUECHIP_DENOM: &str = "ubluechip";

/// Holds the bech32-valid addresses a fixture-built pool references.
/// MockApi.addr_validate rejects raw strings, so every address that
/// passes through `instantiate` (validated via `TokenType::check`) must
/// be derived from `MockApi::addr_make`.
pub struct FixtureAddrs {
    pub factory: Addr,
    pub position_nft: Addr,
    pub creator_token: Addr,
    pub pool_owner: Addr,
    pub bluechip_wallet: Addr,
}

pub fn fixture_addrs() -> FixtureAddrs {
    let api = MockApi::default();
    FixtureAddrs {
        factory: api.addr_make("factory_contract"),
        position_nft: api.addr_make("nft_contract"),
        creator_token: api.addr_make("creator_token"),
        pool_owner: api.addr_make("pool_owner"),
        bluechip_wallet: api.addr_make("bluechip_wallet"),
    }
}

/// Returns a fresh `OwnedDeps` with the factory CW721 querier wired up so
/// `verify_position_ownership` calls on the position-NFT contract find
/// the expected owner.
pub fn mock_deps_with_nft_owner(
    owner: Addr,
    nft_contract: Addr,
) -> OwnedDeps<MockStorage, MockApi, MockQuerier> {
    let mut deps = mock_dependencies();
    let nft_contract = nft_contract.to_string();
    deps.querier.update_wasm(move |query| match query {
        WasmQuery::Smart { contract_addr, msg } => {
            if *contract_addr == nft_contract {
                if let Ok(Cw721QueryMsg::OwnerOf { .. }) = cosmwasm_std::from_json(msg) {
                    let resp = OwnerOfResponse {
                        owner: owner.to_string(),
                        approvals: vec![],
                    };
                    return SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()));
                }
            }
            SystemResult::Err(cosmwasm_std::SystemError::InvalidRequest {
                error: format!("unexpected wasm query to {}", contract_addr),
                request: msg.clone(),
            })
        }
        _ => SystemResult::Err(cosmwasm_std::SystemError::UnsupportedRequest {
            kind: "non-Smart wasm query".to_string(),
        }),
    });
    deps
}

/// Standard `StandardPoolInstantiateMsg` — one Native side (`ubluechip`),
/// one CreatorToken side. Matches the common post-threshold commit-pool
/// shape so tests that port from creator-pool's liquidity_tests continue
/// to work.
pub fn standard_instantiate_msg(addrs: &FixtureAddrs) -> StandardPoolInstantiateMsg {
    StandardPoolInstantiateMsg {
        pool_id: 1,
        pool_token_info: [
            TokenType::Native {
                denom: BLUECHIP_DENOM.to_string(),
            },
            TokenType::CreatorToken {
                contract_addr: addrs.creator_token.clone(),
            },
        ],
        used_factory_addr: addrs.factory.clone(),
        position_nft_address: addrs.position_nft.clone(),
        bluechip_wallet_address: addrs.bluechip_wallet.clone(),
    }
}

/// Runs standard-pool's `instantiate` with the default shape. Returns
/// the `OwnedDeps` for follow-up execute/query calls and the addresses
/// the fixture chose.
pub fn instantiate_default_pool() -> (
    OwnedDeps<MockStorage, MockApi, MockQuerier>,
    FixtureAddrs,
) {
    let addrs = fixture_addrs();
    let mut deps = mock_deps_with_nft_owner(addrs.pool_owner.clone(), addrs.position_nft.clone());
    let info = MessageInfo {
        sender: addrs.factory.clone(),
        funds: vec![],
    };
    instantiate(deps.as_mut(), mock_env(), info, standard_instantiate_msg(&addrs)).unwrap();
    (deps, addrs)
}
