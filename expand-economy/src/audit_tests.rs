/// Expand-economy audit and missing-coverage tests.
///
/// Coverage:
/// - ProposeWithdrawal: address validation, unauthorized, duplicate proposal
/// - ExecuteWithdrawal: timelock not expired, happy path, no pending withdrawal
/// - CancelWithdrawal: happy path, nothing to cancel
/// - Recipient defaults to sender when None
/// - Zero-amount expansion (skipped gracefully)
/// - Unauthorized expansion / update-config

#[cfg(test)]
mod tests {
    use crate::contract::{execute, instantiate};
    use crate::msg::{ExecuteMsg, ExpandEconomyMsg, InstantiateMsg};
    use crate::state::WITHDRAW_TIMELOCK_SECONDS;
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
        instantiate(deps.as_mut(), mock_env(), mock_info("creator", &[]), msg).unwrap();
    }

    // ========================================================================
    // ProposeWithdrawal — address validation (replaces H-4)
    // ========================================================================

    #[test]
    fn test_propose_withdrawal_invalid_recipient_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("".to_string()), // invalid — empty address
        };

        let err = execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), msg).unwrap_err();
        assert!(
            err.to_string().contains("addr") || err.to_string().contains("empty"),
            "Invalid address should be rejected at propose time, got: {}",
            err
        );
    }

    // ========================================================================
    // ProposeWithdrawal — unauthorized caller
    // ========================================================================

    #[test]
    fn test_propose_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        let err = execute(deps.as_mut(), mock_env(), mock_info("hacker", &[]), msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // ProposeWithdrawal — duplicate proposal rejected
    // ========================================================================

    #[test]
    fn test_propose_withdrawal_duplicate_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let msg = || ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), msg()).unwrap();

        // Second proposal before the first is cancelled or executed should fail
        let err = execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), msg()).unwrap_err();
        assert!(
            err.to_string().contains("already pending"),
            "Duplicate proposal should be rejected, got: {}",
            err
        );
    }

    // ========================================================================
    // ExecuteWithdrawal — before timelock expires
    // ========================================================================

    #[test]
    fn test_execute_withdrawal_before_timelock_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        // Propose
        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("valid_recipient".to_string()),
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        // Try to execute immediately — timelock has not elapsed
        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("owner", &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Timelock not expired"),
            "Execute before timelock should fail, got: {}",
            err
        );
    }

    // ========================================================================
    // ExecuteWithdrawal — happy path with explicit recipient
    // ========================================================================

    #[test]
    fn test_execute_withdrawal_after_timelock_with_recipient() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        // Propose
        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some("valid_recipient".to_string()),
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        // Advance time past the 48-hour timelock
        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let res = execute(
            deps.as_mut(),
            future_env,
            mock_info("owner", &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap();

        assert_eq!(res.messages.len(), 1);
        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, amount }) => {
                assert_eq!(to_address, "valid_recipient");
                assert_eq!(amount, &coins(1_000_000, "ubluechip"));
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    // ========================================================================
    // ExecuteWithdrawal — recipient defaults to sender when None
    // ========================================================================

    #[test]
    fn test_execute_withdrawal_defaults_to_sender() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(500_000),
            denom: "ubluechip".to_string(),
            recipient: None, // should default to "owner"
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let res = execute(
            deps.as_mut(),
            future_env,
            mock_info("owner", &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap();

        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send { to_address, .. }) => {
                assert_eq!(to_address, "owner");
            }
            _ => panic!("Expected BankMsg::Send"),
        }
    }

    // ========================================================================
    // ExecuteWithdrawal — nothing pending
    // ========================================================================

    #[test]
    fn test_execute_withdrawal_nothing_pending_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("owner", &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("No pending withdrawal"),
            "Should fail with no-pending error, got: {}",
            err
        );
    }

    // ========================================================================
    // ExecuteWithdrawal — unauthorized caller
    // ========================================================================

    #[test]
    fn test_execute_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        let mut future_env = mock_env();
        future_env.block.time =
            future_env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

        let err = execute(
            deps.as_mut(),
            future_env,
            mock_info("hacker", &[]),
            ExecuteMsg::ExecuteWithdrawal {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // CancelWithdrawal — happy path
    // ========================================================================

    #[test]
    fn test_cancel_withdrawal_happy_path() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        // Cancel before timelock expires
        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("owner", &[]),
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
            mock_info("owner", &[]),
            new_propose_msg,
        )
        .unwrap();
    }

    // ========================================================================
    // CancelWithdrawal — nothing to cancel
    // ========================================================================

    #[test]
    fn test_cancel_withdrawal_nothing_pending_fails() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("owner", &[]),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("No pending withdrawal"),
            "Should fail with no-pending error, got: {}",
            err
        );
    }

    // ========================================================================
    // CancelWithdrawal — unauthorized caller
    // ========================================================================

    #[test]
    fn test_cancel_withdrawal_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        execute(deps.as_mut(), mock_env(), mock_info("owner", &[]), propose_msg).unwrap();

        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("hacker", &[]),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // Zero-amount expansion (skipped gracefully)
    // ========================================================================

    #[test]
    fn test_zero_amount_expansion_skipped() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::zero(),
        });

        let res =
            execute(deps.as_mut(), mock_env(), mock_info("factory", &[]), msg).unwrap();
        assert_eq!(res.messages.len(), 0);

        let action_attr = res
            .attributes
            .iter()
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

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: "user".to_string(),
            amount: Uint128::new(1_000_000),
        });

        let err = execute(deps.as_mut(), mock_env(), mock_info("random", &[]), msg).unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }

    // ========================================================================
    // Update config unauthorized
    // ========================================================================

    #[test]
    fn test_update_config_unauthorized() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let msg = ExecuteMsg::UpdateConfig {
            factory_address: Some("new_factory".to_string()),
            owner: None,
        };

        let err = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("not_owner", &[]),
            msg,
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Unauthorized {}));
    }
}
