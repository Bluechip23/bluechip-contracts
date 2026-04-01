#[cfg(test)]
mod tests {
    use crate::contract::{execute, instantiate};
    use crate::msg::{ExecuteMsg, ExpandEconomyMsg, InstantiateMsg};
    use crate::state::WITHDRAW_TIMELOCK_SECONDS;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, message_info, MockApi};
    use cosmwasm_std::{coins, BankMsg, CosmosMsg, Uint128};

    fn setup_contract(
        deps: &mut cosmwasm_std::OwnedDeps<
            cosmwasm_std::testing::MockStorage,
            cosmwasm_std::testing::MockApi,
            cosmwasm_std::testing::MockQuerier,
        >,
    ) {
        let factory_addr = MockApi::default().addr_make("factory");
        let owner_addr = MockApi::default().addr_make("owner");
        let creator_addr = MockApi::default().addr_make("creator");

        let msg = InstantiateMsg {
            factory_address: factory_addr.to_string(),
            owner: Some(owner_addr.to_string()),
        };
        instantiate(deps.as_mut(), mock_env(), message_info(&creator_addr, &[]), msg).unwrap();
    }

    #[test]
    fn test_propose_withdrawal_invalid_recipient_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("".to_string()), // invalid — empty address
        };

        let err = execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), msg).unwrap_err();
        assert!(
            err.to_string().contains("addr") || err.to_string().contains("empty") || err.to_string().contains("bech32"),
            "Invalid address should be rejected at propose time, got: {}",
            err
        );
    }

    #[test]
    fn test_propose_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let hacker_addr = MockApi::default().addr_make("hacker");

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        let err = execute(deps.as_mut(), mock_env(), message_info(&hacker_addr, &[]), msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn test_propose_withdrawal_duplicate_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let msg = || ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), msg()).unwrap();

        // Second proposal before the first is cancelled or executed should fail
        let err = execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), msg()).unwrap_err();
        assert!(
            err.to_string().contains("already pending"),
            "Duplicate proposal should be rejected, got: {}",
            err
        );
    }

    #[test]
    fn test_execute_withdrawal_before_timelock_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");
        let recipient_addr = MockApi::default().addr_make("valid_recipient");

        // Propose
        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some(recipient_addr.to_string()),
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        // Try to execute immediately — timelock has not elapsed
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Timelock not expired"),
            "Execute before timelock should fail, got: {}",
            err
        );
    }

    #[test]
    fn test_execute_withdrawal_after_timelock_with_recipient() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");
        let recipient_addr = MockApi::default().addr_make("valid_recipient");

        // Propose
        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some(recipient_addr.to_string()),
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        // Advance time past the 48-hour timelock
        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let res = execute(
            deps.as_mut(),
            future_env,
            message_info(&owner_addr, &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap();

        assert_eq!(res.messages.len(), 1);
        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
                assert_eq!(to_address, recipient_addr.as_str());
                assert_eq!(amount, &coins(1_000_000, "ubluechip"));
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    #[test]
    fn test_execute_withdrawal_defaults_to_sender() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(500_000),
            denom: "ubluechip".to_string(),
            recipient: None, // should default to owner
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let res = execute(
            deps.as_mut(),
            future_env,
            message_info(&owner_addr, &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap();

        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, .. }) => {
                assert_eq!(to_address, owner_addr.as_str());
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    #[test]
    fn test_execute_withdrawal_nothing_pending_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("No pending withdrawal"),
            "Should fail with no-pending error, got: {}",
            err
        );
    }

    #[test]
    fn test_execute_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");
        let hacker_addr = MockApi::default().addr_make("hacker");

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let err = execute(
            deps.as_mut(),
            future_env,
            message_info(&hacker_addr, &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn test_cancel_withdrawal_happy_path() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        // Cancel before timelock expires
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap();

        let action_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "action")
            .expect("Should have action attribute");
        assert_eq!(action_attr.value, "cancel_withdrawal");

        // After cancellation, a new proposal should be accepted
        let new_propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(500_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            new_propose_msg,
        )
        .unwrap();
    }

    #[test]
    fn test_cancel_withdrawal_nothing_pending_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("No pending withdrawal"),
            "Should fail with no-pending error, got: {}",
            err
        );
    }


    #[test]
    fn test_cancel_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");
        let hacker_addr = MockApi::default().addr_make("hacker");

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), message_info(&owner_addr, &[]), propose_msg).unwrap();

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&hacker_addr, &[]),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn test_zero_amount_expansion_skipped() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::zero(),
        });

        let res =
            execute(deps.as_mut(), mock_env(), message_info(&factory_addr, &[]), msg).unwrap();
        assert_eq!(res.messages.len(), 0);

        let action_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "action")
            .expect("Should have action attribute");
        assert_eq!(action_attr.value, "request_reward_skipped");
    }

    #[test]
    fn test_expansion_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let random_addr = MockApi::default().addr_make("random");
        let user_addr = MockApi::default().addr_make("user");

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::new(1_000_000),
        });

        let err = execute(deps.as_mut(), mock_env(), message_info(&random_addr, &[]), msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    #[test]
    fn test_propose_config_update_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let not_owner_addr = MockApi::default().addr_make("not_owner");
        let new_factory_addr = MockApi::default().addr_make("new_factory");

        let msg = ExecuteMsg::ProposeConfigUpdate {
            factory_address: Some(new_factory_addr.to_string()),
            owner: None,
        };

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&not_owner_addr, &[]),
            msg,
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }
}
