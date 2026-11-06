//! Head-of-line blocking in `SchedulerEngine::pump`.
//!
//! Measured defect (2026-10-19): with scheduling enabled, the Aether server stops completing
//! requests once the number of live keep-alive connections exceeds `memory_budget_bytes /
//! conn_mem`. At the freeze every handler slot is empty (`active_handlers = 0`, `slots = 32`) and
//! the `Notify` has no parked waiters (`parkedOnNotify = 0`, one wakeup per completed handler), so
//! the stall is not a wake-up storm or lock contention. It comes from the admission walk stopping
//! at the first job that does not fit: one job that *can never* fit under current usage blocks
//! every job behind it that *could* run.
//!
//! These tests are written against the required behaviour, not the current behaviour. The design
//! rule they encode:
//!
//!   * a queued job that does not fit is skipped, and later jobs that fit are admitted;
//!   * skipping is bounded, so the walk cannot degrade into a full queue scan per release, and the
//!     bound rotates so a skipped job cannot hide the rest of the queue forever;
//!   * a job that has waited past `starvation_guard` leads every walk, so the first release large
//!     enough for it is spent on it before any later arrival is offered that capacity;
//!   * when nothing fits, the walk makes no state change at all (backpressure, not spinning);
//!   * arrival precedence is untouched whenever everything fits.

use std::time::Duration;

use aether_scheduler::{
    JobState, NewJob, PolicyKind, ResourceConfig, SchedulerEngine, SchedulingConfig,
};

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

/// The exact production sizes from `app.rs:992-994` (connection) and `app.rs:1304` (handler),
/// with a budget small enough to make the case a unit test instead of a 20-second stall run.
const CONN_MEM: u64 = 8 * 1024 + 2 * 1024 * 1024 + 12 * 1024; // 2_117_632
const HAND_MEM: u64 = 16 * 1024;

fn engine(cfg: SchedulingConfig, budget: u64, handler_slots: usize) -> SchedulerEngine {
    let rcfg = ResourceConfig {
        memory_budget_bytes: budget,
        max_active_handlers: Some(handler_slots),
        min_slots: 1,
        ..ResourceConfig::default()
    };
    SchedulerEngine::new(cfg, rcfg)
}

fn tree() -> SchedulingConfig {
    SchedulingConfig {
        policy: PolicyKind::AdaptiveTree,
        ..SchedulingConfig::default()
    }
}

fn state(e: &SchedulerEngine, id: aether_scheduler::JobId) -> JobState {
    e.job_state(id).unwrap_or(JobState::Completed)
}

/// T1 — the production shape, at unit scale. Budget 3 MiB admits exactly one 2.02 MiB connection;
/// the second connection can never fit while the first is live, and a 16 KiB handler queued behind
/// it must still run. Today the handler is blocked by a job that has nothing to do with it.
#[test]
fn handler_behind_unfittable_connection_is_admitted() {
    let mut e = engine(tree(), 3_000_000, 32);
    let c1 = e.admit(NewJob::connection(CONN_MEM), ms(0)).unwrap();
    let c2 = e.admit(NewJob::connection(CONN_MEM), ms(0)).unwrap();
    let h = e.admit(NewJob::handler(HAND_MEM), ms(0)).unwrap();
    e.pump(ms(0));

    assert_eq!(
        state(&e, c1),
        JobState::Active,
        "first connection fits the budget"
    );
    assert_eq!(
        state(&e, c2),
        JobState::Queued,
        "second connection cannot fit while the first lives"
    );
    assert_eq!(
        state(&e, h),
        JobState::Active,
        "a 16 KiB handler must not be blocked by a 2 MiB connection ahead of it: the walk stopped \
         at the unfit head instead of skipping it"
    );
    assert_eq!(e.resources().active_handlers, 1);
}

/// T2 — several unfittable jobs interleaved with fitting ones: every fitting job runs.
#[test]
fn interleaved_unfittable_and_fitting_jobs_all_progress() {
    let mut e = engine(tree(), 1_000, 8);
    let hog = e.admit(NewJob::handler(700), ms(0)).unwrap();
    e.pump(ms(0));
    assert_eq!(state(&e, hog), JobState::Active);

    // 300 bytes left. Big jobs (400) can never fit; small jobs (100) can.
    let big1 = e.admit(NewJob::handler(400), ms(0)).unwrap();
    let s1 = e.admit(NewJob::handler(100), ms(0)).unwrap();
    let big2 = e.admit(NewJob::handler(400), ms(0)).unwrap();
    let s2 = e.admit(NewJob::handler(100), ms(0)).unwrap();
    e.pump(ms(0));

    assert_eq!(state(&e, big1), JobState::Queued);
    assert_eq!(state(&e, big2), JobState::Queued);
    assert_eq!(
        state(&e, s1),
        JobState::Active,
        "small job behind a big unfit job must run"
    );
    assert_eq!(
        state(&e, s2),
        JobState::Active,
        "both small jobs must run, not just the first"
    );
    assert_eq!(e.resources().active_handlers, 3, "hog + s1 + s2");
}

/// T3 — nothing fits: the walk must be pure backpressure, changing nothing. No admissions, no
/// grants, queue and usage untouched, and repeating it must stay a no-op (the layer parks on the
/// Notify and on the deadline; the engine must not invent progress or churn state).
#[test]
fn pump_with_nothing_admissible_changes_nothing() {
    let mut e = engine(tree(), 1_000, 8);
    let hog = e.admit(NewJob::handler(700), ms(0)).unwrap();
    e.pump(ms(0));
    let blocked = e.admit(NewJob::handler(400), ms(0)).unwrap();
    e.pump(ms(1));

    let before = e.resources();
    let q_before = e.queued_len();
    let out2 = e.pump(ms(2));
    let after = e.resources();

    assert!(
        out2.admitted.is_empty(),
        "no job may be admitted when none fits"
    );
    assert!(
        out2.grants.is_empty(),
        "no divisible rotation is invented while blocked"
    );
    assert_eq!(e.queued_len(), q_before);
    assert_eq!(state(&e, blocked), JobState::Queued);
    assert_eq!(state(&e, hog), JobState::Active);
    assert_eq!(
        after.registered_memory_bytes,
        before.registered_memory_bytes
    );
    assert_eq!(after.active_handlers, before.active_handlers);
}

/// T4 — the guarantee a naive `break -> continue` must not lose: a job that has waited past
/// `starvation_guard` is served **first** out of any capacity that frees.
///
/// Deliberate design change from the pre-fix gate: an over-guard, still-unfittable job no longer
/// HALTS the walk. Halting was the defect — one 2 MiB connection that can never fit under current
/// usage froze every 16 KiB handler behind it (measured: `active_handlers = 0`, `slots = 32`, no
/// parked waiters) until the 5 s queue deadline killed them. Fairness is now carried by ordering
/// instead of veto: the protected job leads every pump, so the first release large enough for it is
/// spent on it before anyone else is offered that capacity.
///
/// The residual risk this accepts: while the protected job cannot fit, newcomers *can* still be
/// admitted into the residue, so a release smaller than the protected job's need can be eaten before
/// a big-enough release arrives. In the production shape that cannot postpone a queued connection
/// indefinitely, because a connection's need is met exactly by a connection's release (2_117_632 B)
/// and the protected job leads the pump that sees it. A same-size-fragment workload could postpone
/// it, which is the argument for shedding unfittable connections (next item) rather than parking them.
#[test]
fn starved_job_is_served_first_out_of_freed_capacity() {
    let cfg = SchedulingConfig {
        starvation_guard: ms(100),
        ..tree()
    };
    let mut e = engine(cfg, 1_000, 8);
    let holder = e.admit(NewJob::handler(400), ms(0)).unwrap();
    e.pump(ms(0));
    let big = e.admit(NewJob::handler(650), ms(0)).unwrap();
    e.pump(ms(0));
    assert_eq!(
        state(&e, big),
        JobState::Queued,
        "650 does not fit in the 600 left after 400"
    );

    // A later arrival that fits in the residue right now: it is admitted, and the freed capacity
    // still cannot serve `big`, so `big` is offered it before this newcomer ever is.
    let young = e.admit(NewJob::handler(200), ms(140)).unwrap();
    e.pump(ms(150));
    assert_eq!(state(&e, big), JobState::Queued, "650 still needs 600");
    assert_eq!(
        state(&e, young),
        JobState::Active,
        "a fitting newcomer is not blocked by it"
    );

    // Holder releases: room for exactly one of {big, young-follow-up}. The protected job leads.
    assert!(e.complete(holder, ms(160)));
    let out = e.pump(ms(160));
    assert_eq!(
        out.admitted,
        vec![big],
        "freed capacity goes to the starved job first"
    );
    assert!(e.metrics().starvation_promotions >= 1);
    assert_eq!(state(&e, young), JobState::Active);

    // And the queue drains in arrival order once the protected job is done with.
    assert!(e.complete(big, ms(170)));
    assert!(e.complete(young, ms(170)));
    let late = e.admit(NewJob::handler(500), ms(180)).unwrap();
    e.pump(ms(180));
    assert_eq!(
        state(&e, late),
        JobState::Active,
        "capacity is no longer reserved by the guard"
    );
}

/// T4b — the no-veto rule has a floor: skipping must never push a protected job behind the *end* of
/// the queue. Its lead is by arrival-within-protected-group, and the bounded window rotates back to
/// it, so it is re-offered capacity every revolution.
#[test]
fn starved_job_leads_the_order() {
    let cfg = SchedulingConfig {
        starvation_guard: ms(100),
        ..tree()
    };
    let mut e = engine(cfg, 1_000, 8);
    let hog = e.admit(NewJob::handler(900), ms(0)).unwrap();
    e.pump(ms(0));
    let big = e.admit(NewJob::handler(700), ms(10)).unwrap();
    let second = e.admit(NewJob::handler(400), ms(20)).unwrap();
    assert!(e.complete(hog, ms(150)));
    // Both waited past the guard, so precedence inside the protected group is still arrival order:
    // 700 takes the freed budget first and leaves 300, which is not enough for 400.
    let out = e.pump(ms(150));
    assert_eq!(
        out.admitted,
        vec![big],
        "protected jobs lead, oldest protected first"
    );
    assert_eq!(state(&e, second), JobState::Queued);
}

/// T5 — precedence regression guard: when everything fits, admission order is still arrival order,
/// for every policy. The skip rule must not turn into a "greedy smallest job" scheduler.
#[test]
fn arrival_precedence_survives_when_everything_fits() {
    for policy in [
        PolicyKind::Fifo,
        PolicyKind::RoundRobin,
        PolicyKind::Deficit,
        PolicyKind::AdaptiveTree,
    ] {
        let mut e = engine(
            SchedulingConfig {
                policy,
                ..SchedulingConfig::default()
            },
            1_000_000,
            4,
        );
        let ids: Vec<_> = (0..4)
            .map(|i| e.admit(NewJob::handler(10 * i), ms(0)).unwrap())
            .collect();
        let out = e.pump(ms(1));
        assert_eq!(
            out.admitted, ids,
            "{policy:?} must admit in arrival order when all fit"
        );
    }
}

/// T6 — bounded skip: a queue full of unfittable jobs must not make every later admission (and
/// every release) pay for a scan of the whole queue. `pump` is called on each release, so an
/// unbounded skip walk is O(queue) policy work per completed request. The bound is only legal
/// because the window *rotates*: a job behind the window must be reached within one revolution.
#[test]
fn skip_walk_is_bounded_and_rotates() {
    use aether_scheduler::ADMISSION_SCAN_WINDOW as WINDOW;
    let mut e = engine(tree(), 1_000, 8);
    let hog = e.admit(NewJob::handler(700), ms(0)).unwrap();
    e.pump(ms(0));
    for _ in 0..64 {
        e.admit(NewJob::handler(400), ms(0)).unwrap();
    }
    // One fitting job sits at the tail, behind 64 that cannot fit.
    let tail = e.admit(NewJob::handler(100), ms(0)).unwrap();
    assert_eq!(state(&e, hog), JobState::Active);

    // 65 queued / WINDOW examined per pump => one revolution completes in ceil(65/WINDOW) pumps.
    // Declared up front: the tail MUST be reached by then, and never earlier than the window allows.
    let revolution = 65usize.div_ceil(WINDOW);
    let mut pumps = 0;
    while state(&e, tail) != JobState::Active {
        pumps += 1;
        assert!(
            pumps <= revolution,
            "tail not reached within one revolution of the window"
        );
        e.pump(ms(1));
    }
    assert_eq!(pumps, revolution, "the tail sits one full revolution away");
    assert_eq!(
        e.metrics().scan_window_halts,
        pumps as u64,
        "every walk stopped at the window bound"
    );
    assert!(
        e.metrics().skipped_unfit >= 64,
        "every unfittable job was passed over, not blocked on"
    );
    assert_eq!(e.resources().registered_memory_bytes, 800, "hog + tail");
    assert!(
        e.resources().registered_memory_bytes <= 1_000,
        "the budget must never be oversubscribed by a skip walk"
    );
    assert_eq!(
        e.metrics().admitted,
        2,
        "only the hog and the tail ever fit"
    );
}

/// T6b — the diagnostic path the fix must expose: a queued job reports *why* it is waiting, and the
/// engine reports the memory actually available, without either call changing a decision.
#[test]
fn blocked_reason_is_reported_without_changing_state() {
    use aether_scheduler::BlockedReason;
    let mut e = engine(tree(), 1_000, 1);
    let hog = e.admit(NewJob::handler(700), ms(0)).unwrap();
    e.pump(ms(0));
    let big = e.admit(NewJob::handler(900), ms(0)).unwrap();
    let slot_starved = e.admit(NewJob::handler(10), ms(0)).unwrap();
    e.pump(ms(1));

    assert_eq!(e.memory_available(), 300, "1000 budget minus the hog");
    assert_eq!(
        e.blocked_reason(big),
        Some(BlockedReason::MemoryBudget),
        "900 > 300 available"
    );
    assert_eq!(
        e.blocked_reason(slot_starved),
        Some(BlockedReason::SlotsFull),
        "1 slot, hog holds it"
    );
    assert_eq!(e.blocked_reason(hog), None, "an active job is not blocked");

    let before = (e.resources(), e.queued_len(), e.metrics().clone());
    assert_eq!(e.blocked_reason(big), Some(BlockedReason::MemoryBudget));
    let after = (e.resources(), e.queued_len(), e.metrics().clone());
    assert_eq!(before.0, after.0, "a diagnostic must not mutate resources");
    assert_eq!(before.1, after.1, "a diagnostic must not mutate the queue");
    assert_eq!(before.2, after.2, "a diagnostic must not mutate metrics");
}
