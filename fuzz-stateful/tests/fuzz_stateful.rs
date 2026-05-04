//! Proptest entry point for the stateful fuzzer.
//!
//! Run:
//!   cargo test -p fuzz-stateful fuzz_stateful --release -- --nocapture
//!
//! Quick smoke (in CI):
//!   PROPTEST_CASES=32 cargo test -p fuzz-stateful fuzz_stateful_quick --release
//!
//! On a regression hit, proptest writes the seed to
//!   fuzz-stateful/proptest-regressions/fuzz_stateful.txt
//! Subsequent runs replay that seed first.

use proptest::prelude::*;

use fuzz_stateful::{apply, build_world, check_all, Action, OutcomeKind};

/// Deterministic smoke test: create a creator pool, commit through threshold,
/// swap, and run all invariants. Confirms the harness actually exercises the
/// contracts (no silent contract instantiation failures).
#[test]
fn smoke_create_commit_swap() {
    let mut world = build_world(false);
    // 1. Create a pool — must succeed.
    let outcome = apply(&mut world, Action::CreateCreatorPool { decimals: 6 });
    assert!(
        matches!(outcome.kind, OutcomeKind::Ok),
        "create pool failed: {} :: {:?}",
        outcome.note, outcome.action
    );
    assert_eq!(world.pools.len(), 1, "pool should be registered");

    // 2. A small commit must succeed (pre-threshold).
    let commit = apply(
        &mut world,
        Action::Commit { user_idx: 0, pool_idx: 0, amount: 1_000_000_000 },
    );
    assert!(
        matches!(commit.kind, OutcomeKind::Ok),
        "commit should succeed pre-threshold: {} :: {:?}",
        commit.note, commit.action
    );

    // 3. Cross threshold by committing larger value (rate=$1/bluechip,
    //    threshold=$25k = 25_000 bluechip = 25e9 ubluechip).
    apply(&mut world, Action::AdvanceBlock { secs: 60 });
    apply(
        &mut world,
        Action::Commit { user_idx: 1, pool_idx: 0, amount: 30_000_000_000 },
    );

    // 4. Run invariants.
    check_all(&mut world).expect("invariants should hold after smoke sequence");

    // 5. The illegal-action variants must reject (panics if they don't).
    apply(&mut world, Action::AttemptUnauthorizedConfigUpdate { attacker_idx: 0, pool_idx: 0 });
    apply(&mut world, Action::AttemptUnauthorizedThresholdNotify { attacker_idx: 0, forged_pool_id: 999 });
    apply(&mut world, Action::AttemptOraclePriceZero);
    apply(&mut world, Action::AttemptUnauthorizedEmergency { attacker_idx: 0, pool_idx: 0 });
    apply(&mut world, Action::AttemptUnauthorizedCreatorClaim { attacker_idx: 0, pool_idx: 0 });
}

/// Exercise the full emergency-withdraw lifecycle on a creator pool.
/// Confirms (a) phase 1 initiates, (b) cancel works, (c) re-initiate
/// works, (d) phase 2 drain succeeds after the 24h timelock, (e) the
/// drained pool blocks subsequent ops.
#[test]
fn smoke_emergency_withdraw_lifecycle() {
    let mut world = build_world(false);
    apply(&mut world, Action::CreateCreatorPool { decimals: 6 });
    assert_eq!(world.pools.len(), 1);

    // EmergencyWithdraw on a creator pool is gated until threshold-cross
    // (committed funds would be stranded otherwise). Push the pool past
    // threshold first.
    // Cross threshold incrementally: commit ~just-under, then a small
    // top-up that crosses with minimal excess (so the post-threshold
    // excess swap doesn't trip the 10% max_spread hard cap).
    let c1 = apply(&mut world, Action::Commit { user_idx: 0, pool_idx: 0, amount: 20_000_000_000 });
    eprintln!("c1: {:?} {}", c1.kind, c1.note);
    apply(&mut world, Action::AdvanceBlock { secs: 60 });
    // Cross with exactly the threshold amount so there is no excess
    // swap to trip max_spread.
    let c2 = apply(&mut world, Action::Commit { user_idx: 1, pool_idx: 0, amount: 5_000_000_000 });
    eprintln!("c2: {:?} {}", c2.kind, c2.note);
    apply(&mut world, Action::AdvanceBlock { secs: 60 });
    check_all(&mut world).expect("post-threshold invariants must hold");
    assert!(world.pools[0].threshold_hit_seen, "threshold should be hit before emergency");

    let init = apply(&mut world, Action::EmergencyInitiate { pool_idx: 0 });
    assert!(matches!(init.kind, OutcomeKind::Ok), "initiate failed: {}", init.note);

    let cancel = apply(&mut world, Action::EmergencyCancel { pool_idx: 0 });
    assert!(matches!(cancel.kind, OutcomeKind::Ok), "cancel failed: {}", cancel.note);

    let init2 = apply(&mut world, Action::EmergencyInitiate { pool_idx: 0 });
    assert!(matches!(init2.kind, OutcomeKind::Ok), "reinit failed: {}", init2.note);

    // Phase 2 before timelock must fail (24h required).
    let early = apply(&mut world, Action::EmergencyExecute { pool_idx: 0 });
    assert!(
        matches!(early.kind, OutcomeKind::Rejected),
        "early execute should reject: {}", early.note
    );

    // Advance 25h.
    apply(&mut world, Action::AdvanceBlock { secs: 25 * 3600 });
    let drain = apply(&mut world, Action::EmergencyExecute { pool_idx: 0 });
    assert!(matches!(drain.kind, OutcomeKind::Ok), "drain failed: {}", drain.note);
    assert!(world.pools[0].drained, "drained flag not tracked");

    // Invariants: drained pool blocks ops + state still consistent.
    check_all(&mut world).expect("post-drain invariants must hold");
}

const SEQUENCE_LEN_DEFAULT: usize = 30;

fn run_sequence(actions: Vec<Action>) -> Result<(), TestCaseError> {
    let mut world = build_world(true);
    let mut log: Vec<String> = Vec::with_capacity(actions.len());

    for (step, action) in actions.into_iter().enumerate() {
        let outcome = apply(&mut world, action);
        log.push(format!(
            "step {step:03}: [{:?}] {} -- {:?}",
            outcome.kind, outcome.note, outcome.action
        ));

        if let Err(v) = check_all(&mut world) {
            let trace = log.join("\n");
            return Err(TestCaseError::fail(format!(
                "INVARIANT VIOLATED after step {step}: {} :: {}\n\
                 ----- action sequence (paste into a regression test) -----\n{}\n\
                 ----- end -----",
                v.name, v.detail, trace
            )));
        }
    }
    if std::env::var("FUZZ_DEBUG").is_ok() {
        let mut ok = 0; let mut rej = 0; let mut exp = 0;
        for line in &log {
            if line.contains("[Ok]") { ok += 1; }
            else if line.contains("[Rejected]") { rej += 1; }
            else if line.contains("[ExpectedErr]") { exp += 1; }
        }
        eprintln!(
            "[fuzz] sequence done: {} steps, ok={}, rejected={}, expected_err={}",
            log.len(), ok, rej, exp
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 8192,
        result_cache: prop::test_runner::basic_result_cache,
        .. ProptestConfig::default()
    })]

    /// Main stateful fuzz: 30-action sequences.
    #[test]
    fn fuzz_stateful(actions in prop::collection::vec(any::<Action>(), 5..=SEQUENCE_LEN_DEFAULT)) {
        run_sequence(actions)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        max_shrink_iters: 1024,
        .. ProptestConfig::default()
    })]

    /// CI-friendly quick smoke (32 short sequences). Used by fuzz.sh.
    #[test]
    fn fuzz_stateful_quick(actions in prop::collection::vec(any::<Action>(), 5..=15)) {
        run_sequence(actions)?;
    }
}
