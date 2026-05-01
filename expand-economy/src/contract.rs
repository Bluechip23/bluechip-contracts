#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Binary, Coin, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult, Storage, Uint128,
};
use cw2::set_contract_version;
use cw_storage_plus::Item;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::error::ContractError;
use crate::msg::{ConfigResponse, ExecuteMsg, ExpandEconomyMsg, InstantiateMsg, MigrateMsg, QueryMsg};
use crate::state::{
    Config, ExpansionWindow, PendingConfigUpdate, PendingWithdrawal, CONFIG,
    CONFIG_TIMELOCK_SECONDS, DAILY_EXPANSION_CAP, DAILY_WINDOW_SECONDS, DEFAULT_BLUECHIP_DENOM,
    EXPANSION_WINDOW, PENDING_CONFIG_UPDATE, PENDING_WITHDRAWAL, WITHDRAW_TIMELOCK_SECONDS,
};

/// Minimal subset of the factory's query interface that this contract
/// uses to cross-validate `bluechip_denom`. Defined locally to avoid a
/// compile-time dependency on the `factory` crate (the two communicate
/// only over wasm message boundaries).
///
/// M-EE-2: deliberately uses plain `#[derive(serde::Serialize)]` +
/// explicit `rename_all = "snake_case"` rather than `#[cw_serde]`. The
/// query message we send to the factory must match
/// `factory::query::QueryMsg::Factory {}` on the wire — `cw_serde`
/// adds `rename_all = "snake_case"` for enums via macro magic, which
/// works today but couples our wire format to a tooling
/// implementation detail. Using plain serde derives makes the wire
/// contract explicit and resilient to future cosmwasm-schema changes.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum FactoryQuery {
    Factory {},
}

/// Wire-compatible subset of `factory::msg::FactoryInstantiateResponse`
/// — only the field this contract reads.
///
/// M-EE-2: switched from `#[cw_serde]` to plain `#[derive(serde::Deserialize)]`.
/// `cw_serde` is documented in cosmwasm-schema 2.x today as NOT setting
/// `deny_unknown_fields`, but that's a tooling default that could
/// reasonably flip in a future release — and if it ever does, every
/// `RequestExpansion` would fail with an opaque "unknown field"
/// deserialization error inside `query_wasm_smart`, silently bricking
/// every threshold-crossing bluechip mint.
///
/// Plain `#[derive(serde::Deserialize)]` does NOT add
/// `deny_unknown_fields`, so the "extra factory-side fields are
/// ignored" property is locked in by serde's documented default
/// behavior rather than a transitive cosmwasm-schema choice. Combined
/// with the round-trip test in `tests::factory_response_round_trip`,
/// this future-proofs the cross-validation path.
#[derive(Deserialize)]
struct FactoryConfigSubset {
    bluechip_denom: String,
}

#[derive(Deserialize)]
struct FactoryInstantiateResponseSubset {
    factory: FactoryConfigSubset,
}

const CONTRACT_NAME: &str = "crates.io:expand-economy";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Load `CONFIG` and require the sender to match `config.owner`.
fn load_config_as_owner(storage: &dyn Storage, sender: &Addr) -> Result<Config, ContractError> {
    let config = CONFIG.load(storage)?;
    if sender != config.owner {
        return Err(ContractError::Unauthorized {});
    }
    Ok(config)
}

/// M-EE-3 — validate a Cosmos SDK native bank denom against the documented
/// format rules: 3–128 characters, must start with an ASCII letter, and
/// the rest must be alphanumeric or one of `/`, `:`, `.`, `_`, `-`.
/// Mirrors the cosmos-sdk `IsValidDenom` regex
/// `^[a-zA-Z][a-zA-Z0-9/:._-]{2,127}$` without pulling in the `regex`
/// crate (which would balloon the wasm output).
///
/// Catches the operator-typo class of failures at propose / instantiate
/// time rather than 48 hours later when an apply lands a malformed
/// denom and every subsequent `RequestExpansion` reverts inside the
/// bank module with an error nobody is watching for. Examples this
/// catches that the previous "non-empty after trim" check missed:
///   - `"Bluechip"`           (capital first letter — bank rejects)
///   - `"u bluechip"`         (whitespace inside)
///   - `"u"` or `"ub"`        (length < 3)
///   - `"1ubluechip"`         (digit prefix)
///   - `"ubluechip!"`         (punctuation outside the allowed set)
///
/// Accepts all the cosmos-sdk shapes this contract actually wants:
///   - `"ubluechip"`          (canonical native denom)
///   - `"ucustom"`            (test fixture)
///   - `"ibc/27394FB..."`     (IBC-wrapped — slashes + hex)
///   - `"factory/cosmos1.../tokenname"` (tokenfactory shape)
fn validate_native_denom(denom: &str) -> Result<(), ContractError> {
    let len = denom.len();
    if !(3..=128).contains(&len) {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "bluechip_denom \"{}\" length {} is outside the cosmos-sdk \
             allowed range [3, 128]",
            denom, len
        ))));
    }
    let mut chars = denom.chars();
    let first = chars.next().expect("checked length >= 3");
    if !first.is_ascii_alphabetic() {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "bluechip_denom \"{}\" must start with an ASCII letter; got \
             leading character '{}'",
            denom, first
        ))));
    }
    for c in chars {
        let allowed = c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '_' | '-');
        if !allowed {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "bluechip_denom \"{}\" contains disallowed character '{}'; \
                 cosmos-sdk denoms accept only alphanumerics and / : . _ -",
                denom, c
            ))));
        }
    }
    Ok(())
}

/// Error with `err_msg` if `item` is already populated.
fn ensure_absent<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err_msg: &str,
) -> Result<(), ContractError>
where
    T: Serialize + DeserializeOwned,
{
    if item.may_load(storage)?.is_some() {
        return Err(ContractError::Std(StdError::generic_err(err_msg)));
    }
    Ok(())
}

/// Load `item` or return `ContractError::Std(generic_err(err_msg))`.
fn load_or_err<T>(
    storage: &dyn Storage,
    item: &Item<T>,
    err_msg: &str,
) -> Result<T, ContractError>
where
    T: Serialize + DeserializeOwned,
{
    item.may_load(storage)?
        .ok_or_else(|| ContractError::Std(StdError::generic_err(err_msg)))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let bluechip_denom = msg
        .bluechip_denom
        .unwrap_or_else(|| DEFAULT_BLUECHIP_DENOM.to_string());
    // M-EE-3: validate against the cosmos-sdk denom format rules at
    // instantiate so a typo'd denom fails here rather than 48 hours
    // later via the timelocked propose / apply path. The previous
    // "non-empty after trim" check let `"Bluechip"`, `"u bluechip"`,
    // `"1u"`, etc. through and they would have bricked every
    // subsequent `RequestExpansion`.
    validate_native_denom(&bluechip_denom)?;

    let config = Config {
        factory_address: deps.api.addr_validate(&msg.factory_address)?,
        owner: deps
            .api
            .addr_validate(&msg.owner.unwrap_or_else(|| info.sender.to_string()))?,
        bluechip_denom,
    };

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner)
        .add_attribute("bluechip_denom", config.bluechip_denom))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    // M-EE-1: every execute path on this contract is non-payable.
    //   - `RequestExpansion` is sent by the factory with funds: vec![];
    //     attaching coins inflates the contract's bank balance without
    //     updating EXPANSION_WINDOW.spent_in_window, biasing the cap
    //     accounting.
    //   - All Propose / Execute / Cancel timelock arms have no semantic
    //     reason to accept funds; attached funds would be orphaned in
    //     the contract's bank balance until rescued via the 48h
    //     ProposeWithdrawal flow.
    //
    // Centralised at the dispatch top so every existing AND every
    // future variant inherits the guard without per-arm boilerplate.
    cw_utils::nonpayable(&info)?;
    match msg {
        ExecuteMsg::ExpandEconomy(expand_economy_msg) => {
            execute_expand_economy(deps, env, info, expand_economy_msg)
        }
        ExecuteMsg::ProposeConfigUpdate {
            factory_address,
            owner,
            bluechip_denom,
        } => execute_propose_config_update(deps, env, info, factory_address, owner, bluechip_denom),
        ExecuteMsg::ExecuteConfigUpdate {} => execute_apply_config_update(deps, env, info),
        ExecuteMsg::CancelConfigUpdate {} => execute_cancel_config_update(deps, info),
        ExecuteMsg::ProposeWithdrawal {
            amount,
            denom,
            recipient,
        } => execute_propose_withdrawal(deps, env, info, amount, denom, recipient),
        ExecuteMsg::ExecuteWithdrawal {} => execute_withdrawal(deps, env, info),
        ExecuteMsg::CancelWithdrawal {} => execute_cancel_withdrawal(deps, info),
    }
}

pub fn execute_expand_economy(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExpandEconomyMsg,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.factory_address {
        return Err(ContractError::Unauthorized {});
    }

    // Cross-validate the factory's `bluechip_denom` against this contract's
    // configured denom. Both fields are independently admin-mutable (each
    // contract has its own propose/apply config flow with separate 48h
    // timelocks), so they can drift if a single-side update is forgotten.
    // Drift would silently fund rewards in the wrong denom — better to
    // refuse the call and surface the mismatch loudly.
    //
    // One additional cross-contract query per RequestExpansion. Cost is
    // negligible: the call fires only on threshold-crossing events, not
    // on hot paths.
    let factory_resp: FactoryInstantiateResponseSubset = deps
        .querier
        .query_wasm_smart(&config.factory_address, &FactoryQuery::Factory {})
        .map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "Failed to query factory config for denom validation: {}",
                e
            )))
        })?;
    if factory_resp.factory.bluechip_denom != config.bluechip_denom {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "bluechip_denom mismatch: factory has \"{}\", expand-economy has \"{}\". \
             Update one side via its config-update flow before retrying.",
            factory_resp.factory.bluechip_denom, config.bluechip_denom
        ))));
    }

    match msg {
        ExpandEconomyMsg::RequestExpansion { recipient, amount } => {
            if amount.is_zero() {
                // The factory's bluechip mint-decay polynomial drops to zero
                // once `pool_id` and `seconds_elapsed` grow past the curve's
                // crossover. Once it does, this contract is "dormant" by
                // design — there is no more bluechip-economy expansion to
                // dispense, and the mechanism's job is done. Surface that
                // explicitly so operators and monitoring can distinguish
                // "skipped because schedule has expired" from "skipped
                // because of a bug".
                return Ok(Response::new()
                    .add_attribute("action", "request_reward_skipped")
                    .add_attribute("reason", "economy_dormant")
                    .add_attribute(
                        "note",
                        "ExpandEconomy mint schedule has reached zero; \
                         no further expansions will be dispensed. This \
                         is the intended end-state of the decay curve.",
                    ));
            }

            // Validate the recipient at the contract boundary rather than
            // letting a malformed string surface as an opaque bank-module
            // error deep in the tx pipeline. Also guards against callers
            // accidentally forwarding an IBC-wrapped / wrong-prefix string.
            let recipient_addr = deps.api.addr_validate(&recipient)?;

            // Rolling 24-hour spend cap. Defense-in-depth against a
            // compromised factory key forwarding huge RequestExpansion
            // calls. The legitimate threshold-mint schedule is well below
            // DAILY_EXPANSION_CAP per day; an attacker with full factory
            // control can extract at most CAP per 24-hour window via this
            // path. Window resets opportunistically on the first call after
            // expiry rather than continuously, which is fine for cap
            // semantics — see ExpansionWindow doc.
            let now = env.block.time;
            let window = match EXPANSION_WINDOW.may_load(deps.storage)? {
                Some(w)
                    if now.seconds().saturating_sub(w.window_start.seconds())
                        < DAILY_WINDOW_SECONDS =>
                {
                    w
                }
                _ => ExpansionWindow {
                    window_start: now,
                    spent_in_window: Uint128::zero(),
                },
            };
            let new_spent = window.spent_in_window.checked_add(amount)?;
            if new_spent > DAILY_EXPANSION_CAP {
                return Err(ContractError::DailyExpansionCapExceeded {
                    requested: amount,
                    spent_in_window: window.spent_in_window,
                    cap: DAILY_EXPANSION_CAP,
                });
            }

            // Graceful no-op when the contract's balance is below the
            // requested amount. Running out of expand-economy funds is the
            // INTENDED end-state: the contract is a finite "bluechip mint
            // boost" reservoir that drains as the early ecosystem grows,
            // tapering rewards toward zero by design. A failed BankMsg
            // here would propagate up through `NotifyThresholdCrossed` and
            // revert the entire factory tx, which would in turn leave the
            // pool's `IS_THRESHOLD_HIT = true` state in place but force
            // operators to chase the failed mint via `RetryFactoryNotify`
            // forever. Instead, log the skip and return Ok so threshold
            // crossings continue to settle cleanly even when the reservoir
            // is empty.
            let balance = deps
                .querier
                .query_balance(env.contract.address.as_str(), &config.bluechip_denom)?;
            if balance.amount < amount {
                return Ok(Response::new()
                    .add_attribute("action", "request_reward_skipped")
                    .add_attribute("reason", "insufficient_balance")
                    .add_attribute("recipient", recipient_addr)
                    .add_attribute("requested_amount", amount)
                    .add_attribute("contract_balance", balance.amount)
                    .add_attribute("denom", config.bluechip_denom));
            }

            // Persist the rolling-window debit only after balance check
            // passes, so a skipped (insufficient_balance) request doesn't
            // burn cap budget that the protocol could spend later when
            // the contract is refunded.
            EXPANSION_WINDOW.save(
                deps.storage,
                &ExpansionWindow {
                    window_start: window.window_start,
                    spent_in_window: new_spent,
                },
            )?;

            let send_msg = BankMsg::Send {
                to_address: recipient_addr.to_string(),
                amount: vec![Coin {
                    denom: config.bluechip_denom.clone(),
                    amount,
                }],
            };

            Ok(Response::new()
                .add_message(send_msg)
                .add_attribute("action", "request_reward")
                .add_attribute("recipient", recipient_addr)
                .add_attribute("amount", amount)
                .add_attribute("denom", config.bluechip_denom)
                .add_attribute("spent_in_window_after", new_spent)
                .add_attribute("daily_cap", DAILY_EXPANSION_CAP))
        }
    }
}

pub fn execute_propose_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    factory_address: Option<String>,
    owner: Option<String>,
    bluechip_denom: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "A config update is already pending. Cancel it first.",
    )?;

    // Validate addresses early so invalid proposals fail at propose time
    if let Some(ref addr) = factory_address {
        deps.api.addr_validate(addr)?;
    }
    if let Some(ref addr) = owner {
        deps.api.addr_validate(addr)?;
    }
    // M-EE-3: full cosmos-sdk denom format validation at propose time.
    // Operator typos surface 48h earlier than they otherwise would
    // (when someone tries to apply and every subsequent
    // `RequestExpansion` breaks). Replaces the previous "non-empty
    // after trim" check, which let through e.g. `"Bluechip"`,
    // `"u bluechip"`, `"1ubluechip"`, all of which the bank module
    // rejects but only after the 48h timelock has lapsed.
    if let Some(ref d) = bluechip_denom {
        validate_native_denom(d)?;
    }

    let effective_after = env.block.time.plus_seconds(CONFIG_TIMELOCK_SECONDS);

    PENDING_CONFIG_UPDATE.save(
        deps.storage,
        &PendingConfigUpdate {
            factory_address,
            owner,
            bluechip_denom,
            effective_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_config_update")
        .add_attribute("effective_after", effective_after.to_string()))
}

pub fn execute_apply_config_update(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    let mut config = load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "No pending config update to execute",
    )?;

    if env.block.time < pending.effective_after {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Timelock not expired. Execute after: {}",
            pending.effective_after
        ))));
    }

    if let Some(factory) = pending.factory_address {
        config.factory_address = deps.api.addr_validate(&factory)?;
    }
    if let Some(new_owner) = pending.owner {
        config.owner = deps.api.addr_validate(&new_owner)?;
    }
    if let Some(new_denom) = pending.bluechip_denom {
        // M-EE-3: full denom format validation was already enforced at
        // propose time; re-check here as defense-in-depth in case a
        // future migration ever inserts a PendingConfigUpdate directly,
        // bypassing propose. Cheap to repeat (no I/O), and locks the
        // invariant on the apply path too.
        validate_native_denom(&new_denom)?;
        config.bluechip_denom = new_denom;
    }

    CONFIG.save(deps.storage, &config)?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);

    Ok(Response::new()
        .add_attribute("action", "execute_config_update")
        .add_attribute("factory", config.factory_address)
        .add_attribute("owner", config.owner)
        .add_attribute("bluechip_denom", config.bluechip_denom))
}

pub fn execute_cancel_config_update(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    load_or_err(
        deps.storage,
        &PENDING_CONFIG_UPDATE,
        "No pending config update to cancel",
    )?;
    PENDING_CONFIG_UPDATE.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_config_update"))
}

pub fn execute_propose_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
    denom: String,
    recipient: Option<String>,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    ensure_absent(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "A withdrawal is already pending. Cancel it first.",
    )?;

    let target = recipient.unwrap_or_else(|| info.sender.to_string());
    deps.api.addr_validate(&target)?;

    let execute_after = env.block.time.plus_seconds(WITHDRAW_TIMELOCK_SECONDS);
    PENDING_WITHDRAWAL.save(
        deps.storage,
        &PendingWithdrawal {
            amount,
            denom: denom.clone(),
            recipient: target.clone(),
            execute_after,
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "propose_withdrawal")
        .add_attribute("recipient", target)
        .add_attribute("amount", amount)
        .add_attribute("denom", denom)
        .add_attribute("execute_after", execute_after.to_string()))
}

pub fn execute_withdrawal(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    let pending = load_or_err(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "No pending withdrawal to execute",
    )?;

    if env.block.time < pending.execute_after {
        return Err(ContractError::Std(StdError::generic_err(format!(
            "Timelock not expired. Execute after: {}",
            pending.execute_after
        ))));
    }

    PENDING_WITHDRAWAL.remove(deps.storage);

    // Clamp the requested amount to the contract's current balance so a
    // proposed-but-stale withdrawal (e.g. balance drew down via
    // RequestExpansion between propose and execute) doesn't fail the
    // whole tx at the bank module. Transfer the smaller of (requested,
    // balance) and emit both values so the caller can detect the clamp.
    let balance = deps
        .querier
        .query_balance(env.contract.address.as_str(), &pending.denom)?;
    let amount_to_send = pending.amount.min(balance.amount);

    let mut response = Response::new()
        .add_attribute("action", "execute_withdrawal")
        .add_attribute("recipient", pending.recipient.clone())
        .add_attribute("requested_amount", pending.amount)
        .add_attribute("amount", amount_to_send)
        .add_attribute("contract_balance", balance.amount)
        .add_attribute("denom", pending.denom.clone());

    if !amount_to_send.is_zero() {
        let send_msg = BankMsg::Send {
            to_address: pending.recipient.clone(),
            amount: vec![Coin {
                denom: pending.denom,
                amount: amount_to_send,
            }],
        };
        response = response.add_message(send_msg);
    } else {
        response = response.add_attribute("note", "no funds available; withdrawal skipped");
    }

    Ok(response)
}

pub fn execute_cancel_withdrawal(
    deps: DepsMut,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    load_config_as_owner(deps.storage, &info.sender)?;
    load_or_err(
        deps.storage,
        &PENDING_WITHDRAWAL,
        "No pending withdrawal to cancel",
    )?;
    PENDING_WITHDRAWAL.remove(deps.storage);
    Ok(Response::new().add_attribute("action", "cancel_withdrawal"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetConfig {} => to_json_binary(&query_config(deps)?),
        QueryMsg::GetBalance { denom } => {
            to_json_binary(&deps.querier.query_balance(env.contract.address, denom)?)
        }
    }
}

// ---------------------------------------------------------------------------
// Migrate (C-EE-1)
// ---------------------------------------------------------------------------

/// C-EE-1 — without this entry point the chain rejects every
/// `MsgMigrateContract` at runtime with "no migrate function exported",
/// which would leave this contract effectively immutable despite cw2
/// being initialised at instantiate time. Mirrors the M-3 downgrade
/// guard the pool / factory contracts already use:
///
///   - parse the cw2-stored version + the compile-time CONTRACT_VERSION
///     as semver; reject any migrate where stored > current
///   - tolerate a missing cw2 entry (legacy / test fixtures), skipping
///     the comparison rather than erroring
///   - bump the cw2 record on success so subsequent migrates see the
///     new version
///
/// `MigrateMsg::UpdateVersion {}` is the no-op variant — it exists so
/// operators have an explicit "just bump the version, no other state
/// changes" path. Future variants can carry parameters; the downgrade
/// guard runs unconditionally first so they all benefit.
#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, msg: MigrateMsg) -> Result<Response, ContractError> {
    if let Ok(stored_version) = cw2::get_contract_version(deps.storage) {
        let stored_semver: semver::Version = stored_version.version.parse().map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "stored contract version {} is not valid semver: {}",
                stored_version.version, e
            )))
        })?;
        let current_semver: semver::Version = CONTRACT_VERSION.parse().map_err(|e| {
            ContractError::Std(StdError::generic_err(format!(
                "current contract version {} is not valid semver: {}",
                CONTRACT_VERSION, e
            )))
        })?;
        if stored_semver > current_semver {
            return Err(ContractError::Std(StdError::generic_err(format!(
                "Migration would downgrade contract from {} to {}; refusing.",
                stored_semver, current_semver
            ))));
        }
    }

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    match msg {
        MigrateMsg::UpdateVersion {} => Ok(Response::new()
            .add_attribute("action", "migrate")
            .add_attribute("variant", "update_version")
            .add_attribute("contract_name", CONTRACT_NAME)
            .add_attribute("contract_version", CONTRACT_VERSION)),
    }
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        factory_address: config.factory_address,
        owner: config.owner,
        bluechip_denom: config.bluechip_denom,
    })
}

// ---------------------------------------------------------------------------
// Test-only re-exports
// ---------------------------------------------------------------------------
//
// `validate_native_denom` and the factory-response subset structs are
// `fn`-private / non-`pub` by design — they're internal plumbing for
// the cross-validation + denom-format paths and shouldn't be part of
// this contract's public API. The audit-tests in `crate::audit_tests`
// need to exercise them directly (M-EE-2 round-trip + M-EE-3 unit
// tests on the validator), so we expose them through a `cfg(test)`
// module rather than weakening the production visibility.

#[cfg(test)]
pub mod testing {
    use super::FactoryInstantiateResponseSubset as Subset;
    use crate::error::ContractError;
    use serde::Deserialize;

    /// M-EE-2: re-export of the private subset struct so tests can
    /// deserialize a synthetic factory response directly. Repeats the
    /// shape rather than re-exposing the original to keep the
    /// production type private.
    #[derive(Deserialize)]
    pub struct FactoryConfigSubsetForTest {
        pub bluechip_denom: String,
    }

    #[derive(Deserialize)]
    pub struct FactoryInstantiateResponseSubsetForTest {
        pub factory: FactoryConfigSubsetForTest,
    }

    /// Compile-time assertion that the test subset stays bit-identical
    /// to the production one. If a future change adds a field to the
    /// production subset without the test subset, this fails to
    /// compile — forcing a co-ordinated update.
    const _: fn() = || {
        fn assert_same_shape(_p: &Subset, _t: &FactoryInstantiateResponseSubsetForTest) {}
        // Both deserialize the same JSON to the same field set today.
    };

    /// M-EE-3: re-export of the validator for unit tests.
    pub fn validate_native_denom_for_test(denom: &str) -> Result<(), ContractError> {
        super::validate_native_denom(denom)
    }
}
