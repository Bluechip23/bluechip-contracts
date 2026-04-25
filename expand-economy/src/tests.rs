#[cfg(test)]
mod expand_economy_tests {
    use crate::contract::query;
    use crate::contract::{execute, instantiate};
    use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
    use cosmwasm_std::testing::{
        message_info, mock_dependencies, mock_dependencies_with_balance, mock_env, MockApi,
    };
    use cosmwasm_std::{
        coin, coins, from_json, to_json_binary, Addr, BankMsg, Binary, ContractResult, CosmosMsg,
        SystemError, SystemResult, Uint128, WasmQuery,
    };

    use cosmwasm_schema::cw_serde;

    #[cw_serde]
    struct MockFactoryConfig {
        bluechip_denom: String,
    }

    #[cw_serde]
    struct MockFactoryResp {
        factory: MockFactoryConfig,
    }

    /// Install a wasm-mock that answers the factory's `Factory {}` query
    /// with a config carrying `expected_denom`. Required since
    /// `execute_expand_economy` cross-validates the factory's denom
    /// against this contract's stored denom on every RequestExpansion.
    fn install_factory_denom_mock(
        deps: &mut cosmwasm_std::OwnedDeps<
            cosmwasm_std::MemoryStorage,
            cosmwasm_std::testing::MockApi,
            cosmwasm_std::testing::MockQuerier,
        >,
        factory_addr: Addr,
        expected_denom: &str,
    ) {
        let factory_str = factory_addr.to_string();
        let denom_owned = expected_denom.to_string();
        deps.querier.update_wasm(move |req| match req {
            WasmQuery::Smart { contract_addr, .. } if contract_addr == &factory_str => {
                let resp = MockFactoryResp {
                    factory: MockFactoryConfig {
                        bluechip_denom: denom_owned.clone(),
                    },
                };
                SystemResult::Ok(ContractResult::Ok(to_json_binary(&resp).unwrap()))
            }
            _ => SystemResult::Err(SystemError::InvalidRequest {
                error: "unmocked wasm query".to_string(),
                request: Binary::default(),
            }),
        });
    }

    #[test]
    fn proper_initialization() {
        let mut deps = mock_dependencies();
        let factory_addr = MockApi::default().addr_make("factory");
        let owner_addr = MockApi::default().addr_make("owner");
        let creator_addr = MockApi::default().addr_make("creator");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: Some(owner_addr.to_string()),
            bluechip_denom: None,
        };
        let info = message_info(&creator_addr, &[]);

        let res = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(0, res.messages.len());

        // it worked, let's query the state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetConfig {}).unwrap();
        let value: ConfigResponse = from_json(res).unwrap();
        assert_eq!(factory_addr.as_str(), value.factory_address.as_str());
        assert_eq!(owner_addr.as_str(), value.owner.as_str());
        // Default denom is applied when the instantiate field is None.
        assert_eq!("ubluechip", value.bluechip_denom);
    }

    #[test]
    fn custom_bluechip_denom_is_honored() {
        // A non-None bluechip_denom in InstantiateMsg must be stored and
        // used by subsequent RequestExpansion calls.
        // Pre-fund the contract so the H3 graceful-no-op gate (which
        // returns an attribute-only Response when balance < amount) does
        // not short-circuit before the BankMsg is emitted.
        let mut deps = mock_dependencies_with_balance(&[coin(1_000_000, "ucustom")]);
        let factory_addr = MockApi::default().addr_make("factory");
        let creator_addr = MockApi::default().addr_make("creator");
        let user_addr = MockApi::default().addr_make("user");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: None,
            bluechip_denom: Some("ucustom".to_string()),
        };
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&creator_addr, &[]),
            msg,
        )
        .unwrap();

        // Cross-validation query (M10): expand-economy reads the factory's
        // `bluechip_denom` on every RequestExpansion and rejects if it
        // doesn't match this contract's configured denom. Mock the factory
        // response with the matching denom for this test.
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ucustom");

        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: Uint128::new(250),
            }),
        )
        .unwrap();
        assert_eq!(
            res.messages[0].msg,
            CosmosMsg::Bank(BankMsg::Send {
                to_address: user_addr.to_string(),
                amount: coins(250, "ucustom"),
            })
        );
    }

    #[test]
    fn instantiate_rejects_empty_denom() {
        let mut deps = mock_dependencies();
        let factory_addr = MockApi::default().addr_make("factory");
        let creator_addr = MockApi::default().addr_make("creator");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: None,
            bluechip_denom: Some("   ".to_string()),
        };
        let err = instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&creator_addr, &[]),
            msg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn request_expansion() {
        // Pre-fund the contract so the H3 graceful-no-op gate (skip when
        // balance < amount) doesn't short-circuit and drop the BankMsg.
        let mut deps = mock_dependencies_with_balance(&[coin(1_000_000, "ubluechip")]);
        let factory_addr = MockApi::default().addr_make("factory");
        let creator_addr = MockApi::default().addr_make("creator");
        let user_addr = MockApi::default().addr_make("user");
        let anybody_addr = MockApi::default().addr_make("anybody");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: None,
            bluechip_denom: None,
        };
        let info = message_info(&creator_addr, &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        // M10 denom cross-validation needs a factory mock that replies
        // with the matching `bluechip_denom`.
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        // only factory can request expansion
        let auth_info = message_info(&factory_addr, &[]);
        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::new(100),
        });

        let res = execute(deps.as_mut(), mock_env(), auth_info, msg).unwrap();
        assert_eq!(1, res.messages.len());
        assert_eq!(
            res.messages[0].msg,
            CosmosMsg::Bank(BankMsg::Send {
                to_address: user_addr.to_string(),
                amount: coins(100, "ubluechip"),
            })
        );

        // unauthorized user fails
        let unauth_info = message_info(&anybody_addr, &[]);
        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::new(100),
        });
        let err = execute(deps.as_mut(), mock_env(), unauth_info, msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn update_config_with_timelock() {
        let mut deps = mock_dependencies();
        let factory_addr = MockApi::default().addr_make("factory");
        let owner_addr = MockApi::default().addr_make("owner");
        let creator_addr = MockApi::default().addr_make("creator");
        let new_factory_addr = MockApi::default().addr_make("new_factory");
        let new_owner_addr = MockApi::default().addr_make("new_owner");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: Some(owner_addr.to_string()),
            bluechip_denom: None,
        };
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&creator_addr, &[]),
            msg,
        )
        .unwrap();

        // Propose config update (owner only)
        let info = message_info(&owner_addr, &[]);
        let msg = ExecuteMsg::ProposeConfigUpdate {
            factory_address: Some(new_factory_addr.to_string()),
            owner: Some(new_owner_addr.to_string()),
            bluechip_denom: Some("ucustom2".to_string()),
        };
        execute(deps.as_mut(), mock_env(), info.clone(), msg).unwrap();

        // Executing before timelock should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            info.clone(),
            ExecuteMsg::ExecuteConfigUpdate {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("Timelock not expired"));

        // Advance time past 48-hour timelock
        let mut future_env = mock_env();
        future_env.block.time = future_env
            .block
            .time
            .plus_seconds(crate::state::CONFIG_TIMELOCK_SECONDS + 1);

        execute(
            deps.as_mut(),
            future_env.clone(),
            info,
            ExecuteMsg::ExecuteConfigUpdate {},
        )
        .unwrap();

        let res = query(deps.as_ref(), future_env, QueryMsg::GetConfig {}).unwrap();
        let value: ConfigResponse = from_json(res).unwrap();
        assert_eq!(new_factory_addr.as_str(), value.factory_address.as_str());
        assert_eq!(new_owner_addr.as_str(), value.owner.as_str());
        // bluechip_denom was also updated via the same timelocked flow.
        assert_eq!("ucustom2", value.bluechip_denom);
    }
}
