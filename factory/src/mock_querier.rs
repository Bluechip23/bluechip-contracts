#![cfg(not(target_arch = "wasm32"))]

use cosmwasm_std::testing::{MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Coin, Empty, OwnedDeps, Querier, QuerierResult, QueryRequest,
    SystemError, SystemResult, WasmQuery,
};
use pool_factory_interfaces::{IsPausedResponse, PoolQueryMsg, PoolStateResponseForFactory};

use crate::query::QueryMsg;

pub fn mock_dependencies(
    contract_balance: &[Coin],
) -> OwnedDeps<MockStorage, MockApi, WasmMockQuerier> {
    let custom_querier: WasmMockQuerier =
        WasmMockQuerier::new(MockQuerier::new(&[(MOCK_CONTRACT_ADDR, contract_balance)]));

    OwnedDeps {
        storage: MockStorage::default(),
        api: MockApi::default(),
        querier: custom_querier,
        custom_query_type: Default::default(),
    }
}

pub struct WasmMockQuerier {
    base: MockQuerier<Empty>,
    pub paused_pools: std::collections::HashSet<String>,
    // Pool addresses whose queries should hard-error. Used to exercise the
    // factory's graceful-fallback behavior when a pool contract is broken
    // or has been migrated out from under the factory.
    pub query_error_pools: std::collections::HashSet<String>,
}

impl Querier for WasmMockQuerier {
    fn raw_query(&self, bin_request: &[u8]) -> QuerierResult {
        let request: QueryRequest<Empty> = match from_json(bin_request) {
            Ok(v) => v,
            Err(e) => {
                return SystemResult::Err(SystemError::InvalidRequest {
                    error: format!("Parsing query request: {}", e),
                    request: bin_request.into(),
                })
            }
        };
        self.handle_query(&request)
    }
}

impl WasmMockQuerier {
    pub fn handle_query(&self, request: &QueryRequest<Empty>) -> QuerierResult {
        match &request {
            QueryRequest::Wasm(WasmQuery::Smart { contract_addr, msg }) => {
                // Hard failure path — lets tests verify fallback behavior.
                if self.query_error_pools.contains(contract_addr.as_str()) {
                    return SystemResult::Err(SystemError::NoSuchContract {
                        addr: contract_addr.clone(),
                    });
                }
                // Try parsing as PoolQueryMsg first (for pool contract queries)
                if let Ok(pool_msg) = from_json::<PoolQueryMsg>(&msg) {
                    match pool_msg {
                        PoolQueryMsg::GetPoolState {
                            pool_contract_address,
                        } => {
                            let pool_state = PoolStateResponseForFactory {
                                pool_contract_address: Addr::unchecked(pool_contract_address),
                                nft_ownership_accepted: true,
                                reserve0: cosmwasm_std::Uint128::new(50_000_000_000),
                                reserve1: cosmwasm_std::Uint128::new(10_000_000_000),
                                total_liquidity: cosmwasm_std::Uint128::new(10_000_000),
                                block_time_last: 0,
                                price0_cumulative_last: cosmwasm_std::Uint128::zero(),
                                price1_cumulative_last: cosmwasm_std::Uint128::zero(),
                                assets: vec![],
                            };
                            return SystemResult::Ok(to_json_binary(&pool_state).into());
                        }
                        PoolQueryMsg::IsPaused {} => {
                            // Tests can mark specific pools as paused by
                            // inserting their address into `paused_pools`.
                            let paused = self.paused_pools.contains(contract_addr.as_str());
                            return SystemResult::Ok(
                                to_json_binary(&IsPausedResponse { paused }).into(),
                            );
                        }
                        _ => {
                            return SystemResult::Err(SystemError::InvalidRequest {
                                error: "Unsupported pool query".to_string(),
                                request: msg.clone(),
                            })
                        }
                    }
                }

                if let Ok(_factory_msg) = from_json::<QueryMsg>(&msg) {
                    panic!("Unsupported factory query");
                }

                // If neither parse succeeded
                SystemResult::Err(SystemError::InvalidRequest {
                    error: "Could not parse query message".to_string(),
                    request: msg.clone(),
                })
            }
            _ => self.base.handle_query(request),
        }
    }
}

impl WasmMockQuerier {
    pub fn new(base: MockQuerier<Empty>) -> Self {
        WasmMockQuerier {
            base,
            paused_pools: std::collections::HashSet::new(),
            query_error_pools: std::collections::HashSet::new(),
        }
    }
}
