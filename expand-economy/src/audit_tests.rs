#[cfg(test)]
mod tests {
    use crate::contract::{execute, instantiate};
    use crate::msg::{ExecuteMsg, ExpandEconomyMsg, InstantiateMsg};
    use crate::state::WITHDRAW_TIMELOCK_SECONDS;
    use cosmwasm_schema::cw_serde;
    use cosmwasm_std::testing::{message_info, mock_dependencies, mock_env, MockApi};
    use cosmwasm_std::{
        coins, to_json_binary, Addr, BankMsg, Binary, ContractResult, CosmosMsg, SystemError,
        SystemResult, Uint128, WasmQuery,
    };

    #[cw_serde]
    struct MockFactoryConfig {
        bluechip_denom: String,
    }
    #[cw_serde]
    struct MockFactoryResp {
        factory: MockFactoryConfig,
    }

    /// Install a wasm-mock that satisfies expand-economy's
    /// cross-validation query: `execute_expand_economy` queries the
    /// factory's `Factory {}` to confirm `bluechip_denom` matches before
    /// issuing a BankMsg::Send. Tests that don't otherwise care about the
    /// wasm querier need to install this mock or the call rejects with
    /// "Failed to query factory config".
    fn install_factory_denom_mock(
        deps: &mut cosmwasm_std::OwnedDeps<
            cosmwasm_std::testing::MockStorage,
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
            bluechip_denom: None,
        };
        instantiate(
            deps.as_mut(),
            mock_env(),
            message_info(&creator_addr, &[]),
            msg,
        )
        .unwrap();
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

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            msg,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("addr")
                || err.to_string().contains("empty")
                || err.to_string().contains("bech32"),
            "Invalid address should be rejected at propose time, got: {}",
            err
        );
    }

    /// A zero-amount withdrawal proposal is rejected at propose time —
    /// it would burn a 48h timelock cycle on a value that can never
    /// produce a payout (the apply path clamps to balance and would
    /// emit a `no_funds` note). Fail fast.
    #[test]
    fn test_propose_withdrawal_zero_amount_rejected() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let owner_addr = MockApi::default().addr_make("owner");

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::zero(),
            denom: "ubluechip".to_string(),
            recipient: None,
        };

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            msg,
        )
        .unwrap_err();
        assert!(
            matches!(err, crate::error::ContractError::WithdrawalAmountZero),
            "expected WithdrawalAmountZero, got: {:?}",
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

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&hacker_addr, &[]),
            msg,
        )
        .unwrap_err();
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

        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            msg(),
        )
        .unwrap();

        // Second proposal before the first is cancelled or executed should fail
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            msg(),
        )
        .unwrap_err();
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
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

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
        // Fund the contract so the new balance-clamp in execute_withdrawal
        // doesn't clip the payout to zero. The contract address comes from
        // mock_env() — hard-coded to `cosmos2contract` by cosmwasm's mock.
        deps.querier.bank.update_balance(
            mock_env().contract.address.to_string(),
            coins(10_000_000, "ubluechip"),
        );

        let owner_addr = MockApi::default().addr_make("owner");
        let recipient_addr = MockApi::default().addr_make("valid_recipient");

        // Propose
        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1_000_000),
            denom: "ubluechip".to_string(),
            recipient: Some(recipient_addr.to_string()),
        };
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

        // Advance time past the 48-hour timelock
        let mut future_env = mock_env();
        future_env.block.time = future_env
            .block
            .time
            .plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

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
        // Fund the contract so the balance-clamp in execute_withdrawal
        // does not clip the payout to zero.
        deps.querier.bank.update_balance(
            mock_env().contract.address.to_string(),
            coins(10_000_000, "ubluechip"),
        );

        let owner_addr = MockApi::default().addr_make("owner");

        let propose_msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(500_000),
            denom: "ubluechip".to_string(),
            recipient: None, // should default to owner
        };
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

        let mut future_env = mock_env();
        future_env.block.time = future_env
            .block
            .time
            .plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

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
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

        let mut future_env = mock_env();
        future_env.block.time = future_env
            .block
            .time
            .plus_seconds(WITHDRAW_TIMELOCK_SECONDS + 1);

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
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

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
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            propose_msg,
        )
        .unwrap();

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

        // Cross-validate factory's bluechip_denom on every call.
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::zero(),
        });

        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            msg,
        )
        .unwrap();
        assert_eq!(res.messages.len(), 0);

        let action_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "action")
            .expect("Should have action attribute");
        assert_eq!(action_attr.value, "request_reward_skipped");
        // Dormant reason explicit so monitoring can distinguish
        // "decay-curve expired" from a bug.
        let reason_attr = res
            .attributes
            .iter()
            .find(|a| a.key == "reason")
            .expect("Should have reason attribute");
        assert_eq!(reason_attr.value, "economy_dormant");
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

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&random_addr, &[]),
            msg,
        )
        .unwrap_err();
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
            bluechip_denom: None,
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

    // -- migrate handler ---------------------------------------------------

    #[test]
    fn migrate_no_op_succeeds_when_cw2_unset() {
        // Test fixtures may not have cw2 initialised. The downgrade
        // guard tolerates this (production deployments always set cw2
        // at instantiate); the migrate must still succeed and bump the
        // cw2 record to the current version.
        let mut deps = mock_dependencies();
        let res = crate::contract::migrate(
            deps.as_mut(),
            mock_env(),
            crate::msg::MigrateMsg::UpdateVersion {},
        )
        .unwrap();
        let action = res.attributes.iter().find(|a| a.key == "action").unwrap();
        assert_eq!(action.value, "migrate");
        let stored = cw2::get_contract_version(&deps.storage).unwrap();
        assert_eq!(stored.contract, "crates.io:expand-economy");
        assert_eq!(stored.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn migrate_rejects_downgrade() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);
        // Pretend the contract is already on a newer version than the
        // wasm being migrated TO. The guard must refuse.
        cw2::set_contract_version(&mut deps.storage, "crates.io:expand-economy", "999.0.0")
            .unwrap();
        let err = crate::contract::migrate(
            deps.as_mut(),
            mock_env(),
            crate::msg::MigrateMsg::UpdateVersion {},
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("downgrade"),
            "downgrade must surface a clear error, got: {}",
            err
        );
    }

    #[test]
    fn migrate_rejects_invalid_stored_semver() {
        let mut deps = mock_dependencies();
        cw2::set_contract_version(&mut deps.storage, "crates.io:expand-economy", "not-a-semver")
            .unwrap();
        let err = crate::contract::migrate(
            deps.as_mut(),
            mock_env(),
            crate::msg::MigrateMsg::UpdateVersion {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("not valid semver"));
    }

    // -- nonpayable guard --------------------------------------------------

    #[test]
    fn execute_rejects_attached_funds_on_request_expansion() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);
        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        let msg = ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
            recipient: user_addr.to_string(),
            amount: Uint128::new(100),
        });
        let funds = cosmwasm_std::coins(1u128, "ubluechip");
        let err = crate::contract::execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &funds),
            msg,
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Payment(_)));
    }

    #[test]
    fn execute_rejects_attached_funds_on_propose_withdrawal() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);
        let owner_addr = MockApi::default().addr_make("owner");

        let msg = ExecuteMsg::ProposeWithdrawal {
            amount: Uint128::new(1),
            denom: "ubluechip".to_string(),
            recipient: None,
        };
        let funds = cosmwasm_std::coins(1u128, "ubluechip");
        let err = crate::contract::execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &funds),
            msg,
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Payment(_)));
    }

    #[test]
    fn execute_rejects_attached_funds_on_cancel_paths() {
        // Cancel arms have no semantic reason to accept funds either.
        // The guard sits at dispatch top so every variant inherits it.
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);
        let owner_addr = MockApi::default().addr_make("owner");
        let funds = cosmwasm_std::coins(1u128, "ubluechip");

        let err = crate::contract::execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &funds),
            ExecuteMsg::CancelWithdrawal {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Payment(_)));

        let err = crate::contract::execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &funds),
            ExecuteMsg::CancelConfigUpdate {},
        )
        .unwrap_err();
        assert!(matches!(err, crate::error::ContractError::Payment(_)));
    }

    // -- subset deserialization round-trip --------------------------------

    /// The factory's real `FactoryInstantiateResponse` carries many more
    /// fields than just `bluechip_denom`. This test constructs a
    /// hand-built JSON blob that mimics the full factory response and
    /// asserts our subset still deserializes — locking in the
    /// "extra fields are ignored" property so a future cosmwasm-schema
    /// upgrade that flips `deny_unknown_fields` would fail this test
    /// before reaching production.
    #[test]
    fn factory_response_subset_round_trip_with_extra_fields() {
        // Mimic the real factory response with several unknown fields
        // alongside the one we read.
        let json = br#"{
            "factory": {
                "factory_admin_address": "cosmos1adminadmin",
                "bluechip_wallet_address": "cosmos1wallet",
                "atom_bluechip_anchor_pool_address": "cosmos1anchor",
                "bluechip_denom": "ubluechip",
                "atom_denom": "uatom",
                "extra_unknown_field": "should be ignored",
                "nested": { "another": "ignored" }
            }
        }"#;
        // Cross-validation path uses cosmwasm_std::from_json under the
        // hood via query_wasm_smart. Exercise the same deserializer.
        let resp: cosmwasm_std::StdResult<crate::contract::testing::FactoryInstantiateResponseSubsetForTest> =
            cosmwasm_std::from_json(&json[..]);
        let resp = resp.expect(
            "extra factory-side fields must continue to deserialize as a no-op; \
             if this test fails, a cosmwasm-schema upgrade or serde change has \
             enabled deny_unknown_fields and execute_expand_economy is now \
             silently bricked in production",
        );
        assert_eq!(resp.factory.bluechip_denom, "ubluechip");
    }

    // -- denom format validator -------------------------------------------

    #[test]
    fn validate_native_denom_accepts_canonical_shapes() {
        // Each of these must be accepted by the propose-time validator —
        // cosmos-sdk's bank module accepts all of them.
        let cases = [
            "ubluechip",
            "ucustom",
            "uatom",
            "ibc/27394FB092D2ECCD56123C74F36E4C1F926001CEADA9CA97EA622B25F41E5EB2",
            "factory/cosmos1abc/tokenname",
            "abc",
        ];
        for d in cases {
            crate::contract::testing::validate_native_denom_for_test(d).unwrap_or_else(|e| {
                panic!("expected '{}' to be accepted, got: {}", d, e)
            });
        }
    }

    #[test]
    fn validate_native_denom_rejects_typos_and_malformed() {
        // Each of these is rejected by the cosmos-sdk denom regex
        // `^[a-zA-Z][a-zA-Z0-9/:._-]{2,127}$` — and would have been
        // silently accepted by the previous "non-empty after trim"
        // check, bricking the contract 48h later when the bank
        // module rejected the denom on every BankMsg::Send.
        //
        // Note: the regex accepts uppercase letters, so "Bluechip"
        // is technically valid (a casing-mismatch with the configured
        // bank denom is a separate operator-typo class that the
        // cross-validation query catches at runtime).
        let cases = [
            ("u bluechip", "disallowed character"),               // whitespace
            ("u", "outside the cosmos-sdk allowed range"),        // too short
            ("ub", "outside the cosmos-sdk allowed range"),       // too short
            ("1ubluechip", "must start with an ASCII letter"),    // digit prefix
            ("ubluechip!", "disallowed character"),               // bad punct
            ("/ubluechip", "must start with an ASCII letter"),    // slash prefix
            ("u@bluechip", "disallowed character"),               // @ outside set
            ("ubluechip\n", "disallowed character"),              // newline
            ("   ", "must start with an ASCII letter"),           // whitespace-only
            ("", "outside the cosmos-sdk allowed range"),         // empty
        ];
        for (d, expected_fragment) in cases {
            let err = crate::contract::testing::validate_native_denom_for_test(d).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(expected_fragment),
                "denom '{}': expected fragment '{}' in error, got: {}",
                d,
                expected_fragment,
                msg
            );
        }
    }

    #[test]
    fn propose_config_update_rejects_malformed_denom() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);
        let owner_addr = MockApi::default().addr_make("owner");
        let new_factory_addr = MockApi::default().addr_make("new_factory");

        let err = crate::contract::execute(
            deps.as_mut(),
            mock_env(),
            message_info(&owner_addr, &[]),
            ExecuteMsg::ProposeConfigUpdate {
                factory_address: Some(new_factory_addr.to_string()),
                owner: None,
                bluechip_denom: Some("u bluechip".to_string()), // space — invalid
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("disallowed character"),
            "malformed denom must be caught at propose time, got: {}",
            err
        );
    }

    // -- Daily expansion cap + rolling window ----------------------------
    //
    // The daily cap is the primary defense against admin-compromise drain
    // through the `RequestExpansion` path. These tests pin the contract's
    // documented invariants:
    //   - Total expansions in a 24h rolling window cannot exceed
    //     DAILY_EXPANSION_CAP.
    //   - The window is single-bucket-reset: when more than
    //     DAILY_WINDOW_SECONDS elapses since `window_start`, the next
    //     request resets `spent_in_window` to zero (acknowledged drift
    //     vs a true sliding window — only LETS more through, never
    //     blocks legitimately).
    //   - Insufficient-balance graceful skip does NOT debit cap budget
    //     (so a refund-then-retry doesn't permanently burn quota on a
    //     payment that never landed).
    //   - Sub-cap requests accumulate correctly across multiple calls.

    use crate::state::{DAILY_EXPANSION_CAP, DAILY_WINDOW_SECONDS, EXPANSION_LOG};

    /// Sum of all amounts currently in the expansion log. Tests
    /// previously asserted `EXPANSION_WINDOW.spent_in_window` directly;
    /// after the M-3.3 sliding-window refactor the equivalent value
    /// is "total of every non-pruned entry."
    fn log_spent(
        deps: &cosmwasm_std::OwnedDeps<
            cosmwasm_std::MemoryStorage,
            cosmwasm_std::testing::MockApi,
            cosmwasm_std::testing::MockQuerier,
        >,
    ) -> Uint128 {
        EXPANSION_LOG
            .may_load(&deps.storage)
            .unwrap()
            .unwrap_or_default()
            .iter()
            .fold(Uint128::zero(), |acc, e| acc + e.amount)
    }

    /// A single request that exceeds the daily cap (fresh window) is
    /// rejected with `DailyExpansionCapExceeded`. The window state is
    /// untouched (no debit).
    #[test]
    fn daily_cap_rejects_single_request_exceeding_cap() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        // Pre-fund the contract beyond the cap so the rejection is on
        // the cap path, not the insufficient-balance fallback.
        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(DAILY_EXPANSION_CAP.u128() + 1, "ubluechip"),
            );

        let over_cap = DAILY_EXPANSION_CAP + Uint128::new(1);
        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: over_cap,
            }),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Daily expansion cap exceeded"),
            "expected DailyExpansionCapExceeded, got: {}",
            err
        );
        // Log unaffected by the rejected request — either absent or
        // empty. The cap check runs before any storage append.
        let log = EXPANSION_LOG.may_load(&deps.storage).unwrap();
        assert!(
            log.is_none() || log.unwrap().is_empty(),
            "rejected request must not debit log"
        );
    }

    /// Sub-cap requests accumulate; the call that would push total past
    /// the cap rejects, prior debits remain.
    #[test]
    fn daily_cap_accumulates_then_rejects_overshoot() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(DAILY_EXPANSION_CAP.u128() * 3, "ubluechip"),
            );

        // Three half-cap requests: the first two land, the third overshoots.
        let half_cap = DAILY_EXPANSION_CAP.checked_div(Uint128::new(2)).unwrap();

        // Call 1: spend half cap.
        execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: half_cap,
            }),
        )
        .unwrap();
        assert_eq!(log_spent(&deps), half_cap);

        // Call 2: another (just-under) half — total is still <= cap, lands.
        let just_under_half = half_cap - Uint128::new(1);
        let mut env_call2 = mock_env();
        env_call2.block.time = env_call2.block.time.plus_seconds(120);
        execute(
            deps.as_mut(),
            env_call2.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: just_under_half,
            }),
        )
        .unwrap();
        assert_eq!(log_spent(&deps), half_cap + just_under_half);

        // Call 3: another half — overshoots. Reject with prior debits
        // intact.
        let pre_call3 = log_spent(&deps);
        let mut env_call3 = mock_env();
        env_call3.block.time = env_call3.block.time.plus_seconds(240);
        let err = execute(
            deps.as_mut(),
            env_call3,
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: half_cap,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Daily expansion cap exceeded"));

        // Log unchanged by the rejection.
        assert_eq!(
            log_spent(&deps),
            pre_call3,
            "rejected request must not append to expansion log"
        );
    }

    /// After 24h+1s elapses, the next request resets the window and
    /// can spend a fresh full-cap budget.
    #[test]
    fn daily_window_rolls_over_after_24h() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(DAILY_EXPANSION_CAP.u128() * 3, "ubluechip"),
            );

        // Day-1 spend: half the cap.
        let half_cap = DAILY_EXPANSION_CAP.checked_div(Uint128::new(2)).unwrap();
        let env_day1 = mock_env();
        execute(
            deps.as_mut(),
            env_day1.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: half_cap,
            }),
        )
        .unwrap();
        assert_eq!(log_spent(&deps), half_cap);

        // Advance time past DAILY_WINDOW_SECONDS + 1s. The day-1 entry
        // is now older than the window cutoff and will be pruned on
        // the next call.
        let mut env_day2 = env_day1.clone();
        env_day2.block.time = env_day1.block.time.plus_seconds(DAILY_WINDOW_SECONDS + 1);

        // Day-2 spend: the full cap. Should succeed because the day-1
        // entry ages out of the sliding window before this call's cap
        // check runs.
        execute(
            deps.as_mut(),
            env_day2.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: DAILY_EXPANSION_CAP,
            }),
        )
        .unwrap();
        let log_after = EXPANSION_LOG.load(&deps.storage).unwrap();
        assert_eq!(
            log_after.len(),
            1,
            "day-1 entry must be pruned and only the day-2 entry remains"
        );
        assert_eq!(
            log_after[0].amount, DAILY_EXPANSION_CAP,
            "post-rollover log holds the fresh full-cap debit"
        );
        assert_eq!(
            log_after[0].timestamp, env_day2.block.time,
            "post-rollover entry is timestamped at the day-2 call"
        );
    }

    /// Insufficient-balance graceful skip MUST NOT debit cap budget.
    /// If the contract balance is below the requested amount, the
    /// handler returns Ok with the skip attribute but `spent_in_window`
    /// stays unchanged — so a later refund + retry can spend that quota.
    #[test]
    fn insufficient_balance_skip_does_not_burn_cap() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        // Contract has zero balance — request will skip on insufficient
        // balance. Pre-set window state to verify it stays untouched.
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: Uint128::new(1_000_000_000),
            }),
        )
        .unwrap();
        // Skipped, not paid. No BankMsg.
        assert_eq!(res.messages.len(), 0);
        let action = res
            .attributes
            .iter()
            .find(|a| a.key == "action")
            .unwrap();
        assert_eq!(action.value, "request_reward_skipped");
        let reason = res
            .attributes
            .iter()
            .find(|a| a.key == "reason")
            .unwrap();
        assert_eq!(reason.value, "insufficient_balance");

        // Log storage is untouched (no debit). Persist runs only on
        // the success path; the balance-skip branch returns Ok before
        // any append.
        let log = EXPANSION_LOG.may_load(&deps.storage).unwrap();
        assert!(
            log.is_none(),
            "skip path must not write any log entries at all; got {:?}",
            log
        );
    }

    /// Successful request emits a BankMsg::Send for the right amount and
    /// debits the window. Smoke test for the happy path that complements
    /// the existing zero-amount and unauthorized tests.
    #[test]
    fn successful_request_sends_bank_msg_and_debits_window() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(50_000_000, "ubluechip"),
            );

        let amount = Uint128::new(10_000_000);
        let res = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount,
            }),
        )
        .unwrap();

        // Exactly one BankMsg::Send to the recipient for the right amount.
        assert_eq!(res.messages.len(), 1);
        match &res.messages[0].msg {
            CosmosMsg::Bank(BankMsg::Send {
                to_address,
                amount: coins,
            }) => {
                assert_eq!(to_address, &user_addr.to_string());
                assert_eq!(coins.len(), 1);
                assert_eq!(coins[0].amount, amount);
                assert_eq!(coins[0].denom, "ubluechip");
            }
            other => panic!("expected BankMsg::Send, got {:?}", other),
        }

        // Log debit equals the request amount.
        assert_eq!(log_spent(&deps), amount);
        let log = EXPANSION_LOG.load(&deps.storage).unwrap();
        assert_eq!(log.len(), 1, "single success appends a single entry");
        assert_eq!(log[0].amount, amount);
    }

    /// M-3.3 boundary-burst protection. The prior single-bucket cap
    /// allowed an attacker to spend the full cap right before
    /// `window_start + 24h`, then spend the full cap again 1 second
    /// after the bucket flipped — 200k bluechip across a single
    /// rolling 24h window. The sliding window keeps the late-window
    /// debit visible to any check whose rolling 24h includes it, so
    /// the post-boundary second drain is rejected.
    #[test]
    fn sliding_window_blocks_boundary_burst() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        // Pre-fund well above the cap so the rejection path is the
        // cap check, not the balance-skip fallback.
        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(DAILY_EXPANSION_CAP.u128() * 4, "ubluechip"),
            );

        // Phase 1: a small initial debit to anchor the would-be
        // "window start" timing under the old single-bucket model.
        let env_t0 = mock_env();
        let small = Uint128::new(1_000_000);
        execute(
            deps.as_mut(),
            env_t0.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: small,
            }),
        )
        .unwrap();

        // Phase 2: just before the would-be boundary at T0 + 24h - 60s,
        // spend almost the rest of the cap. Total in-window = ~cap.
        let mut env_late = env_t0.clone();
        env_late.block.time = env_t0.block.time.plus_seconds(DAILY_WINDOW_SECONDS - 60);
        let big = DAILY_EXPANSION_CAP - small;
        execute(
            deps.as_mut(),
            env_late.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: big,
            }),
        )
        .unwrap();
        assert_eq!(
            log_spent(&deps),
            DAILY_EXPANSION_CAP,
            "phase-2 brings the in-window total to exactly the cap"
        );

        // Phase 3: cross the would-be single-bucket boundary by 1s
        // (i.e. T0 + 24h + 1s). The small phase-1 entry ages out
        // (timestamp T0 < cutoff = T0+1s), but the big phase-2 entry
        // (timestamp T0+24h-60s > cutoff) is still in the window.
        // Try to spend the full cap again. Under the OLD single-bucket
        // model this would succeed because the bucket would have reset;
        // under the sliding-window model it must reject because the
        // big phase-2 debit is still visible.
        let mut env_burst = env_t0.clone();
        env_burst.block.time = env_t0.block.time.plus_seconds(DAILY_WINDOW_SECONDS + 1);
        let err = execute(
            deps.as_mut(),
            env_burst,
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: DAILY_EXPANSION_CAP,
            }),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Daily expansion cap exceeded"),
            "boundary burst must reject under sliding window; got: {}",
            err
        );

        // Rejected calls do NOT persist — the prune that the cap check
        // ran on the in-memory log is discarded along with the Err
        // return. So storage retains the exact post-phase-2 shape:
        // both small and big entries, with the small one eligible to
        // be pruned on the NEXT successful call. The important
        // invariant here is the rejection itself (asserted above);
        // the storage state is incidental.
        let log = EXPANSION_LOG.load(&deps.storage).unwrap();
        assert_eq!(log.len(), 2, "phase-1 + phase-2 entries persist; phase-3 was rejected without persist");
        assert_eq!(log[0].amount, small);
        assert_eq!(log[1].amount, big);
    }

    /// M-3.3 sliding-window admit case. Once the prior big debit
    /// itself ages out (>24h since its timestamp), a fresh full-cap
    /// debit is admitted.
    #[test]
    fn sliding_window_admits_after_oldest_entry_ages_out() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "ubluechip");

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(DAILY_EXPANSION_CAP.u128() * 4, "ubluechip"),
            );

        // First debit fills the cap.
        let env_t0 = mock_env();
        execute(
            deps.as_mut(),
            env_t0.clone(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: DAILY_EXPANSION_CAP,
            }),
        )
        .unwrap();

        // Advance just past the window from the FIRST debit's
        // timestamp. The prune cutoff is now > T0, so the T0 entry
        // ages out; the in-window total drops to zero. A full-cap
        // debit is admitted.
        let mut env_post = env_t0.clone();
        env_post.block.time = env_t0.block.time.plus_seconds(DAILY_WINDOW_SECONDS + 1);
        execute(
            deps.as_mut(),
            env_post,
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: DAILY_EXPANSION_CAP,
            }),
        )
        .expect("fresh window must admit a full-cap debit once the old entry ages out");

        let log = EXPANSION_LOG.load(&deps.storage).unwrap();
        assert_eq!(log.len(), 1, "old entry pruned, only the new one remains");
        assert_eq!(log[0].amount, DAILY_EXPANSION_CAP);
    }

    // ---------------------------------------------------------------
    // M-3.2 — factory cross-validation failure modes
    // ---------------------------------------------------------------
    //
    // `execute_expand_economy` cross-validates the factory's stored
    // `bluechip_denom` against this contract's stored denom on every
    // call. Two distinct failure modes produce typed errors that the
    // pool-side `RetryFactoryNotify` flow relies on being able to
    // distinguish from a balance-skip Ok response:
    //
    //   - BluechipDenomMismatch — the factory and expand-economy
    //     disagree on the canonical denom (config drift between the
    //     two independent 48h timelocks).
    //   - FactoryQueryFailed — the factory query itself errored
    //     (factory paused, mid-migrate, RPC blip).
    //
    // Both surface as Err so the factory-side `NotifyThresholdCrossed`
    // tx reverts and the pool can retry. These tests pin that
    // behaviour.

    /// Denom mismatch between factory and expand-economy returns the
    /// typed `BluechipDenomMismatch` error with both sides' denoms in
    /// the message body. Pool-side retry logic can decode and act on
    /// the structured error.
    #[test]
    fn factory_denom_mismatch_returns_typed_error() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps); // expand-economy denom = "ubluechip"

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");
        // Factory advertises a DIFFERENT denom than expand-economy holds.
        install_factory_denom_mock(&mut deps, factory_addr.clone(), "uother");

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(1_000_000_000, "ubluechip"),
            );

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: Uint128::new(10_000_000),
            }),
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("bluechip_denom mismatch"),
            "expected denom-mismatch error, got: {}",
            msg
        );
        assert!(
            msg.contains("\"uother\"") && msg.contains("\"ubluechip\""),
            "error must surface both sides' denoms for diagnosability; got: {}",
            msg
        );

        // No BankMsg sent on the rejection path; no log entry written.
        let log = EXPANSION_LOG.may_load(&deps.storage).unwrap();
        assert!(
            log.is_none() || log.unwrap().is_empty(),
            "mismatch-rejected request must not debit log"
        );
    }

    /// Factory query failure (unreachable / errors) surfaces as
    /// `FactoryQueryFailed` with the underlying reason. The handler
    /// fails closed rather than silently treating an unreachable
    /// factory as "denoms agree."
    #[test]
    fn factory_query_failure_returns_typed_error() {
        let mut deps = mock_dependencies();
        setup_contract(&mut deps);

        let factory_addr = MockApi::default().addr_make("factory");
        let user_addr = MockApi::default().addr_make("user");

        // Install a wasm-mock that ERRORS on the factory query — no
        // call to `install_factory_denom_mock`. The default mock
        // querier rejects any unmocked smart query, which is exactly
        // the shape we want to surface as FactoryQueryFailed.

        deps.querier
            .bank
            .update_balance(
                &mock_env().contract.address,
                coins(1_000_000_000, "ubluechip"),
            );

        let err = execute(
            deps.as_mut(),
            mock_env(),
            message_info(&factory_addr, &[]),
            ExecuteMsg::ExpandEconomy(ExpandEconomyMsg::RequestExpansion {
                recipient: user_addr.to_string(),
                amount: Uint128::new(10_000_000),
            }),
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("Failed to query factory config"),
            "expected FactoryQueryFailed, got: {}",
            msg
        );

        // No log mutation on the rejection path.
        let log = EXPANSION_LOG.may_load(&deps.storage).unwrap();
        assert!(
            log.is_none() || log.unwrap().is_empty(),
            "factory-query-failed request must not debit log"
        );
    }
}
