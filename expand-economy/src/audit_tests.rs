/// Expand-economy audit and missing-coverage tests.
///
/// Coverage:
/// - H-4: Withdraw with invalid recipient address validation
/// - Unauthorized withdraw attempt
/// - Zero-amount expansion (skipped gracefully)
/// - Withdraw with explicit recipient

#[cfg(test)]
mod tests {
    use crate::contract::{execute, instantiate};
    use crate::msg::{ExecuteMsg, ExpandEconomyMsg, InstantiateMsg};
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coins, BankMsg, CosmosMsg, Uint128};

    fn setup_contract(
        deps: &mut cosmwasm_std::OwnedDeps<
            cosmwasm_std::testing::MockStorage,
            cosmwasm_std::testing::MockApi,
            cosmwasm_std::testing::MockQuerier,
        >,
    ) {
        let msg = InstantiateMsg {
            factory_address: "factory".to_string(),
            owner: Some("owner".to_string()),
        };
        let info = mock_info("creator", &[]);
        instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
    }

    // ========================================================================
    // H-4: Withdraw with invalid recipient address validation
    // ========================================================================

    #[test]
    fn test_h4_withdraw_invalid_recipient_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let info = mock_info("owner", &[]);

        // Try to withdraw to an invalid address (empty string)
        let msg = ExecuteMsg::Withdraw {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("".to_string()),
        };

        let err = execute(deps.as_mut(), mock_env(), info, msg).unwrap_err();
        assert!(
            err.to_string().contains("addr") || err.to_string().contains("empty"),
            "H-4 regression: invalid address should be rejected, got: {}",
            err
        );
    }

    #[test]
    fn test_withdraw_with_valid_recipient() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let info = mock_info("owner", &[]);

        let msg = ExecuteMsg::Withdraw {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("valid_recipient".to_string()),
        };

        let res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(res.messages.len(), 1);

        // Verify the send goes to the specified recipient
        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
                assert_eq!(to_address, "valid_recipient");
                assert_eq!(amount, &coins(1_000_000, "ubluechip"));
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    #[test]
    fn test_withdraw_defaults_to_sender() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let info = mock_info("owner", &[]);

        let msg = ExecuteMsg::Withdraw {
            amount: Uint128::new(500_000),
            denom: "ubluechip".to_string(),
            recipient: None, // Should default to sender
        };

        let res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, .. }) => {
                assert_eq!(to_address, "owner");
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    // ========================================================================
    // Unauthorized withdraw
    // ========================================================================

    #[test]
    fn test_withdraw_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        // Non-owner tries to withdraw
        let hacker_info = mock_info("hacker", &[]);

        let msg = ExecuteMsg::Withdraw {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        let err = execute(deps.as_mut(), mock_env(), hacker_info, msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // Zero-amount expansion (should be skipped gracefully)
    // ========================================================================

    #[test]
    fn test_zero_amount_expansion_skipped() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_info = mock_info("factory", &[]);

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::zero(),
        });

        let res = execute(deps.as_mut(), mock_env(), factory_info, msg).unwrap();

        // Zero amount should produce no messages (no bank send)
        assert_eq!(res.messages.len(), 0);

        // Should have a "skipped" attribute
        let action_attr = res.attributes.iter()
            .find(|a| a.key == "action")
            .expect("Should have action attribute");
        assert_eq!(action_attr.value, "request_reward_skipped");
    }

    // ========================================================================
    // Expansion unauthorized (non-factory caller)
    // ========================================================================

    #[test]
    fn test_expansion_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        // Random user (not factory) tries to trigger expansion
        let random_info = mock_info("random", &[]);

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::new(1_000_000),
        });

        let err = execute(deps.as_mut(), mock_env(), random_info, msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // Update config unauthorized
    // ========================================================================

    #[test]
    fn test_update_config_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let non_owner_info = mock_info("not_owner", &[]);

        let msg = ExecuteMsg::UpdateConfig {
            factory_address: Some("new_factory".to_string()),
            owner: None,
        };

        let err = execute(deps.as_mut(), mock_env(), non_owner_info, msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }
}
