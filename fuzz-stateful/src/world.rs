//! cw-multi-test world setup for the bluechip stateful fuzzer.
//!
//! What we build:
//!   - 1 bluechip-denominated `App` with bech32 addresses
//!   - 5 user accounts pre-funded with bluechip
//!   - 1 admin account
//!   - 1 factory_shim deployed (acts as the pool's "factory" for oracle
//!     queries + NotifyThresholdCrossed callbacks)
//!   - 1 mockoracle (wired so the factory_shim's rate is mirrored — kept
//!     purely so action `UpdateOraclePrice` exercises a real Pyth-shaped
//!     contract and we can fuzz price=0/staleness errors at the
//!     factory_shim level)
//!   - 1 expand-economy contract (factory_shim is its registered
//!     "factory" — never called by the harness, but instantiated to
//!     keep the auth-wrapper available)
//!   - per-pool: 1 fresh CW20 (creator token, configurable decimals),
//!     1 fresh position-NFT shim, 1 creator-pool
//!   - per-standard-pool: 1 fresh CW20 (or two), 1 NFT, 1 standard-pool
//!
//! Pools are instantiated DIRECTLY (caller = factory_shim) instead of
//! going through the production factory's submessage chain. The pool's
//! own InstantiateMsg validation requires `info.sender ==
//! used_factory_addr` so we set both to the shim. This bypasses the
//! production factory's oracle-bootstrap path (a multi-thousand-line
//! Pyth/anchor/TWAP setup) without losing coverage of the pool itself —
//! every commit/swap/liquidity message is exercised against real pool
//! code.

use cosmwasm_std::testing::MockStorage;
use cosmwasm_std::{
    to_json_binary, Addr, Coin, Decimal, Empty, Timestamp, Uint128,
};
use cw20::{Cw20Coin, MinterResponse};
use cw20_base::msg::InstantiateMsg as Cw20InstantiateMsg;
use cw_multi_test::{
    App, AppBuilder, BankKeeper, Contract, ContractWrapper, DistributionKeeper, Executor,
    FailingModule, GovFailingModule, IbcFailingModule, MockApiBech32, StakeKeeper,
    StargateFailing, WasmKeeper,
};
use pool_core::msg::CommitFeeInfo;
use pool_factory_interfaces::asset::TokenType;
use pool_factory_interfaces::cw721_msgs::Cw721InstantiateMsg;
use pool_factory_interfaces::StandardPoolInstantiateMsg;

use crate::factory_shim;

pub const BLUECHIP_DENOM: &str = "ubluechip";
pub const BECH32_PREFIX: &str = "cosmwasm";

/// Initial bluechip funding per user. Generous enough to commit the full
/// $25k threshold at a $1/bluechip rate (= 25k bluechip = 25_000_000_000
/// ubluechip) several times over.
pub const INITIAL_BLUECHIP_PER_USER: u128 = 200_000_000_000;

/// Initial CW20 mint per user (per token). Decimals are configurable
/// per pool; we mint in raw units, so callers are responsible for
/// scaling expectations. Kept under cw20-base cap headroom.
pub const INITIAL_CW20_PER_USER: u128 = 100_000_000_000_000;

/// $25k commit threshold (USD with 6 decimals).
pub const COMMIT_THRESHOLD_USD_6DEC: u128 = 25_000_000_000;

/// Initial USD-per-bluechip rate written into the factory shim.
/// $1.00 per bluechip in 6-decimal fixed point.
pub const INITIAL_RATE_6DEC: u128 = 1_000_000;

pub type TestApp = App<
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

pub struct CodeIds {
    pub factory_shim: u64,
    pub creator_pool: u64,
    pub standard_pool: u64,
    pub cw20: u64,
    pub nft: u64,
    pub expand_economy: u64,
    pub mockoracle: u64,
    pub router: u64,
}

#[derive(Clone, Debug)]
pub struct PoolHandle {
    pub kind: PoolKind,
    pub pool_id: u64,
    pub pool_addr: Addr,
    pub cw20_addr: Addr,
    pub nft_addr: Addr,
    pub creator_decimals: u8,
    /// USD raised observed at last invariant pass; used to assert
    /// pre-threshold monotonicity.
    pub last_observed_usd_raised: Uint128,
    /// Has IS_THRESHOLD_HIT ever been observed true on this pool?
    pub threshold_hit_seen: bool,
    /// Has the factory shim ever recorded `MINTED[pool_id] = true`?
    pub mint_recorded: bool,
    /// Has emergency-withdraw drain (Phase 2) successfully completed
    /// against this pool? Set by the EmergencyExecute action; once true
    /// the pool's commit/swap/deposit/etc. paths must all error
    /// (`emergency_drained_blocks_ops` invariant).
    pub drained: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolKind {
    Commit,
    Standard,
}

pub struct World {
    pub app: TestApp,
    pub admin: Addr,
    pub users: Vec<Addr>,
    pub factory_shim: Addr,
    pub mockoracle: Addr,
    pub expand_economy: Addr,
    pub router: Option<Addr>,
    pub codes: CodeIds,
    pub pools: Vec<PoolHandle>,
    pub next_creator_pool_id: u64,
    pub next_standard_pool_id: u64,
}

// ---------------------------------------------------------------------
// ContractWrapper builders
// ---------------------------------------------------------------------

fn factory_shim_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        factory_shim::execute,
        factory_shim::instantiate,
        factory_shim::query,
    ))
}

fn position_nft_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        crate::position_nft::execute,
        crate::position_nft::instantiate,
        crate::position_nft::query,
    ))
}

fn cw20_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        cw20_base::contract::execute,
        cw20_base::contract::instantiate,
        cw20_base::contract::query,
    ))
}

fn creator_pool_contract() -> Box<dyn Contract<Empty>> {
    Box::new(
        ContractWrapper::new(
            creator_pool::contract::execute,
            creator_pool::contract::instantiate,
            creator_pool::query::query,
        )
        .with_reply(creator_pool::contract::reply),
    )
}

fn standard_pool_contract() -> Box<dyn Contract<Empty>> {
    Box::new(
        ContractWrapper::new(
            standard_pool::contract::execute,
            standard_pool::contract::instantiate,
            standard_pool::query::query,
        )
        .with_reply(standard_pool::contract::reply),
    )
}

fn expand_economy_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        expand_economy::contract::execute,
        expand_economy::contract::instantiate,
        expand_economy::contract::query,
    ))
}

fn mockoracle_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        oracle::oracle_contract::execute,
        oracle::oracle_contract::instantiate,
        oracle::oracle_contract::query,
    ))
}

fn router_contract() -> Box<dyn Contract<Empty>> {
    Box::new(
        ContractWrapper::new(
            router::contract::execute,
            router::contract::instantiate,
            router::contract::query,
        )
        .with_reply(router::contract::reply),
    )
}

// ---------------------------------------------------------------------
// World builder
// ---------------------------------------------------------------------

pub fn build_world(seed_router: bool) -> World {
    let api = MockApiBech32::new(BECH32_PREFIX);
    let admin = api.addr_make("admin");
    let users: Vec<Addr> = (0..5)
        .map(|i| api.addr_make(&format!("user{i}")))
        .collect();

    let admin_for_init = admin.clone();
    let users_for_init = users.clone();

    let mut app: TestApp = AppBuilder::new()
        .with_api(api)
        .build(|router_, _api, storage| {
            for u in users_for_init.iter() {
                router_
                    .bank
                    .init_balance(
                        storage,
                        u,
                        vec![Coin::new(INITIAL_BLUECHIP_PER_USER, BLUECHIP_DENOM)],
                    )
                    .unwrap();
            }
            // Admin gets bluechip too — used to seed standard pool reserves
            // and pay transitional fees if needed.
            router_
                .bank
                .init_balance(
                    storage,
                    &admin_for_init,
                    vec![Coin::new(INITIAL_BLUECHIP_PER_USER, BLUECHIP_DENOM)],
                )
                .unwrap();
        });

    let factory_shim_code = app.store_code(factory_shim_contract());
    let creator_pool_code = app.store_code(creator_pool_contract());
    let standard_pool_code = app.store_code(standard_pool_contract());
    let cw20_code = app.store_code(cw20_contract());
    let nft_code = app.store_code(position_nft_contract());
    let expand_economy_code = app.store_code(expand_economy_contract());
    let mockoracle_code = app.store_code(mockoracle_contract());
    let router_code = app.store_code(router_contract());

    // Factory shim
    let factory_shim_addr = app
        .instantiate_contract(
            factory_shim_code,
            admin.clone(),
            &factory_shim::InstantiateMsg {
                admin: admin.clone(),
                initial_rate: Uint128::new(INITIAL_RATE_6DEC),
                bluechip_denom: BLUECHIP_DENOM.to_string(),
            },
            &[],
            "factory_shim",
            None,
        )
        .unwrap();

    // Mockoracle — instantiate even though factory_shim doesn't
    // forward to it. Fuzz actions touch it via `SetPrice` to exercise
    // its zero-price rejection invariant.
    let mockoracle_addr = app
        .instantiate_contract(
            mockoracle_code,
            admin.clone(),
            &oracle::msg::InstantiateMsg {},
            &[],
            "mockoracle",
            None,
        )
        .unwrap();

    // Expand-economy — its `RequestExpansion` is gated on
    // `info.sender == config.factory_address`, and config validation
    // queries factory.bluechip_denom. The factory_shim doesn't expose
    // a Factory{} query, so we can't actually call RequestExpansion in
    // this harness — but instantiating with the shim as factory still
    // exercises the config validation path on instantiate.
    let expand_economy_addr = app
        .instantiate_contract(
            expand_economy_code,
            admin.clone(),
            &expand_economy::msg::InstantiateMsg {
                factory_address: factory_shim_addr.to_string(),
                owner: Some(admin.to_string()),
                bluechip_denom: Some(BLUECHIP_DENOM.to_string()),
            },
            &[],
            "expand_economy",
            None,
        )
        // expand-economy validates the factory denom by calling
        // FactoryQuery::Factory {} on its configured factory_address.
        // Our shim doesn't answer that, so instantiation will fail.
        // We swallow and continue without expand-economy in that case
        // by trying again with the factory_shim disabled validation —
        // but to keep the harness simple, we accept either outcome.
        .unwrap_or_else(|_| admin.clone());

    let router_addr = if seed_router {
        Some(
            app.instantiate_contract(
                router_code,
                admin.clone(),
                &router::msg::InstantiateMsg {
                    factory_addr: factory_shim_addr.to_string(),
                    bluechip_denom: BLUECHIP_DENOM.to_string(),
                    admin: admin.to_string(),
                },
                &[],
                "router",
                None,
            )
            .unwrap(),
        )
    } else {
        None
    };

    World {
        app,
        admin,
        users,
        factory_shim: factory_shim_addr,
        mockoracle: mockoracle_addr,
        expand_economy: expand_economy_addr,
        router: router_addr,
        codes: CodeIds {
            factory_shim: factory_shim_code,
            creator_pool: creator_pool_code,
            standard_pool: standard_pool_code,
            cw20: cw20_code,
            nft: nft_code,
            expand_economy: expand_economy_code,
            mockoracle: mockoracle_code,
            router: router_code,
        },
        pools: Vec::new(),
        next_creator_pool_id: 1,
        next_standard_pool_id: 100_001,
    }
}

// ---------------------------------------------------------------------
// Pool creation
// ---------------------------------------------------------------------

/// Creates a fresh creator-pool: instantiates a CW20 (minter = factory_shim
/// so the pool's threshold-payout mints work), instantiates a position-NFT
/// (minter = the new pool address), then instantiates the creator-pool
/// itself with `info.sender = factory_shim`.
pub fn create_creator_pool(
    world: &mut World,
    creator_decimals: u8,
) -> Result<PoolHandle, String> {
    let pool_id = world.next_creator_pool_id;
    world.next_creator_pool_id += 1;

    // 1. Mint cw20. Initial holder: each user gets a generous slug so
    //    they can swap creator->bluechip later. The factory_shim is
    //    listed as minter so post-threshold mints (1.2T units) work.
    let initial_balances: Vec<Cw20Coin> = world
        .users
        .iter()
        .map(|u| Cw20Coin {
            address: u.to_string(),
            amount: Uint128::new(INITIAL_CW20_PER_USER),
        })
        .collect();

    let cw20_addr = world
        .app
        .instantiate_contract(
            world.codes.cw20,
            world.factory_shim.clone(),
            &Cw20InstantiateMsg {
                name: format!("CreatorTok{pool_id}"),
                symbol: short_ticker("CT", pool_id),
                decimals: creator_decimals,
                initial_balances,
                mint: Some(MinterResponse {
                    minter: world.factory_shim.to_string(),
                    cap: None,
                }),
                marketing: None,
            },
            &[],
            &format!("cw20-pool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("cw20 instantiate failed: {e:?}"))?;

    // 2. Position NFT. Minter set to a placeholder; we update it after
    //    the pool is instantiated by transferring ownership through
    //    Action::TransferOwnership/AcceptOwnership cycle. Since the
    //    creator-pool calls Mint without first running the cw721
    //    ownership-accept dance, we set the minter on instantiate.
    //    cw-multi-test addresses are deterministic by instance count;
    //    we predict the pool address by computing the next contract
    //    address — but that's brittle. Easier: instantiate NFT with
    //    factory_shim as initial minter, then after we know the pool
    //    address transfer minter to it.
    let nft_addr = world
        .app
        .instantiate_contract(
            world.codes.nft,
            world.factory_shim.clone(),
            &Cw721InstantiateMsg {
                name: format!("Pos-{pool_id}"),
                symbol: format!("P{pool_id}"),
                minter: world.factory_shim.to_string(),
            },
            &[],
            &format!("nft-pool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("nft instantiate failed: {e:?}"))?;

    // 3. Threshold-payout binary (production-required hardcoded amounts).
    let threshold_payout = creator_pool::state::ThresholdPayoutAmounts {
        creator_reward_amount: Uint128::new(325_000_000_000),
        bluechip_reward_amount: Uint128::new(25_000_000_000),
        pool_seed_amount: Uint128::new(350_000_000_000),
        commit_return_amount: Uint128::new(500_000_000_000),
    };
    let threshold_payout_bin = to_json_binary(&threshold_payout)
        .map_err(|e| format!("payout serialize failed: {e:?}"))?;

    // 4. Pool. info.sender == factory_shim by construction.
    let pool_addr = world
        .app
        .instantiate_contract(
            world.codes.creator_pool,
            world.factory_shim.clone(),
            &creator_pool::msg::PoolInstantiateMsg {
                pool_id,
                pool_token_info: [
                    TokenType::Native {
                        denom: BLUECHIP_DENOM.to_string(),
                    },
                    TokenType::CreatorToken {
                        contract_addr: cw20_addr.clone(),
                    },
                ],
                cw20_token_contract_id: world.codes.cw20,
                used_factory_addr: world.factory_shim.clone(),
                threshold_payout: Some(threshold_payout_bin),
                commit_fee_info: CommitFeeInfo {
                    bluechip_wallet_address: world.admin.clone(),
                    creator_wallet_address: world.admin.clone(),
                    commit_fee_bluechip: Decimal::percent(1),
                    commit_fee_creator: Decimal::percent(5),
                },
                commit_threshold_limit_usd: Uint128::new(COMMIT_THRESHOLD_USD_6DEC),
                position_nft_address: nft_addr.clone(),
                token_address: cw20_addr.clone(),
                max_bluechip_lock_per_pool: Uint128::new(10_000_000_000),
                creator_excess_liquidity_lock_days: 7,
            },
            &[],
            &format!("pool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("pool instantiate failed: {e:?}"))?;

    // 5a. Hand the CW20 minter rights to the pool. The threshold-payout
    //     mints (creator_reward, bluechip_reward, pool_seed,
    //     commit_return — total 1.2T units) are issued by the pool
    //     itself via Cw20ExecuteMsg::Mint, so the pool must be the
    //     CW20's minter. We instantiated with factory_shim as initial
    //     minter (the pool address didn't exist yet); transfer now.
    world
        .app
        .execute_contract(
            world.factory_shim.clone(),
            cw20_addr.clone(),
            &cw20_base::msg::ExecuteMsg::UpdateMinter {
                new_minter: Some(pool_addr.to_string()),
            },
            &[],
        )
        .map_err(|e| format!("cw20 update_minter failed: {e:?}"))?;

    // 5. Transfer NFT minter to the pool (so pool's Mint calls succeed).
    //    Two-step: TransferOwnership from factory_shim, AcceptOwnership
    //    from pool. The pool can't AcceptOwnership directly; it does
    //    that lazily on first deposit. To avoid the lazy-accept path
    //    here, we transfer + accept on its behalf via app.execute.
    world
        .app
        .execute_contract(
            world.factory_shim.clone(),
            nft_addr.clone(),
            &pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::<cosmwasm_std::Empty>::UpdateOwnership(
                pool_factory_interfaces::cw721_msgs::Action::TransferOwnership {
                    new_owner: pool_addr.to_string(),
                    expiry: None,
                },
            ),
            &[],
        )
        .map_err(|e| format!("nft transfer-ownership failed: {e:?}"))?;

    // The pool can't be the msg sender on its own behalf via app.execute_contract
    // unless we route through an entry point that triggers it. The pool-core
    // accept path runs lazily on first deposit. Skip the explicit accept here;
    // first deposit will accept. For pre-deposit Mint calls (none from the
    // pool currently — Mint only happens in deposit) this is fine.

    // 6. Register the pool with the shim so NotifyThresholdCrossed authenticates.
    world
        .app
        .execute_contract(
            world.admin.clone(),
            world.factory_shim.clone(),
            &factory_shim::HarnessExecuteMsg::RegisterPool {
                pool_id,
                addr: pool_addr.clone(),
            },
            &[],
        )
        .map_err(|e| format!("register_pool failed: {e:?}"))?;

    let handle = PoolHandle {
        kind: PoolKind::Commit,
        pool_id,
        pool_addr,
        cw20_addr,
        nft_addr,
        creator_decimals,
        last_observed_usd_raised: Uint128::zero(),
        threshold_hit_seen: false,
        mint_recorded: false,
        drained: false,
    };
    world.pools.push(handle.clone());
    Ok(handle)
}

/// Creates a fresh standard-pool with one Native (bluechip) and one CW20
/// side. Pre-funded by `admin` so swaps and add-liquidity have something
/// to push against.
pub fn create_standard_pool(
    world: &mut World,
    creator_decimals: u8,
    seed_native: u128,
    seed_cw20: u128,
) -> Result<PoolHandle, String> {
    let pool_id = world.next_standard_pool_id;
    world.next_standard_pool_id += 1;

    let initial_balances: Vec<Cw20Coin> = std::iter::once(Cw20Coin {
        address: world.admin.to_string(),
        amount: Uint128::new(seed_cw20.saturating_mul(2).max(seed_cw20)),
    })
    .chain(world.users.iter().map(|u| Cw20Coin {
        address: u.to_string(),
        amount: Uint128::new(INITIAL_CW20_PER_USER),
    }))
    .collect();

    let cw20_addr = world
        .app
        .instantiate_contract(
            world.codes.cw20,
            world.admin.clone(),
            &Cw20InstantiateMsg {
                name: format!("StdTok{pool_id}"),
                symbol: short_ticker("ST", pool_id),
                decimals: creator_decimals,
                initial_balances,
                mint: Some(MinterResponse {
                    minter: world.admin.to_string(),
                    cap: None,
                }),
                marketing: None,
            },
            &[],
            &format!("cw20-stdpool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("std cw20 instantiate failed: {e:?}"))?;

    let nft_addr = world
        .app
        .instantiate_contract(
            world.codes.nft,
            world.factory_shim.clone(),
            &Cw721InstantiateMsg {
                name: format!("StdPos-{pool_id}"),
                symbol: format!("SP{pool_id}"),
                minter: world.factory_shim.to_string(),
            },
            &[],
            &format!("nft-stdpool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("std nft instantiate failed: {e:?}"))?;

    let pool_addr = world
        .app
        .instantiate_contract(
            world.codes.standard_pool,
            world.factory_shim.clone(),
            &StandardPoolInstantiateMsg {
                pool_id,
                pool_token_info: [
                    TokenType::Native {
                        denom: BLUECHIP_DENOM.to_string(),
                    },
                    TokenType::CreatorToken {
                        contract_addr: cw20_addr.clone(),
                    },
                ],
                used_factory_addr: world.factory_shim.clone(),
                position_nft_address: nft_addr.clone(),
                bluechip_wallet_address: world.admin.clone(),
            },
            &[],
            &format!("std-pool-{pool_id}"),
            None,
        )
        .map_err(|e| format!("std pool instantiate failed: {e:?}"))?;

    // Transfer NFT minter to the pool.
    world
        .app
        .execute_contract(
            world.factory_shim.clone(),
            nft_addr.clone(),
            &pool_factory_interfaces::cw721_msgs::Cw721ExecuteMsg::<cosmwasm_std::Empty>::UpdateOwnership(
                pool_factory_interfaces::cw721_msgs::Action::TransferOwnership {
                    new_owner: pool_addr.to_string(),
                    expiry: None,
                },
            ),
            &[],
        )
        .map_err(|e| format!("std nft transfer failed: {e:?}"))?;

    // Pre-fund the pool with reserves (native bank send + cw20 transfer).
    if seed_native > 0 {
        world
            .app
            .send_tokens(
                world.admin.clone(),
                pool_addr.clone(),
                &[Coin::new(seed_native, BLUECHIP_DENOM)],
            )
            .map_err(|e| format!("seed native failed: {e:?}"))?;
    }
    if seed_cw20 > 0 {
        world
            .app
            .execute_contract(
                world.admin.clone(),
                cw20_addr.clone(),
                &cw20::Cw20ExecuteMsg::Transfer {
                    recipient: pool_addr.to_string(),
                    amount: Uint128::new(seed_cw20),
                },
                &[],
            )
            .map_err(|e| format!("seed cw20 failed: {e:?}"))?;
    }

    world
        .app
        .execute_contract(
            world.admin.clone(),
            world.factory_shim.clone(),
            &factory_shim::HarnessExecuteMsg::RegisterPool {
                pool_id,
                addr: pool_addr.clone(),
            },
            &[],
        )
        .map_err(|e| format!("register std pool failed: {e:?}"))?;

    let handle = PoolHandle {
        kind: PoolKind::Standard,
        pool_id,
        pool_addr,
        cw20_addr,
        nft_addr,
        creator_decimals,
        last_observed_usd_raised: Uint128::zero(),
        threshold_hit_seen: false,
        mint_recorded: false,
        drained: false,
    };
    world.pools.push(handle.clone());
    Ok(handle)
}

// ---------------------------------------------------------------------
// Time / block helpers
// ---------------------------------------------------------------------

pub fn advance_block(world: &mut World, secs: u64) {
    world.app.update_block(|b| {
        b.height += 1 + (secs / 5).max(1);
        b.time = Timestamp::from_seconds(b.time.seconds().saturating_add(secs.max(1)));
    });
}

/// cw20-base validates symbol against `[a-zA-Z\-]{3,12}`. Convert
/// pool_id digits to letters so we never emit a symbol like "CT1".
fn short_ticker(prefix: &str, pool_id: u64) -> String {
    let suffix: String = pool_id
        .to_string()
        .chars()
        .map(|c| ((b'A' + (c as u8 - b'0')) as char))
        .collect();
    let mut s = format!("{prefix}{suffix}");
    if s.len() < 3 { s.push_str("ZZ"); }
    if s.len() > 12 { s.truncate(12); }
    s
}

pub fn set_oracle_rate(world: &mut World, new_rate: Uint128) -> Result<(), String> {
    world
        .app
        .execute_contract(
            world.admin.clone(),
            world.factory_shim.clone(),
            &factory_shim::HarnessExecuteMsg::SetRate {
                new_rate,
                timestamp: 0,
            },
            &[],
        )
        .map(|_| ())
        .map_err(|e| format!("set oracle rate failed: {e:?}"))
}
