#![cfg(not(target_arch = "wasm32"))]
#![allow(non_snake_case)]
use crate::oracle::{PriceResponse, PythQueryMsg};
use cosmwasm_std::testing::{MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Coin, Decimal, Empty, OwnedDeps, Querier, QuerierResult,
    QuerierWrapper, QueryRequest, SystemError, SystemResult, Uint128, WasmQuery,
};
use cw20::{BalanceResponse, Cw20QueryMsg, TokenInfoResponse};
use std::collections::HashMap;

use crate::msg::{CommitFeeInfo, FeeInfoResponse, PoolResponse, QueryMsg};
use cosmwasm_schema::cw_serde;

#[cw_serde]
pub enum MockFactoryQuery {
    InternalBlueChipOracleQuery(MockOracleQuery),
}

#[cw_serde]
pub enum MockOracleQuery {
    ConvertBluechipToUsd { amount: Uint128 },
}

// mock_dependencies is a drop-in replacement for cosmwasm_std::testing::mock_dependencies.
// This uses the BETFI CustomQuerier.
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
    token_querier: TokenQuerier,
}

#[derive(Clone, Default)]
pub struct TokenQuerier {
    // This lets us iterate over all pairs that match the first string
    balances: HashMap<String, HashMap<String, Uint128>>,
}

impl TokenQuerier {
    pub fn new(balances: &[(&String, &[(&String, &Uint128)])]) -> Self {
        TokenQuerier {
            balances: balances_to_map(balances),
        }
    }
}

pub(crate) fn balances_to_map(
    balances: &[(&String, &[(&String, &Uint128)])],
) -> HashMap<String, HashMap<String, Uint128>> {
    let mut balances_map: HashMap<String, HashMap<String, Uint128>> = HashMap::new();
    for (contract_addr, balances) in balances.iter() {
        let mut contract_balances_map: HashMap<String, Uint128> = HashMap::new();
        for (addr, balance) in balances.iter() {
            contract_balances_map.insert(addr.to_string(), **balance);
        }

        balances_map.insert(contract_addr.to_string(), contract_balances_map);
    }
    balances_map
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
    pub fn new(base: MockQuerier<Empty>) -> Self {
        WasmMockQuerier {
            base,
            token_querier: TokenQuerier::default(),
        }
    }

    // Seed CW20 balances for `contract_addr`
    pub fn with_token_balances(&mut self, balances: &[(&String, &[(&String, &Uint128)])]) {
        self.token_querier = TokenQuerier::new(balances);
    }

    // Seed bluechip bank balances
    pub fn with_balance(&mut self, balances: &[(&String, &[Coin])]) {
        for (addr, coins) in balances {
            self.base.update_balance(addr.to_string(), coins.to_vec());
        }
    }

    fn handle_query(&self, request: &QueryRequest<Empty>) -> QuerierResult {
        match request {
            QueryRequest::Wasm(WasmQuery::Smart { contract_addr, msg }) => {
                // 1) factory fee-info
                if contract_addr == "factory" {
                    if let Ok(QueryMsg::FeeInfo {}) = from_json(&msg) {
                        let fee_info = CommitFeeInfo {
                            bluechip_wallet_address: Addr::unchecked("ubluechip"),
                            creator_wallet_address: Addr::unchecked("creator"),
                            commit_fee_bluechip: Decimal::percent(10),
                            commit_fee_creator: Decimal::percent(10),
                        };
                        let resp = FeeInfoResponse { fee_info };
                        let bin = to_json_binary(&resp).unwrap();
                        return SystemResult::Ok(cosmwasm_std::ContractResult::Ok(bin));
                    }
                    // Handle InternalBlueChipOracleQuery
                    if let Ok(MockFactoryQuery::InternalBlueChipOracleQuery(
                        MockOracleQuery::ConvertBluechipToUsd { amount },
                    )) = from_json(&msg)
                    {
                        // Mock 1:1 price for simplicity in tests
                        let bin = to_json_binary(&amount).unwrap();
                        return SystemResult::Ok(cosmwasm_std::ContractResult::Ok(bin));
                    }
                    panic!(
                        "Unexpected query to factory: {}",
                        String::from_utf8_lossy(msg)
                    );
                }

                // 2) pool reserves
                if let Ok(QueryMsg::PoolInfo {}) = from_json(&msg) {
                    // bluechip balance from bank
                    let bluechip = QuerierWrapper::<Empty>::new(&self.base)
                        .query_balance(contract_addr.clone(), "ubluechip".to_string())
                        .unwrap();
                    // cw20 balance via smart query
                    let wrapper = QuerierWrapper::<Empty>::new(&self.base);
                    let raw: BalanceResponse = wrapper
                        .query_wasm_smart(
                            contract_addr.clone(),
                            &Cw20QueryMsg::Balance {
                                address: contract_addr.clone(),
                            },
                        )
                        .unwrap();
                    let cw20_amount = raw.balance;
                    let resp = PoolResponse {
                        assets: [
                            crate::asset::bluechip_asset("ubluechip".to_string(), bluechip.amount),
                            crate::asset::token_asset(
                                Addr::unchecked(contract_addr.clone()),
                                cw20_amount,
                            ),
                        ],
                    };
                    let bin = to_json_binary(&resp).unwrap();
                    return SystemResult::Ok(cosmwasm_std::ContractResult::Ok(bin));
                } else if contract_addr == "oracle0000" {
                    match from_json(&msg).unwrap() {
                        PythQueryMsg::GetPrice { price_id: _ } => {
                            let resp = PriceResponse {
                                price: Uint128::new(100_000_000),
                                conf: Uint128::new(100_000),
                                expo: -8,
                                publish_time: 1234567890,
                            };
                            return SystemResult::Ok(cosmwasm_std::ContractResult::Ok(
                                to_json_binary(&resp).unwrap(),
                            ));
                        }
                    }
                }
                // 3) CW20 canonical queries
                match from_json(&msg).unwrap() {
                    Cw20QueryMsg::TokenInfo {} => {
                        let supply = self
                            .token_querier
                            .balances
                            .get(contract_addr)
                            .map(|m| m.values().copied().sum())
                            .unwrap_or_default();
                        let info = TokenInfoResponse {
                            name: "TOKEN".to_string(),
                            decimals: 6,
                            total_supply: supply,
                            symbol: "TKN".to_string(),
                        };
                        let bin = to_json_binary(&info).unwrap();
                        SystemResult::Ok(cosmwasm_std::ContractResult::Ok(bin))
                    }
                    Cw20QueryMsg::Balance { address } => {
                        let bal = self
                            .token_querier
                            .balances
                            .get(contract_addr)
                            .and_then(|m| m.get(&address))
                            .copied()
                            .unwrap_or_default();
                        let resp = BalanceResponse { balance: bal };
                        let bin = to_json_binary(&resp).unwrap();
                        SystemResult::Ok(cosmwasm_std::ContractResult::Ok(bin))
                    }
                    _ => panic!("Unexpected CW20 query: {:?}", msg),
                }
            }
            QueryRequest::Wasm(WasmQuery::Raw { contract_addr, .. }) => {
                if contract_addr == "factory" {
                    SystemResult::Ok(to_json_binary(&Vec::<Addr>::new()).into())
                } else {
                    panic!("DO NOT ENTER HERE");
                }
            }
            _ => self.base.handle_query(request),
        }
    }
}
