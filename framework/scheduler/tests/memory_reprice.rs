//! Re-pricing a live job's memory registration (`SchedulerEngine::adjust_memory`).
//!
//! A connection is admitted for its overhead and re-priced as the bytes it buffers grow
//! or drain. Four properties must hold, and each one is a distinct way the server could
//! break if the primitive were wrong:
//!
//!   * a growth the budget cannot cover is refused and leaves every number untouched
//!     (a partially applied growth would silently shrink the budget for everyone else);
//!   * a shrink makes the freed bytes available to queued work on the next walk;
//!   * only a *running* job can be re-priced — a queued or vanished job is refused;
//!   * release gives back the re-priced amount, not the original one. Getting this wrong
//!     leaks capacity permanently: the budget drains toward zero and the server stops
//!     admitting connections with nothing live.

use std::time::Duration;

use aether_scheduler::{
    JobState, NewJob, PolicyKind, ResourceConfig, SchedulerEngine, SchedulingConfig,
};

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn engine(budget: u64, handler_slots: usize) -> SchedulerEngine {
    let rcfg = ResourceConfig {
        memory_budget_bytes: budget,
        max_active_handlers: Some(handler_slots),
        min_slots: 1,
        ..ResourceConfig::default()
    };
    SchedulerEngine::new(
        SchedulingConfig {
            policy: PolicyKind::AdaptiveTree,
            ..SchedulingConfig::default()
        },
        rcfg,
    )
}

fn activate(e: &mut SchedulerEngine, id: aether_scheduler::JobId) {
    e.pump(ms(0));
    assert_eq!(
        e.job_state(id),
        Some(JobState::Active),
        "job must be running to be re-priced"
    );
}

#[test]
fn growth_beyond_the_budget_is_refused_and_changes_nothing() {
    let mut e = engine(2_000, 4);
    let c = e.admit(NewJob::connection(1_000), ms(0)).unwrap();
    activate(&mut e, c);

    let before = e.resources();
    let metrics_before = e.metrics().clone();
    assert!(
        !e.adjust_memory(c, 1_500),
        "a growth the budget cannot cover must be refused"
    );

    let after = e.resources();
    assert_eq!(
        after.registered_memory_bytes, before.registered_memory_bytes,
        "a refused growth still charged"
    );
    assert_eq!(
        e.metrics().memory_shed,
        metrics_before.memory_shed + 1,
        "the shed must be counted"
    );
    assert_eq!(
        e.job_state(c),
        Some(JobState::Active),
        "a refused growth must not disturb the job"
    );
}

#[test]
fn shrinking_returns_bytes_to_work_that_was_blocked_on_them() {
    let mut e = engine(2_000, 4);
    let c = e.admit(NewJob::connection(1_000), ms(0)).unwrap();
    activate(&mut e, c);
    // Queued behind it: 1_500 does not fit while the connection holds 1_000 of 2_000.
    let h = e.admit(NewJob::handler(1_500), ms(0)).unwrap();
    e.pump(ms(0));
    assert_eq!(
        e.job_state(h),
        Some(JobState::Queued),
        "the test needs the handler blocked on memory"
    );

    assert!(
        e.adjust_memory(c, -700),
        "giving bytes back is always allowed"
    );
    e.pump(ms(0));
    assert_eq!(
        e.job_state(h),
        Some(JobState::Active),
        "the freed bytes must be offered to the queue"
    );
    assert_eq!(
        e.resources().registered_memory_bytes,
        300 + 1_500,
        "registration must be the re-priced total"
    );
}

#[test]
fn only_a_running_job_can_be_repriced() {
    let mut e = engine(2_000, 4);
    let c = e.admit(NewJob::connection(1_000), ms(0)).unwrap();
    activate(&mut e, c);
    let queued = e.admit(NewJob::handler(1_900), ms(0)).unwrap();
    e.pump(ms(0));
    assert_eq!(e.job_state(queued), Some(JobState::Queued));

    let before = e.resources();
    let metrics_before = e.metrics().clone();
    assert!(
        !e.adjust_memory(queued, 100),
        "a queued job holds nothing to re-price"
    );
    assert!(
        !e.adjust_memory(aether_scheduler::JobId(9_999), 100),
        "an unknown job is refused"
    );
    assert_eq!(
        e.resources().registered_memory_bytes,
        before.registered_memory_bytes
    );
    assert_eq!(
        e.metrics().memory_shed,
        metrics_before.memory_shed,
        "refusing a non-live job is not a shed"
    );
}

#[test]
fn release_returns_the_repriced_amount_not_the_original_one() {
    let mut e = engine(4_000, 4);
    let c = e.admit(NewJob::connection(1_000), ms(0)).unwrap();
    activate(&mut e, c);
    assert!(e.adjust_memory(c, 2_500));
    assert_eq!(e.resources().registered_memory_bytes, 3_500);
    assert!(e.adjust_memory(c, -500));

    e.complete(c, ms(10));
    assert_eq!(
        e.resources().registered_memory_bytes,
        0,
        "release must give back exactly what the re-priced job held"
    );
    assert_eq!(
        4_000 - e.resources().registered_memory_bytes,
        4_000,
        "budget must be whole again"
    );
}
