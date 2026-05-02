//! Multi-hop execution logic.
//!
//! ## Execution model
//!
//! The router uses the standard CosmWasm self-recursion pattern. The
//! public [`execute_multi_hop`] (or [`execute_receive_cw20`]) entry
//! point validates the route, captures the recipient's pre-route balance
//! of the final ask token, and then builds a `Response` whose messages
//! are a sequence of self-calls:
//!
//! 1. One [`crate::msg::ExecuteMsg::ExecuteSwapOperation`] per hop, in
//!    order. Hops 0..N-1 send their output back to the router; hop N
//!    sends its output directly to the recipient. Each is wrapped in a
//!    `SubMsg::reply_on_error` so the [`handle_reply`] handler can
//!    re-raise raw pool errors as [`crate::error::RouterError::HopFailed`]
//!    with hop context preserved through the submsg payload.
//!
//! 2. One trailing [`crate::msg::ExecuteMsg::AssertReceived`] self-call
//!    that compares the recipient's post-route balance to the captured
//!    pre-route balance and rejects if the delta is below
//!    `minimum_receive`.
//!
//! Atomicity comes for free: every message in a `Response` runs in
//! sequence within a single transaction; any error reverts everything.
//!
//! ## Why per-hop balance reads are safe
//!
//! Each [`execute_swap_operation`] call uses the router's *current*
//! balance of the offer token as the swap input. This works because the
//! router holds zero balance between transactions (the entry-point
//! deposit is the only credit on hop 0, and every subsequent hop is
//! credited only by the previous hop). Operators must not send tokens
//! directly to the router contract; doing so could cause an unrelated
//! deposit to get swept into the next user's route.

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    from_json, to_json_binary, Addr, Binary, Coin, CosmosMsg, DepsMut, Env, MessageInfo, Reply,
    ReplyOn, Response, StdError, SubMsg, SubMsgResult, Timestamp, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use pool_factory_interfaces::asset::{TokenInfo, TokenType};
use pool_factory_interfaces::routing::{PoolSwapCw20HookMsg, PoolSwapExecuteMsg, SwapOperation};

use crate::error::RouterError;
use crate::msg::{Cw20HookMsg, ExecuteMsg};
use crate::state::MAX_HOPS;

/// Reply IDs are offset by this base so that future router features can
/// claim a different range without colliding with hop replies.
pub const REPLY_ID_HOP_BASE: u64 = 1000;

/// Carried in each hop's submessage payload so that [`handle_reply`] can
/// produce a [`RouterError::HopFailed`] with the failing pool address
/// even though the reply handler does not see the original operation.
#[cw_serde]
struct HopReplyPayload {
    hop_index: u32,
    pool_addr: String,
}

/// Public entry: native bluechip offer for the first hop.
///
/// The caller must attach exactly one coin matching the first hop's
/// declared offer denom; that coin's amount is used as the route input.
pub fn execute_multi_hop(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    operations: Vec<SwapOperation>,
    minimum_receive: Uint128,
    deadline: Option<Timestamp>,
    recipient: Option<String>,
) -> Result<Response, RouterError> {
    let first_op = operations.first().ok_or(RouterError::EmptyRoute)?;
    let offer_amount = match &first_op.offer_asset_info {
        TokenType::Native { denom } => extract_native_offer(&info, denom)?,
        TokenType::CreatorToken { .. } => {
            return Err(RouterError::Std(StdError::generic_err(
                "ExecuteMultiHop is for native offers; use cw20::Send for CW20-offered routes",
            )));
        }
    };

    start_multi_hop(
        deps,
        env,
        info.sender,
        offer_amount,
        operations,
        minimum_receive,
        deadline,
        recipient,
    )
}

/// Public entry: CW20 offer for the first hop, dispatched via
/// `cw20::Send`. The CW20 contract has already transferred the offer
/// amount to the router by the time this handler runs.
pub fn execute_receive_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> Result<Response, RouterError> {
    let hook: Cw20HookMsg = from_json(&cw20_msg.msg)?;
    match hook {
        Cw20HookMsg::ExecuteMultiHop {
            operations,
            minimum_receive,
            deadline,
            recipient,
        } => {
            let first_op = operations.first().ok_or(RouterError::EmptyRoute)?;
            match &first_op.offer_asset_info {
                TokenType::CreatorToken { contract_addr } => {
                    if *contract_addr != info.sender {
                        return Err(RouterError::Std(StdError::generic_err(format!(
                            "first hop offer cw20 ({}) does not match sender ({})",
                            contract_addr, info.sender
                        ))));
                    }
                }
                TokenType::Native { .. } => {
                    return Err(RouterError::Std(StdError::generic_err(
                        "first hop is native; do not call cw20::Send for native offers",
                    )));
                }
            }
            let user = deps.api.addr_validate(&cw20_msg.sender)?;
            start_multi_hop(
                deps,
                env,
                user,
                cw20_msg.amount,
                operations,
                minimum_receive,
                deadline,
                recipient,
            )
        }
    }
}

/// Shared route setup. Validates the route, captures the recipient's
/// pre-route balance of the final ask token, and builds the per-hop
/// self-call sequence plus the final assertion call.
fn start_multi_hop(
    deps: DepsMut,
    env: Env,
    sender: Addr,
    offer_amount: Uint128,
    operations: Vec<SwapOperation>,
    minimum_receive: Uint128,
    deadline: Option<Timestamp>,
    recipient: Option<String>,
) -> Result<Response, RouterError> {
    if offer_amount.is_zero() {
        return Err(RouterError::ZeroAmount);
    }
    if let Some(d) = deadline {
        if env.block.time > d {
            return Err(RouterError::DeadlineExceeded {
                deadline: d.seconds(),
                current: env.block.time.seconds(),
            });
        }
    }
    validate_route(&operations)?;

    let recipient_addr = match recipient {
        Some(r) => deps.api.addr_validate(&r)?,
        None => sender.clone(),
    };

    let final_ask = operations.last().unwrap().ask_asset_info.clone();
    // Strict query (audit fix R1): on a CW20 final-ask, a swallowed
    // pre-balance query would silently report zero and let the recipient's
    // pre-existing CW20 holdings count toward the post-route "received"
    // total — eroding slippage protection by up to that amount. The strict
    // variant propagates query errors so a failed CW20 read fails the
    // entire route closed instead of corrupting the assertion.
    let recipient_initial_balance =
        final_ask.query_pool_strict(&deps.querier, recipient_addr.clone())?;

    let last_idx = operations.len() - 1;
    let mut messages: Vec<SubMsg> = Vec::with_capacity(operations.len() + 1);

    for (idx, op) in operations.iter().enumerate() {
        let to = if idx == last_idx {
            recipient_addr.to_string()
        } else {
            env.contract.address.to_string()
        };
        let exec_op = ExecuteMsg::ExecuteSwapOperation {
            operation: op.clone(),
            hop_index: idx as u32,
            to,
        };
        let payload: Binary = to_json_binary(&HopReplyPayload {
            hop_index: idx as u32,
            pool_addr: op.pool_addr.clone(),
        })?;
        let sub = SubMsg::reply_on_error(
            CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: env.contract.address.to_string(),
                msg: to_json_binary(&exec_op)?,
                funds: vec![],
            }),
            hop_reply_id(idx as u32),
        )
        .with_payload(payload);
        messages.push(sub);
    }

    let assert_msg = ExecuteMsg::AssertReceived {
        ask_info: final_ask,
        recipient: recipient_addr.to_string(),
        prev_balance: recipient_initial_balance,
        minimum_receive,
    };
    messages.push(SubMsg {
        id: 0,
        payload: Binary::default(),
        msg: CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_json_binary(&assert_msg)?,
            funds: vec![],
        }),
        gas_limit: None,
        reply_on: ReplyOn::Never,
    });

    Ok(Response::new()
        .add_submessages(messages)
        .add_attribute("action", "execute_multi_hop")
        .add_attribute("sender", sender)
        .add_attribute("recipient", recipient_addr)
        .add_attribute("offer_amount", offer_amount)
        .add_attribute("hops", operations.len().to_string())
        .add_attribute("minimum_receive", minimum_receive))
}

/// Internal handler for one hop. Self-only.
///
/// Reads the router's current balance of the offer token (which equals
/// either the user's deposit on hop 0 or the previous hop's output on
/// hops 1..N), then dispatches the underlying pool swap targeting `to`.
///
/// The underlying pool message is built with `belief_price = None` and
/// `max_spread = None` unconditionally — see the module-level
/// `ExecuteMsg` doc-comment in `msg.rs` for why per-hop slippage knobs
/// are not exposed at the multi-hop level (`minimum_receive` is the
/// canonical end-to-end gate).
pub fn execute_swap_operation(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    operation: SwapOperation,
    hop_index: u32,
    to: String,
) -> Result<Response, RouterError> {
    if info.sender != env.contract.address {
        return Err(RouterError::Unauthorized);
    }
    let to_addr = deps.api.addr_validate(&to)?;

    // Strict query (audit fix R1). The router's own balance is what
    // becomes the swap input for this hop; if a CW20 balance query
    // silently returns zero on error we'd dispatch a zero-amount swap
    // and the explicit zero-check below would mask the underlying
    // query failure. Strict propagation surfaces the real cause.
    let offer_balance = operation
        .offer_asset_info
        .query_pool_strict(&deps.querier, env.contract.address.clone())?;
    if offer_balance.is_zero() {
        return Err(RouterError::HopFailed {
            hop_index: hop_index as usize,
            pool_addr: operation.pool_addr.clone(),
            reason: "router holds zero balance of the offer token at hop start".to_string(),
        });
    }

    let pool_msg = build_pool_swap_msg(&operation, offer_balance, to_addr.to_string())?;

    Ok(Response::new()
        .add_message(pool_msg)
        .add_attribute("action", "execute_swap_operation")
        .add_attribute("hop_index", hop_index.to_string())
        .add_attribute("pool", operation.pool_addr)
        .add_attribute("offer_amount", offer_balance)
        .add_attribute("to", to_addr))
}

/// Internal handler for the final slippage check. Self-only.
pub fn execute_assert_received(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    ask_info: TokenType,
    recipient: String,
    prev_balance: Uint128,
    minimum_receive: Uint128,
) -> Result<Response, RouterError> {
    if info.sender != env.contract.address {
        return Err(RouterError::Unauthorized);
    }
    let recipient_addr = deps.api.addr_validate(&recipient)?;
    // Strict query (audit fix R1). Symmetric with the pre-route read in
    // `start_multi_hop` — both must use the same strict variant so a
    // CW20 query failure fails the assertion closed.
    let current_balance =
        ask_info.query_pool_strict(&deps.querier, recipient_addr.clone())?;
    let received = current_balance.checked_sub(prev_balance).map_err(|_| {
        RouterError::Std(StdError::generic_err(
            "recipient balance decreased during route; impossible state",
        ))
    })?;
    if received < minimum_receive {
        return Err(RouterError::SlippageExceeded {
            minimum: minimum_receive,
            actual: received,
        });
    }
    Ok(Response::new()
        .add_attribute("action", "assert_received")
        .add_attribute("recipient", recipient_addr)
        .add_attribute("received", received)
        .add_attribute("minimum_receive", minimum_receive))
}

/// Reply handler for hop submessages. Wraps the raw pool error into a
/// [`RouterError::HopFailed`] with hop index and pool address.
///
/// The pool address is read from the submsg payload when available. Some
/// host runtimes (notably `cw-multi-test` 2.1) do not propagate the
/// payload through to replies, so the handler tolerates an empty or
/// unparseable payload by reporting an empty `pool_addr` instead of
/// failing the wrapping. The hop index is recovered from the reply ID
/// in either case so frontends always learn which hop failed.
pub fn handle_reply(_deps: DepsMut, _env: Env, msg: Reply) -> Result<Response, RouterError> {
    let hop_index = parse_hop_reply_id(msg.id).ok_or_else(|| {
        RouterError::Std(StdError::generic_err(format!(
            "unknown reply id: {}",
            msg.id
        )))
    })?;
    let pool_addr = if msg.payload.is_empty() {
        String::new()
    } else {
        from_json::<HopReplyPayload>(&msg.payload)
            .map(|p| p.pool_addr)
            .unwrap_or_default()
    };
    let reason = match msg.result {
        SubMsgResult::Err(err) => err,
        // ReplyOn::Error never fires on success; treat as a no-op so
        // the contract does not panic if a future runtime change alters
        // delivery semantics.
        SubMsgResult::Ok(_) => return Ok(Response::new()),
    };
    Err(RouterError::HopFailed {
        hop_index: hop_index as usize,
        pool_addr,
        reason,
    })
}

/// Validates a candidate route in isolation -- no chain state is read.
///
/// Performs every cheap check before any pool query so callers get fast
/// rejection of obviously malformed routes.
pub fn validate_route(operations: &[SwapOperation]) -> Result<(), RouterError> {
    if operations.is_empty() {
        return Err(RouterError::EmptyRoute);
    }
    if operations.len() > MAX_HOPS {
        return Err(RouterError::MaxHopsExceeded {
            max: MAX_HOPS,
            got: operations.len(),
        });
    }

    for (idx, op) in operations.iter().enumerate() {
        if op.offer_asset_info.equal(&op.ask_asset_info) {
            return Err(RouterError::Std(StdError::generic_err(format!(
                "hop {} declares offer and ask as the same token: {}",
                idx, op.offer_asset_info
            ))));
        }
    }

    for i in 0..operations.len() - 1 {
        let cur_ask = &operations[i].ask_asset_info;
        let next_offer = &operations[i + 1].offer_asset_info;
        if !cur_ask.equal(next_offer) {
            return Err(RouterError::RouteDiscontinuity {
                hop_index: i,
                next_hop_index: i + 1,
                transition: format!("{} -> {}", cur_ask, next_offer),
            });
        }
    }

    let input = &operations.first().unwrap().offer_asset_info;
    let output = &operations.last().unwrap().ask_asset_info;
    if input.equal(output) {
        return Err(RouterError::SameInputOutput);
    }
    Ok(())
}

/// Builds the underlying pool swap message for one hop. Per-hop
/// slippage gates (`belief_price`, `max_spread`) are intentionally
/// pinned to `None` here — see the module / `ExecuteMsg` doc-comment
/// for why they cannot be made meaningful across heterogeneous
/// multi-hop pairs. End-to-end slippage is enforced via
/// `minimum_receive` in `execute_assert_received`.
fn build_pool_swap_msg(
    operation: &SwapOperation,
    offer_amount: Uint128,
    to: String,
) -> Result<CosmosMsg, RouterError> {
    match &operation.offer_asset_info {
        TokenType::Native { denom } => {
            let exec = PoolSwapExecuteMsg::SimpleSwap {
                offer_asset: TokenInfo {
                    info: operation.offer_asset_info.clone(),
                    amount: offer_amount,
                },
                belief_price: None,
                max_spread: None,
                to: Some(to),
                transaction_deadline: None,
            };
            Ok(CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: operation.pool_addr.clone(),
                msg: to_json_binary(&exec)?,
                funds: vec![Coin {
                    denom: denom.clone(),
                    amount: offer_amount,
                }],
            }))
        }
        TokenType::CreatorToken { contract_addr } => {
            let hook = PoolSwapCw20HookMsg::Swap {
                belief_price: None,
                max_spread: None,
                to: Some(to),
                transaction_deadline: None,
            };
            let send = Cw20ExecuteMsg::Send {
                contract: operation.pool_addr.clone(),
                amount: offer_amount,
                msg: to_json_binary(&hook)?,
            };
            Ok(CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: contract_addr.to_string(),
                msg: to_json_binary(&send)?,
                funds: vec![],
            }))
        }
    }
}

/// Extracts the offer amount from `info.funds` for a native first hop.
/// Requires exactly one coin and that its denom matches the declared
/// first-hop offer denom.
fn extract_native_offer(info: &MessageInfo, denom: &str) -> Result<Uint128, RouterError> {
    if info.funds.len() != 1 {
        return Err(RouterError::Std(StdError::generic_err(
            "ExecuteMultiHop requires exactly one funds coin matching the first hop offer denom",
        )));
    }
    let coin = &info.funds[0];
    if coin.denom != denom {
        return Err(RouterError::Std(StdError::generic_err(format!(
            "funds denom {} does not match first hop offer denom {}",
            coin.denom, denom
        ))));
    }
    Ok(coin.amount)
}

pub fn hop_reply_id(hop_index: u32) -> u64 {
    REPLY_ID_HOP_BASE + hop_index as u64
}

pub fn parse_hop_reply_id(id: u64) -> Option<u32> {
    if id >= REPLY_ID_HOP_BASE && id < REPLY_ID_HOP_BASE + MAX_HOPS as u64 {
        Some((id - REPLY_ID_HOP_BASE) as u32)
    } else {
        None
    }
}
