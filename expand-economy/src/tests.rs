#[cfg(test)]
mod tests {
    use crate::contract::query;
    use crate::contract::{execute, instantiate};
    use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, QueryMsg};
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coins, from_json, BankMsg, CosmosMsg, Uint128};

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
                amount: coins(100, "stake"),
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

    #[test]
    fn test_multiple_pools_minting_behavior() {
        let mut deps = mock_dependencies();
        let msg = InstantiateMsg {
            factory_address: "factory".to_string(),
            owner: None,
        };
        let info = mock_info("creator", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let factory_info = mock_info("factory", &[]);

        let mut previous_amount = Uint128::MAX;

        // Simulate 10 pools being created sequentially
        // The formula shows that as pools (x) increases, the minted amount should decrease.
        for i in 1..=10 {
            // We use the same formula as the factory to verify the expected behavior
            // from the economy's perspective.
            let amount = pool_factory_interfaces::calculate_mint_amount(0, i).unwrap();

            assert!(
                amount < previous_amount,
                "Pool {} amount {} should be less than previous amount",
                i,
                amount
            );
            previous_amount = amount;

            let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: format!("user{}", i),
                amount,
            });

            let res = execute(deps.as_mut(), mock_env(), factory_info.clone(), msg).unwrap();
            assert_eq!(1, res.messages.len());

            if let CosmosMsg::Bank(BankMsg::Send {
                to_address,
                amount: coins_amount,
            }) = &res.messages[0].msg
            {
                assert_eq!(to_address, &format!("user{}", i));
                assert_eq!(coins_amount[0].amount, amount);
                assert_eq!(coins_amount[0].denom, "stake");
            } else {
                panic!("Expected bank send message");
            }
        }

        // Also verify that it still works after some time has passed
        // According to the formula, if time (s) increases, the amount actually increases slightly
        // towards the base amount, but for a fixed time, increasing pools (x) always decreases the amount.
        let amount_p10_t0 = pool_factory_interfaces::calculate_mint_amount(0, 10).unwrap();
        let amount_p11_t0 = pool_factory_interfaces::calculate_mint_amount(0, 11).unwrap();
        assert!(amount_p11_t0 < amount_p10_t0);

        let amount_p10_t600 = pool_factory_interfaces::calculate_mint_amount(600, 10).unwrap();
        let amount_p11_t600 = pool_factory_interfaces::calculate_mint_amount(600, 11).unwrap();
        assert!(amount_p11_t600 < amount_p10_t600);
    }
}
