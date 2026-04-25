//! Router integration tests.
//!
//! Stands up a `cw-multi-test` world with a handful of mock pools and
//! creator-token CW20s, then exercises the router end to end. The mock
//! pool implements just the surface area the router needs (see
//! [`crate::testing::mock_pool`]) which keeps each test focused on
//! router behaviour rather than factory + oracle + threshold setup.

use cosmwasm_std::testing::MockStorage;
use cosmwasm_std::{to_json_binary, Addr, Coin, Empty, Timestamp, Uint128};
use cw20::{BalanceResponse, Cw20Coin, Cw20ExecuteMsg, Cw20QueryMsg, MinterResponse};
use cw20_base::msg::InstantiateMsg as Cw20InstantiateMsg;
use cw_multi_test::{
    App, AppBuilder, BankKeeper, Contract, ContractWrapper, DistributionKeeper, Executor,
    FailingModule, GovFailingModule, IbcFailingModule, MockApiBech32, StakeKeeper, StargateFailing,
    WasmKeeper,
};
use pool_factory_interfaces::asset::TokenType;
use pool_factory_interfaces::routing::SwapOperation;

use crate::contract;
use crate::msg::{
    ConfigResponse, Cw20HookMsg, ExecuteMsg as RouterExecuteMsg,
    InstantiateMsg as RouterInstantiateMsg, QueryMsg as RouterQueryMsg, SimulateMultiHopResponse,
};
use crate::testing::mock_pool;

const BLUECHIP_DENOM: &str = "ubluechip";
const POOL_RESERVE: u128 = 1_000_000;
const USER_NATIVE: u128 = 10_000_000;
const USER_CW20: u128 = 1_000_000;

type TestApp = App<
    BankKeeper,
    MockApiBech32,
    MockStorage,
    FailingModule<Empty, Empty, Empty>,
    WasmKeeper<Empty, Empty>,
    StakeKeeper,
    DistributionKeeper,
    IbcFailingModule,
    GovFailingModule,
    StargateFailing,
>;

struct World {
    app: TestApp,
    user: Addr,
    admin: Addr,
    router: Addr,
    creator_a: Addr,
    creator_b: Addr,
    creator_c: Addr,
    pool_a: Addr,
    pool_b: Addr,
    pool_c: Addr,
    pool_uncommitted: Addr,
    pool_empty: Addr,
}

fn router_contract() -> Box<dyn Contract<Empty>> {
    Box::new(
        ContractWrapper::new(contract::execute, contract::instantiate, contract::query)
            .with_reply(contract::reply),
    )
}

fn mock_pool_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        mock_pool::execute,
        mock_pool::instantiate,
        mock_pool::query,
    ))
}

fn cw20_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        cw20_base::contract::execute,
        cw20_base::contract::instantiate,
        cw20_base::contract::query,
    ))
}

fn setup_world() -> World {
    let api = MockApiBech32::new("cosmwasm");
    let user = api.addr_make("user");
    let admin = api.addr_make("admin");
    let factory = api.addr_make("factory");

    let user_for_init = user.clone();
    let admin_for_init = admin.clone();
    let mut app: TestApp = AppBuilder::new()
        .with_api(api)
        .build(|router, _api, storage| {
            router
                .bank
                .init_balance(
                    storage,
                    &user_for_init,
                    vec![Coin::new(USER_NATIVE, BLUECHIP_DENOM)],
                )
                .unwrap();
            router
                .bank
                .init_balance(
                    storage,
                    &admin_for_init,
                    vec![Coin::new(20 * POOL_RESERVE, BLUECHIP_DENOM)],
                )
                .unwrap();
        });

    let cw20_code = app.store_code(cw20_contract());
    let pool_code = app.store_code(mock_pool_contract());
    let router_code = app.store_code(router_contract());

    let creator_a =
        instantiate_creator_token(&mut app, cw20_code, &admin, &user, "Creator A", "CRA");
    let creator_b =
        instantiate_creator_token(&mut app, cw20_code, &admin, &user, "Creator B", "CRB");
    let creator_c =
        instantiate_creator_token(&mut app, cw20_code, &admin, &user, "Creator C", "CRC");
    let creator_uncommitted = instantiate_creator_token(
        &mut app,
        cw20_code,
        &admin,
        &user,
        "Creator Uncommitted",
        "CRU",
    );
    let creator_empty =
        instantiate_creator_token(&mut app, cw20_code, &admin, &user, "Creator Empty", "CRE");

    let pool_a = instantiate_pool(&mut app, pool_code, &admin, &creator_a, true, true);
    let pool_b = instantiate_pool(&mut app, pool_code, &admin, &creator_b, true, true);
    let pool_c = instantiate_pool(&mut app, pool_code, &admin, &creator_c, true, true);
    let pool_uncommitted = instantiate_pool(
        &mut app,
        pool_code,
        &admin,
        &creator_uncommitted,
        false,
        true,
    );
    let pool_empty = instantiate_pool(&mut app, pool_code, &admin, &creator_empty, true, false);

    let router = app
        .instantiate_contract(
            router_code,
            admin.clone(),
            &RouterInstantiateMsg {
                factory_addr: factory.to_string(),
                bluechip_denom: BLUECHIP_DENOM.to_string(),
                admin: admin.to_string(),
            },
            &[],
            "router",
            None,
        )
        .unwrap();

    World {
        app,
        user,
        admin,
        router,
        creator_a,
        creator_b,
        creator_c,
        pool_a,
        pool_b,
        pool_c,
        pool_uncommitted,
        pool_empty,
    }
}

fn instantiate_creator_token(
    app: &mut TestApp,
    code_id: u64,
    admin: &Addr,
    user: &Addr,
    name: &str,
    symbol: &str,
) -> Addr {
    app.instantiate_contract(
        code_id,
        admin.clone(),
        &Cw20InstantiateMsg {
            name: name.to_string(),
            symbol: symbol.to_string(),
            decimals: 6,
            initial_balances: vec![
                Cw20Coin {
                    address: user.to_string(),
                    amount: Uint128::new(USER_CW20),
                },
                Cw20Coin {
                    address: admin.to_string(),
                    amount: Uint128::new(2 * POOL_RESERVE),
                },
            ],
            mint: Some(MinterResponse {
                minter: admin.to_string(),
                cap: None,
            }),
            marketing: None,
        },
        &[],
        symbol,
        None,
    )
    .unwrap()
}

fn instantiate_pool(
    app: &mut TestApp,
    code_id: u64,
    admin: &Addr,
    creator: &Addr,
    fully_committed: bool,
    seed_reserves: bool,
) -> Addr {
    let pool = app
        .instantiate_contract(
            code_id,
            admin.clone(),
            &mock_pool::InstantiateMsg {
                asset_infos: [
                    TokenType::Native {
                        denom: BLUECHIP_DENOM.to_string(),
                    },
                    TokenType::CreatorToken {
                        contract_addr: creator.clone(),
                    },
                ],
                fully_committed,
            },
            &[],
            "mock_pool",
            None,
        )
        .unwrap();
    if seed_reserves {
        app.send_tokens(
            admin.clone(),
            pool.clone(),
            &[Coin::new(POOL_RESERVE, BLUECHIP_DENOM)],
        )
        .unwrap();
        app.execute_contract(
            admin.clone(),
            creator.clone(),
            &Cw20ExecuteMsg::Transfer {
                recipient: pool.to_string(),
                amount: Uint128::new(POOL_RESERVE),
            },
            &[],
        )
        .unwrap();
    }
    pool
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

fn cw20_balance(app: &TestApp, token: &Addr, account: &Addr) -> Uint128 {
    let res: BalanceResponse = app
        .wrap()
        .query_wasm_smart(
            token,
            &Cw20QueryMsg::Balance {
                address: account.to_string(),
            },
        )
        .unwrap();
    res.balance
}

fn bank_balance(app: &TestApp, account: &Addr, denom: &str) -> Uint128 {
    app.wrap().query_balance(account, denom).unwrap().amount
}

fn op(pool: &Addr, offer: TokenType, ask: TokenType) -> SwapOperation {
    SwapOperation {
        pool_addr: pool.to_string(),
        offer_asset_info: offer,
        ask_asset_info: ask,
    }
}

// ---------------------------------------------------------------------------
// Test cases
// ---------------------------------------------------------------------------

#[test]
fn happy_path_two_hop_creator_to_creator() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let creator_b = TokenType::CreatorToken {
        contract_addr: world.creator_b.clone(),
    };

    let route = vec![
        op(&world.pool_a, creator_a.clone(), bluechip.clone()),
        op(&world.pool_b, bluechip.clone(), creator_b.clone()),
    ];

    let amount = Uint128::new(100_000);
    let creator_b_before = cw20_balance(&world.app, &world.creator_b, &world.user);

    let send_msg = Cw20ExecuteMsg::Send {
        contract: world.router.to_string(),
        amount,
        msg: to_json_binary(&Cw20HookMsg::ExecuteMultiHop {
            operations: route,
            minimum_receive: Uint128::new(1),
            belief_price: None,
            max_spread: None,
            deadline: None,
            recipient: None,
        })
        .unwrap(),
    };

    world
        .app
        .execute_contract(world.user.clone(), world.creator_a.clone(), &send_msg, &[])
        .unwrap();

    let creator_b_after = cw20_balance(&world.app, &world.creator_b, &world.user);
    assert!(
        creator_b_after > creator_b_before,
        "user should receive creator B"
    );

    // Router holds zero of every involved token after a successful route.
    assert_eq!(
        cw20_balance(&world.app, &world.creator_a, &world.router),
        Uint128::zero()
    );
    assert_eq!(
        cw20_balance(&world.app, &world.creator_b, &world.router),
        Uint128::zero()
    );
    assert_eq!(
        bank_balance(&world.app, &world.router, BLUECHIP_DENOM),
        Uint128::zero()
    );
}

#[test]
fn single_hop_native_passthrough() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let route = vec![op(&world.pool_a, bluechip.clone(), creator_a)];

    let amount = Uint128::new(50_000);
    let creator_a_before = cw20_balance(&world.app, &world.creator_a, &world.user);

    world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(amount.u128(), BLUECHIP_DENOM)],
        )
        .unwrap();

    let creator_a_after = cw20_balance(&world.app, &world.creator_a, &world.user);
    assert!(creator_a_after > creator_a_before);
    assert_eq!(
        bank_balance(&world.app, &world.router, BLUECHIP_DENOM),
        Uint128::zero()
    );
    assert_eq!(
        cw20_balance(&world.app, &world.creator_a, &world.router),
        Uint128::zero()
    );
}

#[test]
fn slippage_exceeded_reverts_route() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let route = vec![op(&world.pool_a, bluechip.clone(), creator_a)];

    let amount = Uint128::new(50_000);
    let user_before = bank_balance(&world.app, &world.user, BLUECHIP_DENOM);

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                // Demand absurdly more than the pool can possibly return.
                minimum_receive: Uint128::new(u128::MAX / 2),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(amount.u128(), BLUECHIP_DENOM)],
        )
        .unwrap_err();
    assert!(
        err.root_cause().to_string().contains("Slippage exceeded"),
        "expected SlippageExceeded, got: {err}"
    );

    // Reverted: user balance unchanged, router holds nothing.
    let user_after = bank_balance(&world.app, &world.user, BLUECHIP_DENOM);
    assert_eq!(user_before, user_after);
    assert_eq!(
        bank_balance(&world.app, &world.router, BLUECHIP_DENOM),
        Uint128::zero()
    );
}

#[test]
fn max_hops_exceeded_rejected() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let creator_b = TokenType::CreatorToken {
        contract_addr: world.creator_b.clone(),
    };
    let creator_c = TokenType::CreatorToken {
        contract_addr: world.creator_c.clone(),
    };
    // Four hops: bluechip -> A -> bluechip -> B -> bluechip... exceed MAX_HOPS=3.
    let route = vec![
        op(&world.pool_a, bluechip.clone(), creator_a.clone()),
        op(&world.pool_a, creator_a.clone(), bluechip.clone()),
        op(&world.pool_b, bluechip.clone(), creator_b.clone()),
        op(&world.pool_c, creator_b.clone(), creator_c.clone()),
    ];

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(10_000u128, BLUECHIP_DENOM)],
        )
        .unwrap_err();
    assert!(
        err.root_cause().to_string().contains("maximum of 3 hops"),
        "expected MaxHopsExceeded, got: {err}"
    );
}

#[test]
fn deadline_expired_rejected() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let route = vec![op(&world.pool_a, bluechip.clone(), creator_a)];

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: Some(Timestamp::from_seconds(1)),
                recipient: None,
            },
            &[Coin::new(10_000u128, BLUECHIP_DENOM)],
        )
        .unwrap_err();
    assert!(
        err.root_cause().to_string().contains("deadline exceeded"),
        "expected DeadlineExceeded, got: {err}"
    );
}

#[test]
fn same_input_output_rejected() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    // bluechip -> A -> bluechip: structurally a round trip.
    let route = vec![
        op(&world.pool_a, bluechip.clone(), creator_a.clone()),
        op(&world.pool_a, creator_a, bluechip.clone()),
    ];

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(10_000u128, BLUECHIP_DENOM)],
        )
        .unwrap_err();
    assert!(
        err.root_cause()
            .to_string()
            .contains("input and final output must differ"),
        "expected SameInputOutput, got: {err}"
    );
}

#[test]
fn zero_liquidity_pool_in_path_errors_with_hop_context() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    // pool_empty was instantiated with seed_reserves=false, so its
    // bluechip and cw20 balances are both zero.
    let creator_empty_addr: Addr = {
        // Read the pool's pair to get the cw20 address it knows about.
        let pair: mock_pool::PairResponse = world
            .app
            .wrap()
            .query_wasm_smart(&world.pool_empty, &mock_pool::QueryMsg::Pair {})
            .unwrap();
        match &pair.asset_infos[1] {
            TokenType::CreatorToken { contract_addr } => contract_addr.clone(),
            _ => panic!("expected creator token on side 1"),
        }
    };
    let creator_empty = TokenType::CreatorToken {
        contract_addr: creator_empty_addr,
    };
    let route = vec![op(&world.pool_empty, bluechip.clone(), creator_empty)];

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(10_000u128, BLUECHIP_DENOM)],
        )
        .unwrap_err();
    let msg = err.root_cause().to_string();
    assert!(
        msg.contains("Hop 0") && msg.contains("no liquidity"),
        "expected HopFailed with hop context, got: {msg}"
    );
}

#[test]
fn router_holds_zero_after_successful_route() {
    // Same flow as the happy path but explicitly verifies the router's
    // balance for every involved asset both BEFORE and AFTER the route.
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let creator_b = TokenType::CreatorToken {
        contract_addr: world.creator_b.clone(),
    };
    let route = vec![
        op(&world.pool_a, creator_a.clone(), bluechip.clone()),
        op(&world.pool_b, bluechip.clone(), creator_b.clone()),
    ];

    for asset in [&world.creator_a, &world.creator_b] {
        assert_eq!(
            cw20_balance(&world.app, asset, &world.router),
            Uint128::zero()
        );
    }
    assert_eq!(
        bank_balance(&world.app, &world.router, BLUECHIP_DENOM),
        Uint128::zero()
    );

    let send_msg = Cw20ExecuteMsg::Send {
        contract: world.router.to_string(),
        amount: Uint128::new(100_000),
        msg: to_json_binary(&Cw20HookMsg::ExecuteMultiHop {
            operations: route,
            minimum_receive: Uint128::new(1),
            belief_price: None,
            max_spread: None,
            deadline: None,
            recipient: None,
        })
        .unwrap(),
    };
    world
        .app
        .execute_contract(world.user.clone(), world.creator_a.clone(), &send_msg, &[])
        .unwrap();

    for asset in [&world.creator_a, &world.creator_b] {
        assert_eq!(
            cw20_balance(&world.app, asset, &world.router),
            Uint128::zero(),
            "router still holds {asset} after route",
        );
    }
    assert_eq!(
        bank_balance(&world.app, &world.router, BLUECHIP_DENOM),
        Uint128::zero()
    );
}

#[test]
fn simulate_matches_execute() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_a = TokenType::CreatorToken {
        contract_addr: world.creator_a.clone(),
    };
    let creator_b = TokenType::CreatorToken {
        contract_addr: world.creator_b.clone(),
    };
    let route = vec![
        op(&world.pool_a, creator_a.clone(), bluechip.clone()),
        op(&world.pool_b, bluechip.clone(), creator_b.clone()),
    ];
    let offer_amount = Uint128::new(100_000);

    let sim: SimulateMultiHopResponse = world
        .app
        .wrap()
        .query_wasm_smart(
            &world.router,
            &RouterQueryMsg::SimulateMultiHop {
                operations: route.clone(),
                offer_amount,
            },
        )
        .unwrap();
    assert_eq!(sim.intermediate_amounts.len(), 2);
    assert_eq!(sim.final_amount, *sim.intermediate_amounts.last().unwrap());

    let creator_b_before = cw20_balance(&world.app, &world.creator_b, &world.user);
    let send_msg = Cw20ExecuteMsg::Send {
        contract: world.router.to_string(),
        amount: offer_amount,
        msg: to_json_binary(&Cw20HookMsg::ExecuteMultiHop {
            operations: route,
            minimum_receive: Uint128::new(1),
            belief_price: None,
            max_spread: None,
            deadline: None,
            recipient: None,
        })
        .unwrap(),
    };
    world
        .app
        .execute_contract(world.user.clone(), world.creator_a.clone(), &send_msg, &[])
        .unwrap();
    let creator_b_after = cw20_balance(&world.app, &world.creator_b, &world.user);
    let actual_received = creator_b_after - creator_b_before;
    assert_eq!(
        actual_received, sim.final_amount,
        "execute output should match simulation exactly"
    );
}

#[test]
fn commit_phase_pool_rejected_in_simulation() {
    let world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_uncommitted_addr: Addr = {
        let pair: mock_pool::PairResponse = world
            .app
            .wrap()
            .query_wasm_smart(&world.pool_uncommitted, &mock_pool::QueryMsg::Pair {})
            .unwrap();
        match &pair.asset_infos[1] {
            TokenType::CreatorToken { contract_addr } => contract_addr.clone(),
            _ => panic!("expected creator token on side 1"),
        }
    };
    let creator_uncommitted = TokenType::CreatorToken {
        contract_addr: creator_uncommitted_addr,
    };
    let route = vec![op(
        &world.pool_uncommitted,
        bluechip.clone(),
        creator_uncommitted,
    )];

    let err = world
        .app
        .wrap()
        .query_wasm_smart::<SimulateMultiHopResponse>(
            &world.router,
            &RouterQueryMsg::SimulateMultiHop {
                operations: route,
                offer_amount: Uint128::new(10_000),
            },
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("commit phase"),
        "expected PoolInCommitPhase error from simulation, got: {msg}"
    );
}

#[test]
fn commit_phase_pool_rejected_in_execution() {
    let mut world = setup_world();

    let bluechip = TokenType::Native {
        denom: BLUECHIP_DENOM.to_string(),
    };
    let creator_uncommitted_addr: Addr = {
        let pair: mock_pool::PairResponse = world
            .app
            .wrap()
            .query_wasm_smart(&world.pool_uncommitted, &mock_pool::QueryMsg::Pair {})
            .unwrap();
        match &pair.asset_infos[1] {
            TokenType::CreatorToken { contract_addr } => contract_addr.clone(),
            _ => panic!("expected creator token on side 1"),
        }
    };
    let creator_uncommitted = TokenType::CreatorToken {
        contract_addr: creator_uncommitted_addr,
    };
    let route = vec![op(
        &world.pool_uncommitted,
        bluechip.clone(),
        creator_uncommitted,
    )];

    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::ExecuteMultiHop {
                operations: route,
                minimum_receive: Uint128::new(1),
                belief_price: None,
                max_spread: None,
                deadline: None,
                recipient: None,
            },
            &[Coin::new(10_000u128, BLUECHIP_DENOM)],
        )
        .unwrap_err();
    let msg = err.root_cause().to_string();
    assert!(
        msg.contains("Hop 0") && msg.contains("commit phase"),
        "expected HopFailed wrapping commit phase, got: {msg}"
    );
}

#[test]
fn update_config_admin_only() {
    let mut world = setup_world();

    // Non-admin attempt rejected.
    let err = world
        .app
        .execute_contract(
            world.user.clone(),
            world.router.clone(),
            &RouterExecuteMsg::UpdateConfig {
                admin: Some(world.user.to_string()),
                factory_addr: None,
            },
            &[],
        )
        .unwrap_err();
    assert!(err.root_cause().to_string().contains("Unauthorized"));

    // Admin can rotate.
    world
        .app
        .execute_contract(
            world.admin.clone(),
            world.router.clone(),
            &RouterExecuteMsg::UpdateConfig {
                admin: Some(world.user.to_string()),
                factory_addr: None,
            },
            &[],
        )
        .unwrap();

    let cfg: ConfigResponse = world
        .app
        .wrap()
        .query_wasm_smart(&world.router, &RouterQueryMsg::Config {})
        .unwrap();
    assert_eq!(cfg.admin, world.user);
    assert_eq!(cfg.bluechip_denom, BLUECHIP_DENOM);
}
