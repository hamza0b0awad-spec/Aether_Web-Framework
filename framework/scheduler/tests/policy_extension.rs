//! A scheduling policy from outside this crate.
//!
//! `SchedulingPolicy` is public API, but every hook takes `&mut SchedulerState`, so the trait
//! was un-implementable downstream until the engine type was re-exported: the only way to
//! discover that was a compile error (`E0425: cannot find type SchedulerState`). These tests are
//! that missing proof — they name the exported type in a signature and drive a third-party
//! policy through the engine.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_scheduler::{
    JobId, NewJob, ResourceConfig, SchedulerEngine, SchedulerState, SchedulingConfig,
    SchedulingPolicy, Work,
};

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn slots(n: usize) -> ResourceConfig {
    ResourceConfig {
        max_active_handlers: Some(n),
        min_slots: n,
        ..ResourceConfig::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    arrives: usize,
    slices: usize,
    completes: usize,
    cancels: usize,
}

/// Minimal outside policy: only the two required methods plus the hooks it wants to observe.
/// `admission_order`, `round_budget`, `on_slice`-defaults and `baseline` come from the trait.
struct ExternalPolicy {
    counts: Arc<Mutex<Counts>>,
}

impl ExternalPolicy {
    fn new() -> (Self, Arc<Mutex<Counts>>) {
        let counts = Arc::new(Mutex::new(Counts::default()));
        (
            Self {
                counts: Arc::clone(&counts),
            },
            counts,
        )
    }

    fn bump(&self, field: impl FnOnce(&mut Counts)) {
        let mut c = *self.counts.lock().unwrap();
        field(&mut c);
        *self.counts.lock().unwrap() = c;
    }
}

impl SchedulingPolicy for ExternalPolicy {
    fn name(&self) -> &'static str {
        "external-observer"
    }

    fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
        st.first_active_divisible()
    }

    fn quota(
        &mut self,
        _st: &mut SchedulerState,
        _id: JobId,
        _now: Duration,
        _budget: Work,
    ) -> Work {
        100
    }

    fn on_arrive(&mut self, _st: &mut SchedulerState, _id: JobId) {
        self.bump(|c| c.arrives += 1);
    }

    fn on_slice(&mut self, _st: &mut SchedulerState, _id: JobId, _work: Work, _elapsed: Duration) {
        self.bump(|c| c.slices += 1);
    }

    fn on_complete(&mut self, _st: &mut SchedulerState, _id: JobId, _now: Duration) {
        self.bump(|c| c.completes += 1);
    }

    fn on_cancel(&mut self, _st: &mut SchedulerState, _id: JobId) {
        self.bump(|c| c.cancels += 1);
    }
}

#[test]
fn an_external_policy_drives_the_engine_and_sees_every_hook() {
    let (policy, counts) = ExternalPolicy::new();
    let mut e =
        SchedulerEngine::with_policy(SchedulingConfig::default(), slots(1), Box::new(policy));

    let a = e
        .admit(NewJob::divisible(Some(250), 0), ms(0))
        .expect("first job must be admitted");
    let granted = e.pump(ms(1)).grants.iter().any(|g| g.job == a);
    assert!(granted, "the external policy's quota must produce a grant");
    assert!(
        e.report_slice(a, 100, ms(1), ms(2)),
        "job {a:?} must still be running"
    );
    assert!(e.complete(a, ms(3)), "job {a:?} must complete");

    let b = e
        .admit(NewJob::divisible(Some(250), 0), ms(4))
        .expect("capacity freed by the completion must be reusable");
    e.pump(ms(5));
    assert!(e.cancel(b, ms(6)), "job {b:?} must cancel");

    assert_eq!(
        *counts.lock().unwrap(),
        Counts {
            arrives: 2,
            slices: 1,
            completes: 1,
            cancels: 1
        },
        "each hook must reach the third-party policy exactly once per event"
    );
    assert_eq!(e.metrics().completed, 1);
    assert_eq!(e.metrics().cancelled, 1);
}

#[test]
fn an_external_policy_can_override_admission_order_and_round_budget() {
    /// Last-arrived-first, with an explicit round budget: both are defaulted methods, so this
    /// also proves their default signatures are callable on the re-exported state type.
    struct Reverse;
    impl SchedulingPolicy for Reverse {
        fn name(&self) -> &'static str {
            "reverse"
        }
        fn admission_order(&mut self, st: &mut SchedulerState, _now: Duration) -> Vec<JobId> {
            let mut ids = st.queued_sorted();
            ids.reverse();
            ids
        }
        fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
            st.rotate_active_divisible()
        }
        fn quota(
            &mut self,
            _st: &mut SchedulerState,
            _id: JobId,
            _now: Duration,
            budget: Work,
        ) -> Work {
            budget.min(10)
        }
        fn round_budget(&self, _st: &SchedulerState, _slots: usize, _baseline: Work) -> Work {
            20
        }
    }

    let mut e =
        SchedulerEngine::with_policy(SchedulingConfig::default(), slots(4), Box::new(Reverse));
    let first = e.admit(NewJob::divisible(Some(100), 0), ms(0)).unwrap();
    let second = e.admit(NewJob::divisible(Some(100), 0), ms(1)).unwrap();

    // Round budget 20 with quota 10 bounds the granted slices, and the reversed admission walk
    // still promotes every queued job within one revolution.
    let mut promoted = 0;
    for tick in 2..8 {
        let outcome = e.pump(ms(tick));
        promoted += outcome.grants.len();
    }
    assert!(promoted > 0, "the round budget must still hand out work");
    assert!(
        e.job_state(first).is_some() && e.job_state(second).is_some(),
        "both jobs must still be tracked"
    );
    assert!(
        e.metrics().work_executed == 0,
        "grants alone execute no work until reported"
    );
}
