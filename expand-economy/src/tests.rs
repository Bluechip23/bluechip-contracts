#[cfg(test)]
mod tests {
    use crate::contract::query;
    use crate::contract::{execute, instantiate};
    use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coins, from_json, Addr, BankMsg, CosmosMsg, Uint128};

    #[test]
    fn proper_initialization() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            factory_address: "factory".to_string(),
            owner: Some("owner".to_string()),
        };
        let info = mock_info("creator", &[]);

        let res = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(0, res.messages.len());

        // it worked, let's query the state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetConfig {}).unwrap();
        let value: ConfigResponse = from_json(&res).unwrap();
        assert_eq!("factory", value.factory_address);
        assert_eq!("owner", value.owner);
    }

    #[test]
    fn request_expansion() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            factory_address: "factory".to_string(),
            owner: None,
        };
        let info = mock_info("creator", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        // only factory can request expansion
        let auth_info = mock_info("factory", &[]);
        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::new(100),
        });

        let res = execute(deps.as_mut(), mock_env(), auth_info, msg).unwrap();
        assert_eq!(1, res.messages.len());
        assert_eq!(
            res.messages[0].msg,
            CosmosMsg::Bank(BankMsg::Send {
                to_address: "user".to_string(),
                amount: coins(100, "ubluechip"),
            })
        );

        // unauthorized user fails
        let unauth_info = mock_info("anybody", &[]);
        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::new(100),
        });
        let err = execute(deps.as_mut(), mock_env(), unauth_info, msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn update_config() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            factory_address: "factory".to_string(),
            owner: Some("owner".to_string()),
        };
        instantiate(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();

        // only owner can update config
        let info = mock_info("owner", &[]);
        let msg = ExecuteMsg::UpdateConfig {
            factory_address: Some("new_factory".to_string()),
            owner: Some("new_owner".to_string()),
        };

        execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetConfig {}).unwrap();
        let value: ConfigResponse = from_json(&res).unwrap();
        assert_eq!("new_factory", value.factory_address);
        assert_eq!("new_owner", value.owner);
    }
}
