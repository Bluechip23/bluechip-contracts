#[cfg(test)]
mod expand_economy_tests {
    use crate::contract::query;
    use crate::contract::{execute, instantiate};
    use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
    use cosmwasm_std::testing::{message_info, mock_dependencies, mock_env, MockApi};
    use cosmwasm_std::{coins, from_json, BankMsg, CosmosMsg, Uint128};

    #[test]
    fn proper_initialization() {
        let mut deps = mock_dependencies();
        let factory_addr = MockApi::default().addr_make("factory");
        let owner_addr = MockApi::default().addr_make("owner");
        let creator_addr = MockApi::default().addr_make("creator");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: Some(owner_addr.to_string()),
        };
        let info = message_info(&creator_addr, &[]);

        let res = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(0, res.messages.len());

        // it worked, let's query the state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetConfig {}).unwrap();
        let value: ConfigResponse = from_json(res).unwrap();
        assert_eq!(factory_addr.as_str(), value.factory_address.as_str());
        assert_eq!(owner_addr.as_str(), value.owner.as_str());
    }

    #[test]
    fn request_expansion() {
        let mut deps = mock_dependencies();
        let factory_addr = MockApi::default().addr_make("factory");
        let creator_addr = MockApi::default().addr_make("creator");
        let user_addr = MockApi::default().addr_make("user");
        let anybody_addr = MockApi::default().addr_make("anybody");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: None,
        };
        let info = message_info(&creator_addr, &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

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
    }
}
