use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crate::config::{ResourceConfig, SchedulingConfig};
use crate::job::{JobClass, JobId, JobRecord, JobState, NewJob, Work};
use crate::policy::{SchedulingPolicy, build_policy};
use crate::resources::{BlockedReason, ResourceSnapshot, Resources};
use crate::tree::JobTree;

/// A scheduling decision recorded for observability (design: metrics must show
/// the scheduler's decisions and their reasons).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionKind {
    Queued,
    Admitted,
    StarvationPromoted,
    RejectedQueueFull,
    RejectedMemory,
    Reclassified { from: JobClass, to: JobClass },
    Quota { work: Work },
    CapacityChanged { slots: usize },
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub now: Duration,
    pub job: JobId,
    pub kind: DecisionKind,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SchedulerMetrics {
    pub admitted: u64,
    pub rejected_queue_full: u64,
    pub rejected_memory: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub starvation_promotions: u64,
    pub class_promotions: u64,
    pub class_demotions: u64,
    pub grants_issued: u64,
    pub slice_reports: u64,
    pub work_executed: Work,
    pub peak_queue: usize,
    /// Queued jobs passed over during an admission walk because they did not fit.
    /// Non-zero is normal (that is how an unfittable job stops blocking the jobs
    /// behind it); a large value against zero admissions means the budget is the
    /// binding constraint, not the scheduler.
    pub skipped_unfit: u64,
    /// Times an admission walk ended at the scan-window bound with jobs still
    /// unexamined. Those jobs are reached on a later revolution of the window.
    pub scan_window_halts: u64,
    /// Times a *running* job asked for more registration than the budget had left and
    /// was refused, so the caller had to shed it (a connection whose read buffer grew
    /// past the remaining memory budget). Distinct from `rejected_memory`, which counts
    /// refusals before the job ever started.
    pub memory_shed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    pub job: JobId,
    pub quota: Work,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoundOutcome {
    pub admitted: Vec<JobId>,
    pub grants: Vec<Grant>,
    pub capacity_changed: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    QueueFull,
    MemoryBudget,
}

/// Everything a policy can see and adjust. Public so policies live outside the
/// engine module; the traversal itself only happens through [`SchedulerEngine`].
pub struct SchedulerState {
    pub cfg: SchedulingConfig,
    pub jobs: HashMap<JobId, JobRecord>,
    pub tree: JobTree,
    pub resources: Resources,
    pub metrics: SchedulerMetrics,
    pub decisions: VecDeque<Decision>,
    pub(crate) seq_next: u64,
    pub(crate) now: Duration,
    /// Where the next admission walk starts inside `admission_order`. The walk is
    /// bounded, so this cursor is what guarantees every queued job is examined
    /// within one revolution of the window.
    pub(crate) admission_cursor: usize,
}

impl SchedulerState {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Self {
        Self {
            jobs: HashMap::new(),
            tree: JobTree::new(),
            resources: Resources::new(rcfg),
            metrics: SchedulerMetrics::default(),
            decisions: VecDeque::new(),
            seq_next: 0,
            now: Duration::ZERO,
            admission_cursor: 0,
            cfg,
        }
    }

    #[must_use]
    pub fn job(&self, id: JobId) -> Option<&JobRecord> {
        self.jobs.get(&id)
    }

    #[must_use]
    pub fn job_mut(&mut self, id: JobId) -> Option<&mut JobRecord> {
        self.jobs.get_mut(&id)
    }

    /// All queued jobs, deterministically ordered by arrival sequence.
    #[must_use]
    pub fn queued_sorted(&self) -> Vec<JobId> {
        let mut ids = self.tree.queued_ids();
        ids.sort_by_key(|id| self.jobs.get(id).map(|j| j.seq).unwrap_or(u64::MAX));
        ids
    }

    /// Oldest-not-yet-granted active divisible job (FIFO-style run-to-completion).
    #[must_use]
    pub fn first_active_divisible(&mut self) -> Option<JobId> {
        // The rotation index only ever holds divisible jobs, so no kind filter is needed.
        for i in 0..self.tree.divisible_len() {
            let id = self.tree.divisible_ids()[i];
            if !self.jobs.get(&id).is_some_and(|j| j.granted_this_round) {
                return Some(id);
            }
        }
        None
    }

    /// Rotation pick: next ungranted active divisible job after the cursor. The
    /// cursor itself is advanced once per round by the engine, so successive
    /// rounds start at successive jobs.
    #[must_use]
    pub fn rotate_active_divisible(&self) -> Option<JobId> {
        let n = self.tree.divisible_len();
        if n == 0 {
            return None;
        }
        let ids = self.tree.divisible_ids();
        for i in 0..n {
            let idx = (self.tree.rotation_cursor + i) % n;
            let id = ids[idx];
            if self.jobs.get(&id).is_some_and(|j| !j.granted_this_round) {
                return Some(id);
            }
        }
        None
    }

    /// Move a job to another class node; counts promotions/demotions and logs it.
    pub fn set_class(&mut self, id: JobId, to: JobClass) {
        let from = match self.jobs.get(&id) {
            Some(j) => j.class,
            None => return,
        };
        if from == to {
            return;
        }
        if self.tree.is_queued(id, from) {
            self.tree.reclassify(id, from, to);
        }
        if let Some(j) = self.jobs.get_mut(&id) {
            j.class = to;
        }
        if to == JobClass::Large {
            self.metrics.class_promotions += 1;
        } else if from == JobClass::Large {
            self.metrics.class_demotions += 1;
        }
        self.log(id, DecisionKind::Reclassified { from, to });
    }

    pub(crate) fn log(&mut self, job: JobId, kind: DecisionKind) {
        let cap = self.cfg.decision_log_capacity.max(8);
        self.decisions.push_back(Decision {
            now: self.now,
            job,
            kind,
        });
        while self.decisions.len() > cap {
            self.decisions.pop_front();
        }
    }
}

/// Queued jobs one admission walk may examine. Bounds the policy work `pump` does
/// per release; `SchedulerState::admission_cursor` rotates the window so a deep
/// pile of unfittable jobs cannot hide the rest of the queue forever.
pub const ADMISSION_SCAN_WINDOW: usize = 32;

/// The engine: owns the tree, runs one deterministic traversal per `pump`, and is
/// the only place that mutates job lifecycle state. Pure sync code — callers pass
/// an explicit clock, drivers run on whatever runtime they like.
pub struct SchedulerEngine {
    pub(crate) st: SchedulerState,
    policy: Box<dyn SchedulingPolicy>,
    rejected_in_window: usize,
}

impl SchedulerEngine {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Self {
        let policy = build_policy(&cfg);
        Self {
            st: SchedulerState::new(cfg, rcfg),
            policy,
            rejected_in_window: 0,
        }
    }

    #[must_use]
    pub fn with_policy(
        cfg: SchedulingConfig,
        rcfg: ResourceConfig,
        policy: Box<dyn SchedulingPolicy>,
    ) -> Self {
        Self {
            st: SchedulerState::new(cfg, rcfg),
            policy,
            rejected_in_window: 0,
        }
    }

    #[must_use]
    pub fn policy_name(&self) -> &'static str {
        self.policy.name()
    }

    #[must_use]
    pub fn config(&self) -> &SchedulingConfig {
        &self.st.cfg
    }

    #[must_use]
    pub fn job(&self, id: JobId) -> Option<&JobRecord> {
        self.st.job(id)
    }

    #[must_use]
    pub fn job_state(&self, id: JobId) -> Option<JobState> {
        self.st.job(id).map(|j| j.state)
    }

    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.st.tree.queued_count()
    }

    #[must_use]
    pub fn metrics(&self) -> &SchedulerMetrics {
        &self.st.metrics
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.st.resources.snapshot()
    }

    /// Registered memory still available for admission. Read-only diagnostic.
    #[must_use]
    pub fn memory_available(&self) -> u64 {
        self.st.resources.memory_available()
    }

    /// Queued jobs in the order the next admission walk considers them. Read-only
    /// diagnostic; the walk itself re-reads this under the engine lock.
    #[must_use]
    pub fn queued_ids(&self) -> Vec<JobId> {
        self.st.tree.queued_ids()
    }

    /// Why a queued job could not be admitted at this instant, or `None` when it
    /// currently could (it just has not been reached by a walk yet). Explains a
    /// decision; never changes one.
    #[must_use]
    pub fn blocked_reason(&self, id: JobId) -> Option<BlockedReason> {
        let j = self.st.job(id)?;
        if j.state != JobState::Queued {
            return None;
        }
        self.st.resources.capacity_block(j.kind, j.memory_bytes)
    }

    /// Re-price a running job's memory registration by `delta` bytes.
    ///
    /// A connection reserves a small base when it opens and re-prices it as its read
    /// buffer actually grows, so an idle keep-alive socket stops consuming the whole
    /// worst-case body budget for the life of the connection. Growing is refused (state
    /// untouched, `memory_shed` bumped) when the budget cannot cover it and the caller
    /// must shed the work; shrinking is always allowed and clamped to what is charged.
    /// Returns `false` for an unknown or non-Active job, which is not a shed.
    pub fn adjust_memory(&mut self, id: JobId, delta: i64) -> bool {
        if delta == 0 {
            return true;
        }
        let Some(job) = self.st.job(id) else {
            return false;
        };
        if job.state != JobState::Active {
            return false;
        }
        if delta > 0 {
            let add = delta as u64;
            if !self.st.resources.grow_memory(add) {
                self.st.metrics.memory_shed += 1;
                // A refused grow is the caller shedding real work, so it counts toward
                // the pressure window like a rejection. Without it, a memory-bound
                // server sheds connections forever while `handler_slots` stays at the
                // ceiling — `rejected_in_window` was the only rejection term feeding
                // the adaptation, and memory never entered it.
                self.rejected_in_window += 1;
                return false;
            }
            if let Some(job) = self.st.job_mut(id) {
                job.memory_bytes = job.memory_bytes.saturating_add(add);
            }
            true
        } else {
            let free = delta
                .unsigned_abs()
                .min(self.st.job(id).map(|j| j.memory_bytes).unwrap_or(0));
            self.st.resources.shrink_memory(free);
            if let Some(job) = self.st.job_mut(id) {
                job.memory_bytes = job.memory_bytes.saturating_sub(free);
            }
            true
        }
    }

    #[must_use]
    pub fn baseline(&self) -> Work {
        self.policy
            .baseline()
            .unwrap_or(self.st.cfg.default_baseline)
    }
    #[must_use]
    pub fn decisions(&self) -> &VecDeque<Decision> {
        &self.st.decisions
    }

    /// Register a new job. Overflow policy: when the queue is at `max_queue` the
    /// arrival is rejected outright — no silent dropping, no priority guessing.
    pub fn admit(&mut self, req: NewJob, now: Duration) -> Result<JobId, RejectReason> {
        self.st.now = now;
        if self.st.resources.over_ceiling(req.memory_bytes) {
            self.st.metrics.rejected_memory += 1;
            self.rejected_in_window += 1;
            let id = JobId(self.st.seq_next);
            self.st.log(id, DecisionKind::RejectedMemory);
            return Err(RejectReason::MemoryBudget);
        }
        if self.st.tree.queued_count() >= self.st.cfg.max_queue {
            self.st.metrics.rejected_queue_full += 1;
            self.rejected_in_window += 1;
            let id = JobId(self.st.seq_next);
            self.st.log(id, DecisionKind::RejectedQueueFull);
            return Err(RejectReason::QueueFull);
        }
        let id = JobId(self.st.seq_next);
        self.st.seq_next += 1;
        self.st.jobs.insert(id, JobRecord::new(id, id.0, &req, now));
        self.st.tree.queue(id, JobClass::Unknown);
        self.st.metrics.peak_queue = self.st.metrics.peak_queue.max(self.st.tree.queued_count());
        self.st.log(id, DecisionKind::Queued);
        self.policy.on_arrive(&mut self.st, id);
        Ok(id)
    }

    /// One round: resource refresh → admission by arrival precedence (starvation
    /// guard leading) → bounded rotation of divisible jobs under a round budget.
    pub fn pump(&mut self, now: Duration) -> RoundOutcome {
        self.st.now = now;
        let mut out = RoundOutcome::default();

        let max_queue = self.st.cfg.max_queue.max(1);
        let queue_ratio = self.st.tree.queued_count() as f64 / max_queue as f64;
        let rejected = std::mem::take(&mut self.rejected_in_window);
        if let Some(slots) = self.st.resources.refresh(now, queue_ratio, rejected) {
            self.st
                .log(JobId(u64::MAX), DecisionKind::CapacityChanged { slots });
            out.capacity_changed = Some(slots);
        }

        // Admission walk: the policy's order (arrival precedence, starved jobs
        // leading) considered through a bounded window. A job that does not fit is
        // skipped so it cannot block jobs behind it that do fit — stopping at the
        // first refusal turned one unfittable connection into a full stall, with
        // every handler slot empty. The window bounds the work `pump` does per
        // release; `admission_cursor` rotates it, so an unfittable head cannot
        // permanently hide the tail, and a starved job is reached again within one
        // revolution.
        let order = self.policy.admission_order(&mut self.st, now);
        let n = order.len();
        if n > 0 {
            let start = self.st.admission_cursor % n;
            let window = ADMISSION_SCAN_WINDOW.min(n);
            let mut examined = 0usize;
            let mut skipped = 0usize;
            while examined < window {
                let id = order[(start + examined) % n];
                examined += 1;
                let (kind, mem, starved) = match self.st.job(id) {
                    Some(j) => (
                        j.kind,
                        j.memory_bytes,
                        j.queue_wait(now) >= self.st.cfg.starvation_guard,
                    ),
                    None => continue,
                };
                if !self.st.resources.try_admit(kind, mem) {
                    skipped += 1;
                    continue;
                }
                let class = if let Some(j) = self.st.job_mut(id) {
                    j.state = JobState::Active;
                    j.admitted_at = Some(now);
                    j.class
                } else {
                    continue;
                };
                self.st.tree.unqueue(id, class);
                // Only divisible work joins the rotation index. Connections and handlers
                // are active for a long time, can never be granted a slice, and would
                // otherwise be scanned by every round and searched by every close.
                if kind.is_divisible() {
                    self.st.tree.promote_divisible(id);
                }
                self.st.metrics.admitted += 1;
                if starved {
                    self.st.metrics.starvation_promotions += 1;
                    self.st.log(id, DecisionKind::StarvationPromoted);
                }
                self.st.log(id, DecisionKind::Admitted);
                out.admitted.push(id);
            }
            self.st.metrics.skipped_unfit += skipped as u64;
            if examined < n {
                self.st.metrics.scan_window_halts += 1;
            }
            // Only rotate when the walk did not already cover the whole queue.
            if window < n {
                self.st.admission_cursor = (start + examined) % n;
            }
        }

        // Execution rotation for divisible jobs, bounded by the round budget.
        for i in 0..self.st.tree.divisible_len() {
            let id = self.st.tree.divisible_ids()[i];
            if let Some(j) = self.st.jobs.get_mut(&id) {
                j.granted_this_round = false;
            }
        }
        let baseline_v = self
            .policy
            .baseline()
            .unwrap_or(self.st.cfg.default_baseline)
            .max(1);
        let slots = self.st.resources.handler_slots();
        let mut budget = self.policy.round_budget(&self.st, slots, baseline_v);
        while let Some(id) = self.policy.pick_execution(&mut self.st, now) {
            let q = self.policy.quota(&mut self.st, id, now, budget).min(budget);
            if let Some(j) = self.st.jobs.get_mut(&id) {
                j.granted_this_round = true;
                if q > 0 {
                    j.last_grant = q;
                }
            }
            if q > 0 {
                budget = budget.saturating_sub(q);
                out.grants.push(Grant { job: id, quota: q });
                self.st.metrics.grants_issued += 1;
                self.st.log(id, DecisionKind::Quota { work: q });
            }
        }
        // Advance the rotation start for the next round.
        let n = self.st.tree.divisible_len();
        if n > 0 {
            self.st.tree.rotation_cursor = (self.st.tree.rotation_cursor + 1) % n;
        }
        out
    }

    /// Driver reports one completed slice of a `Divisible` job. Returns false for
    /// unknown, terminal, or indivisible jobs (work can only be reported for work
    /// that was actually granted a slice).
    pub fn report_slice(
        &mut self,
        id: JobId,
        work: Work,
        elapsed: Duration,
        now: Duration,
    ) -> bool {
        self.st.now = now;
        let accepted = self
            .st
            .job(id)
            .is_some_and(|j| j.state == JobState::Active && j.kind.is_divisible());
        if !accepted {
            return false;
        }
        if let Some(j) = self.st.job_mut(id) {
            j.executed_work = j.executed_work.saturating_add(work);
            j.total_runtime = j.total_runtime.saturating_add(elapsed);
            j.slices = j.slices.saturating_add(1);
            j.remaining_work = j.remaining_work.map(|r| r.saturating_sub(work));
            j.last_executed_at = Some(now);
        }
        self.st.metrics.slice_reports += 1;
        self.st.metrics.work_executed = self.st.metrics.work_executed.saturating_add(work);
        self.policy.on_slice(&mut self.st, id, work, elapsed);
        true
    }

    /// Terminal: job finished normally. Releases its slot and memory exactly once.
    pub fn complete(&mut self, id: JobId, now: Duration) -> bool {
        self.finish(id, now, true)
    }

    /// Terminal: disconnect, timeout, or shutdown drain. The job is never
    /// rescheduled as if healthy; terminal states are sticky.
    pub fn cancel(&mut self, id: JobId, now: Duration) -> bool {
        self.finish(id, now, false)
    }

    fn finish(&mut self, id: JobId, now: Duration, completed: bool) -> bool {
        self.st.now = now;
        let (state, kind, mem, class, was_active, implicit_work) = match self.st.job(id) {
            Some(j) if j.state.is_terminal() => return false,
            Some(j) => (
                j.state,
                j.kind,
                j.memory_bytes,
                j.class,
                j.state == JobState::Active,
                // A completed job with a declared total has, by definition, now
                // executed all of it (indivisible jobs report no slices).
                // `remaining_work` is already decremented by every slice report,
                // so the unreported remainder *is* the whole remaining total —
                // subtracting `executed_work` again here would double-count it
                // and silently under-report completed work and baseline samples.
                // Only an admitted job can have executed anything: `complete()` is
                // public, and crediting a Queued job's declared hint here would
                // bill never-performed work into `work_executed` and the baseline.
                if completed && j.state == JobState::Active {
                    j.remaining_work.unwrap_or(0)
                } else {
                    0
                },
            ),
            None => return false,
        };
        let _ = state;
        if was_active {
            if kind.is_divisible() {
                self.st.tree.remove_divisible(id);
            }
            self.st.resources.release(kind, mem);
        } else {
            self.st.tree.unqueue(id, class);
        }
        if let Some(j) = self.st.job_mut(id) {
            j.state = if completed {
                JobState::Completed
            } else {
                JobState::Cancelled
            };
            j.executed_work = j.executed_work.saturating_add(implicit_work);
            j.remaining_work = j.remaining_work.map(|r| r.saturating_sub(implicit_work));
        }
        self.st.metrics.work_executed = self.st.metrics.work_executed.saturating_add(implicit_work);
        if completed {
            self.policy.on_complete(&mut self.st, id, now);
            self.st.metrics.completed += 1;
            self.st.log(id, DecisionKind::Completed);
        } else {
            self.policy.on_cancel(&mut self.st, id);
            self.st.metrics.cancelled += 1;
            self.st.log(id, DecisionKind::Cancelled);
        }
        self.st.jobs.remove(&id);
        true
    }

    /// Slots currently granted for adaptive handler execution (integration hint).
    #[must_use]
    pub fn handler_slots(&self) -> usize {
        self.st.resources.handler_slots()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn engine() -> SchedulerEngine {
        SchedulerEngine::new(SchedulingConfig::default(), ResourceConfig::default())
    }

    /// The rotation index exists to hand out work slices, so it must be the size of the
    /// divisible set — not the size of the connection count. When it held every job kind,
    /// every round scanned and every close searched a list of all live sockets.
    #[test]
    fn only_divisible_jobs_join_the_rotation_index() {
        let mut e = engine();
        let t = Duration::from_millis(1);
        let conn = e.admit(NewJob::connection(4096), t).unwrap();
        let hnd = e.admit(NewJob::handler(4096), t).unwrap();
        let cpu = e.admit(NewJob::cpu_heavy(4096), t).unwrap();
        let div = e.admit(NewJob::divisible(Some(10), 4096), t).unwrap();
        e.pump(t);

        assert_eq!(
            e.st.tree.divisible_ids(),
            &[div],
            "a connection, handler or cpu job in the index is what made pump() O(connections)"
        );
        // All four are live, so the index is not simply a subset of admission.
        let r = e.resources();
        assert_eq!(
            (
                r.active_connections,
                r.active_handlers,
                r.active_cpu,
                r.active_divisible
            ),
            (1, 1, 1, 1)
        );

        // Closing the non-divisible jobs must leave the one divisible job alone.
        for id in [conn, hnd, cpu] {
            e.cancel(id, t);
        }
        assert_eq!(
            e.st.tree.divisible_ids(),
            &[div],
            "closing a connection shifted or dropped a rotation entry"
        );
    }

    /// No orphaned entry may survive a divisible job, and removal must take exactly the
    /// element asked for — the index is positional, so a wrong entry moves the cursor.
    #[test]
    fn divisible_removal_keeps_the_remaining_order_and_leaves_no_orphans() {
        let mut e = engine();
        let t = Duration::from_millis(1);
        let a = e.admit(NewJob::divisible(Some(10), 4096), t).unwrap();
        let b = e.admit(NewJob::divisible(Some(10), 4096), t).unwrap();
        let c = e.admit(NewJob::divisible(Some(10), 4096), t).unwrap();
        e.pump(t);
        assert_eq!(e.st.tree.divisible_ids(), &[a, b, c]);

        // Through the real lifecycle, so the resource counter and the index must agree.
        e.cancel(b, t);
        assert_eq!(
            e.st.tree.divisible_ids(),
            &[a, c],
            "removal took the wrong element"
        );

        e.complete(a, t);
        e.complete(c, t);
        assert_eq!(
            e.st.tree.divisible_len(),
            0,
            "a finished job left an orphan in the index"
        );
        assert_eq!(e.resources().active_divisible, 0);

        // An id that is not in the index must be refused, never shift a neighbour.
        assert!(!e.st.tree.remove_divisible(b));
    }

    /// A policy banks per-job state (deficits, learning) against a `JobId`, and ids are
    /// never reused, so the only thing that releases it is a terminal hook. `on_complete`
    /// was the sole one: cancel is the *most common* terminal transition a server has
    /// (disconnect, timeout, shutdown drain), so every cancelled job leaked its entry.
    #[test]
    fn every_terminal_transition_notifies_the_policy() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Hooks {
            cancels: Arc<AtomicUsize>,
            completions: Arc<AtomicUsize>,
        }
        impl SchedulingPolicy for Hooks {
            fn name(&self) -> &'static str {
                "hooks"
            }
            fn pick_execution(
                &mut self,
                _st: &mut SchedulerState,
                _now: Duration,
            ) -> Option<JobId> {
                None
            }
            fn quota(
                &mut self,
                _st: &mut SchedulerState,
                _id: JobId,
                _now: Duration,
                _budget: Work,
            ) -> Work {
                0
            }
            fn on_cancel(&mut self, _st: &mut SchedulerState, _id: JobId) {
                self.cancels.fetch_add(1, Ordering::SeqCst);
            }
            fn on_complete(&mut self, _st: &mut SchedulerState, _id: JobId, _now: Duration) {
                self.completions.fetch_add(1, Ordering::SeqCst);
            }
        }

        let cancels = Arc::new(AtomicUsize::new(0));
        let completions = Arc::new(AtomicUsize::new(0));
        let mut e = SchedulerEngine::with_policy(
            SchedulingConfig::default(),
            ResourceConfig {
                max_active_handlers: Some(1),
                min_slots: 1,
                ..ResourceConfig::default()
            },
            Box::new(Hooks {
                cancels: Arc::clone(&cancels),
                completions: Arc::clone(&completions),
            }),
        );
        let t = Duration::from_millis(1);
        let active = e.admit(NewJob::handler(0), t).unwrap();
        let queued = e.admit(NewJob::handler(0), t).unwrap();
        e.pump(t);
        assert_eq!(e.job_state(active), Some(JobState::Active));
        assert_eq!(e.job_state(queued), Some(JobState::Queued));

        assert!(e.complete(active, t));
        assert_eq!(completions.load(Ordering::SeqCst), 1);
        assert_eq!(cancels.load(Ordering::SeqCst), 0);

        // A job that never left the queue still has policy state to release.
        assert!(e.cancel(queued, t));
        assert_eq!(cancels.load(Ordering::SeqCst), 1);

        // Terminal states are sticky, so a repeated call must not double-release.
        assert!(!e.cancel(queued, t));
        assert!(!e.complete(active, t));
        assert_eq!(
            (
                cancels.load(Ordering::SeqCst),
                completions.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
    }

    /// `complete()` is public and takes no driver proof: a caller may finish a job the
    /// engine never admitted. Its declared hint must not then be billed as work that
    /// ran — that inflates `work_executed` and feeds a never-executed size into the
    /// adaptive baseline, which is exactly the sample the whole policy learns from.
    #[test]
    fn completing_a_queued_job_credits_no_never_executed_work() {
        let mut e = engine();
        let t = Duration::from_millis(1);
        let never_started = e.admit(NewJob::divisible(Some(500), 0), t).unwrap();
        assert_eq!(e.job_state(never_started), Some(JobState::Queued));
        assert!(e.complete(never_started, t));
        assert_eq!(
            e.metrics().work_executed,
            0,
            "a job that never ran executed nothing"
        );
        assert_eq!(e.metrics().completed, 1);

        // The intended path is untouched: an admitted job with a declared total that
        // completes normally has, by definition, now executed all of it.
        let ran = e.admit(NewJob::divisible(Some(500), 0), t).unwrap();
        e.pump(t);
        assert_eq!(e.job_state(ran), Some(JobState::Active));
        assert!(e.complete(ran, t));
        assert_eq!(
            e.metrics().work_executed,
            500,
            "the declared remainder of a finished job still counts"
        );
    }

    fn memory_tight() -> ResourceConfig {
        ResourceConfig {
            memory_budget_bytes: 1_000,
            max_active_handlers: Some(8),
            ..ResourceConfig::default()
        }
    }

    /// Handler slots only shrink through `rejected_in_window`, and memory rejections
    /// never entered it — so a server shedding work on the memory budget kept running
    /// the full handler ceiling, which is the load that consumes the budget.
    #[test]
    fn sustained_memory_rejection_shrinks_adaptive_slots() {
        let mut e = SchedulerEngine::new(
            SchedulingConfig {
                max_queue: 256,
                ..SchedulingConfig::default()
            },
            memory_tight(),
        );
        // Real queued work, so the queue term alone cannot explain the pressure.
        for _ in 0..64 {
            e.admit(NewJob::handler(0), Duration::ZERO).unwrap();
        }
        let before = e.handler_slots();
        for round in 0..2u64 {
            let t = Duration::from_millis(1_500 * round + 1);
            for _ in 0..150 {
                // Each arrival asks for more than the entire registered budget.
                assert_eq!(
                    e.admit(NewJob::handler(5_000), t),
                    Err(RejectReason::MemoryBudget)
                );
            }
            e.pump(t);
        }
        assert_eq!(e.metrics().rejected_memory, 300);
        assert_eq!(
            e.handler_slots(),
            before / 2,
            "memory shedding must reach the pressure signal"
        );
    }

    /// Same signal, other path: a running job re-priced beyond what the budget has left
    /// is shed by the caller. `memory_shed` was a diagnostic only, so sustained shedding
    /// of growing read buffers never reduced concurrency.
    #[test]
    fn refused_memory_growth_shrinks_adaptive_slots() {
        let mut e = SchedulerEngine::new(
            SchedulingConfig::default(),
            ResourceConfig {
                high_pressure: 0.5,
                ..memory_tight()
            },
        );
        let t = Duration::from_millis(1);
        let conn = e.admit(NewJob::connection(100), t).unwrap();
        e.pump(t);
        assert_eq!(e.job_state(conn), Some(JobState::Active));
        let before = e.handler_slots();
        for round in 0..2u64 {
            let t = Duration::from_millis(1_500 * round + 1);
            for _ in 0..150 {
                assert!(
                    !e.adjust_memory(conn, 2_000),
                    "900 bytes left cannot cover a 2000-byte grow"
                );
            }
            e.pump(t);
        }
        assert_eq!(e.metrics().memory_shed, 300);
        assert_eq!(
            e.handler_slots(),
            before / 2,
            "shedding real work is pressure, whatever its cause"
        );
    }
}
