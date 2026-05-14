#![cfg(not(target_arch = "wasm32"))]

use cosmwasm_std::testing::{MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Coin, Empty, OwnedDeps, Querier, QuerierResult, QueryRequest,
    SystemError, SystemResult, WasmQuery,
};
use pool_factory_interfaces::{IsPausedResponse, PoolQueryMsg, PoolStateResponseForFactory};

use crate::pyth_types::{PriceResponse, PythQueryMsg};
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
    // When set, responds to mock-oracle GetPrice { price_id: "BLUECHIP_USD" }
    // with this price. Used by the #[cfg(feature = "mock")] oracle-update tests.
    pub mock_bluechip_usd_price: Option<cosmwasm_std::Uint128>,
    // Per-pool overrides for `PoolQueryMsg::GetPoolState`. Keyed by
    // contract address; when present, the override is returned verbatim.
    // Falls back to the default 50B/10B reserves below if no override is
    // registered for the queried address. Used by integration tests that
    // need different reserves on different pools (dedup, M-4
    // liquidity floor) — without this, every cross-contract pool query
    // returns the same numbers regardless of which pool was asked.
    pub pool_state_overrides:
        std::collections::HashMap<String, PoolStateResponseForFactory>,
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
                        PoolQueryMsg::GetPoolState {} => {
                            // Per-pool override takes precedence — tests
                            // that need distinct reserves per pool register
                            // them via `pool_state_overrides`. Fall back to
                            // the default 50B/10B numbers (which most
                            // existing tests rely on) when no override is
                            // registered for this address.
                            let pool_state = if let Some(override_state) =
                                self.pool_state_overrides.get(contract_addr.as_str())
                            {
                                override_state.clone()
                            } else {
                                PoolStateResponseForFactory {
                                    pool_contract_address: Addr::unchecked(contract_addr.clone()),
                                    nft_ownership_accepted: true,
                                    reserve0: cosmwasm_std::Uint128::new(50_000_000_000),
                                    reserve1: cosmwasm_std::Uint128::new(10_000_000_000),
                                    total_liquidity: cosmwasm_std::Uint128::new(10_000_000),
                                    block_time_last: 0,
                                    price0_cumulative_last: cosmwasm_std::Uint128::zero(),
                                    price1_cumulative_last: cosmwasm_std::Uint128::zero(),
                                    assets: vec![],
                                }
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

                // Mock-oracle GetPrice: tests set `mock_bluechip_usd_price`
                // to configure what BLUECHIP_USD returns.
                if let Ok(pyth_msg) = from_json::<PythQueryMsg>(&msg) {
                    if let PythQueryMsg::GetPrice { price_id } = pyth_msg {
                        if price_id == "BLUECHIP_USD" {
                            if let Some(price) = self.mock_bluechip_usd_price {
                                let resp = PriceResponse {
                                    price,
                                    publish_time: 0,
                                    expo: -6,
                                    conf: cosmwasm_std::Uint128::zero(),
                                };
                                return SystemResult::Ok(to_json_binary(&resp).into());
                            }
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
            mock_bluechip_usd_price: None,
            pool_state_overrides: std::collections::HashMap::new(),
        }
    }

    /// Register an explicit `PoolStateResponseForFactory` for a given
    /// contract address. Subsequent `GetPoolState` queries against that
    /// address will return the override verbatim, bypassing the default
    /// 50B/10B response. Used by integration tests that need to model
    /// drained / lopsided / healthy pools side-by-side.
    pub fn set_pool_state(&mut self, addr: &str, state: PoolStateResponseForFactory) {
        self.pool_state_overrides
            .insert(addr.to_string(), state);
    }
}
