use std::collections::HashMap;
use std::time::Duration;

use crate::baseline::Baseline;
use crate::config::SchedulingConfig;
use crate::engine::SchedulerState;
use crate::job::{JobClass, JobId, Work};
use crate::quota::QuotaModel;

/// Selection and quota logic, pluggable against one shared engine (admission,
/// queue bounds, rejection, and resource caps are engine-level and identical for
/// every policy, so comparisons isolate the scheduling logic only).
pub trait SchedulingPolicy: Send {
    fn name(&self) -> &'static str;

    /// Queued jobs in the order the engine should consider them for promotion. Default: strict
    /// arrival precedence (lowest sequence number first).
    fn admission_order(&mut self, st: &mut SchedulerState, _now: Duration) -> Vec<JobId> {
        st.queued_sorted()
    }

    /// Next active divisible job due one slice this round (`None` ⇒ no rotation).
    fn pick_execution(&mut self, st: &mut SchedulerState, now: Duration) -> Option<JobId>;

    /// Work units granted to `id` for its next slice, given the remaining round
    /// budget. Returning 0 skips the job this round.
    fn quota(&mut self, st: &mut SchedulerState, id: JobId, now: Duration, budget: Work) -> Work;

    /// Total work budget one round may hand out. Default: unbounded (only the
    /// adaptive tree enforces a round budget by design).
    fn round_budget(&self, _st: &SchedulerState, _slots: usize, _baseline: Work) -> Work {
        Work::MAX / 8
    }

    fn on_arrive(&mut self, _st: &mut SchedulerState, _id: JobId) {}
    fn on_slice(&mut self, _st: &mut SchedulerState, _id: JobId, _work: Work, _elapsed: Duration) {}
    fn on_complete(&mut self, _st: &mut SchedulerState, _id: JobId, _now: Duration) {}
    /// Terminal counterpart of [`Self::on_complete`] for a cancelled job: the job is
    /// gone and can never complete, so per-job state a policy banks for it must be
    /// dropped here or it leaks (JobIds are never reused).
    fn on_cancel(&mut self, _st: &mut SchedulerState, _id: JobId) {}

    /// Current stable baseline, for metrics; plain policies report `None`.
    fn baseline(&self) -> Option<Work> {
        None
    }
}

#[must_use]
pub fn build_policy(cfg: &SchedulingConfig) -> Box<dyn SchedulingPolicy> {
    use crate::config::PolicyKind;
    match cfg.policy {
        PolicyKind::Fifo => Box::new(Fifo),
        PolicyKind::RoundRobin => Box::new(RoundRobin),
        PolicyKind::Deficit => Box::new(DeficitRoundRobin::default()),
        PolicyKind::AdaptiveTree => Box::new(AdaptiveTree::from_cfg(cfg)),
    }
}

/// Strict arrival order, run-to-completion, no quota rotation.
#[derive(Debug, Default)]
pub struct Fifo;

impl SchedulingPolicy for Fifo {
    fn name(&self) -> &'static str {
        "fifo"
    }

    fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
        st.first_active_divisible()
    }

    fn quota(&mut self, st: &mut SchedulerState, id: JobId, _now: Duration, budget: Work) -> Work {
        // Run to completion: the whole remainder in one grant, bounded by budget.
        st.job(id)
            .and_then(|j| j.grantable_remaining())
            .unwrap_or(budget)
            .min(budget)
    }
}

/// Fixed-quantum rotation among active jobs, arrival order between equals.
#[derive(Debug, Default)]
pub struct RoundRobin;

impl SchedulingPolicy for RoundRobin {
    fn name(&self) -> &'static str {
        "round_robin"
    }

    fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
        st.rotate_active_divisible()
    }

    fn quota(&mut self, st: &mut SchedulerState, id: JobId, _now: Duration, budget: Work) -> Work {
        let q = st.cfg.default_baseline.max(1);
        st.job(id)
            .map(|j| j.grantable_remaining().unwrap_or(q).min(q))
            .unwrap_or(q)
            .min(budget)
    }
}

/// Deficit round robin: a job that cannot use its whole quantum banks the
/// remainder as deficit and gets it back on a later turn. Standard fair-share
/// comparator for the adaptive tree to be measured against.
#[derive(Debug, Default)]
pub struct DeficitRoundRobin {
    deficit: HashMap<JobId, Work>,
}

impl SchedulingPolicy for DeficitRoundRobin {
    fn name(&self) -> &'static str {
        "deficit_rr"
    }

    fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
        st.rotate_active_divisible()
    }

    fn quota(&mut self, st: &mut SchedulerState, id: JobId, _now: Duration, budget: Work) -> Work {
        let quantum = st.cfg.default_baseline.max(1);
        let owed = self.deficit.get(&id).copied().unwrap_or(0);
        let q = quantum.saturating_add(owed);
        st.job(id)
            .map(|j| j.grantable_remaining().unwrap_or(q).min(q))
            .unwrap_or(q)
            .min(budget)
    }

    fn on_slice(&mut self, st: &mut SchedulerState, id: JobId, work: Work, _elapsed: Duration) {
        // Deficit round robin carries the *unused part of the grant that was just
        // made*, not a debt added on top of it. The banked quantum is already inside
        // `last_grant`, so accumulating would re-bill the same credit every round and
        // a job that once under-used a quantum would receive `quantum + owed`
        // permanently — which is why this policy used to behave exactly like RR.
        let granted = st.job(id).map(|j| j.last_grant).unwrap_or(0);
        let unused = granted.saturating_sub(work);
        if unused == 0 {
            self.deficit.remove(&id);
        } else {
            self.deficit.insert(id, unused);
        }
    }

    fn on_complete(&mut self, st: &mut SchedulerState, id: JobId, _now: Duration) {
        let _ = st;
        self.deficit.remove(&id);
    }

    fn on_cancel(&mut self, st: &mut SchedulerState, id: JobId) {
        let _ = st;
        self.deficit.remove(&id);
    }
}

/// Adaptive Tree Scheduling (design §4): stable baseline, hysteresis size classes,
/// quota'd large jobs, arrival precedence with a starvation guard, and a bounded
/// rotation among active divisible jobs.
pub struct AdaptiveTree {
    baseline: Baseline,
}

impl AdaptiveTree {
    #[must_use]
    pub fn from_cfg(cfg: &SchedulingConfig) -> Self {
        Self {
            baseline: Baseline::from_cfg(cfg),
        }
    }
}

impl SchedulingPolicy for AdaptiveTree {
    fn name(&self) -> &'static str {
        "adaptive_tree"
    }

    fn on_arrive(&mut self, st: &mut SchedulerState, id: JobId) {
        // A declared hint classifies up-front; unknown sizes enter as `Unknown`
        // and converge from executed work after slices.
        let (hint, prev) = match st.job(id) {
            Some(j) => (j.work_hint, j.class),
            None => return,
        };
        if let Some(hint) = hint {
            let new = self.baseline.classify(hint, prev);
            st.set_class(id, new);
        }
    }

    fn admission_order(&mut self, st: &mut SchedulerState, now: Duration) -> Vec<JobId> {
        let guard = st.cfg.starvation_guard;
        let mut queued = st.queued_sorted();
        // Starvation guard: the oldest over-protected job leads regardless of class;
        // it takes first refusal on capacity that frees, but it never vetoes the
        // walk (a veto is what turned one unfittable connection into a full stall).
        queued.sort_by_key(|id| {
            let protected = st.job(*id).is_some_and(|j| j.queue_wait(now) >= guard);
            let seq = st.job(*id).map(|j| j.seq).unwrap_or(u64::MAX);
            (!protected, seq)
        });
        queued
    }

    fn pick_execution(&mut self, st: &mut SchedulerState, _now: Duration) -> Option<JobId> {
        st.rotate_active_divisible()
    }

    fn quota(&mut self, st: &mut SchedulerState, id: JobId, now: Duration, budget: Work) -> Work {
        let (class, baseline) = match st.job(id) {
            Some(j) => (j.class, self.baseline.value()),
            None => return 0,
        };
        let q = match class {
            JobClass::Small => {
                // Small divisible work runs to completion; no fake preemption.
                st.job(id)
                    .and_then(|j| j.grantable_remaining())
                    .unwrap_or(baseline.max(1))
            }
            JobClass::Unknown => {
                // Unknown-size jobs get at most one baseline share per round and
                // re-classify from executed work after every slice.
                let capped = baseline.max(1);
                st.job(id)
                    .and_then(|j| j.grantable_remaining())
                    .map_or(capped, |r| r.min(capped))
            }
            JobClass::Large => {
                let wait = st
                    .job(id)
                    .map(|j| j.grant_wait(now))
                    .unwrap_or(Duration::ZERO);
                let cfg = st.cfg.clone();
                let Some(job) = st.job_mut(id) else {
                    return 0;
                };
                QuotaModel::grant(&cfg, job, baseline, wait, budget)
            }
        };
        q.min(budget)
    }

    fn round_budget(&self, st: &SchedulerState, slots: usize, baseline: Work) -> Work {
        // Σ quota per round ≤ slots × B × round_factor (design §4 step 6).
        (slots as Work)
            .saturating_mul(baseline.max(1))
            .saturating_mul(st.cfg.round_factor.max(1))
    }

    fn on_slice(&mut self, st: &mut SchedulerState, id: JobId, work: Work, elapsed: Duration) {
        {
            let cfg = st.cfg.clone();
            let Some(job) = st.job_mut(id) else { return };
            QuotaModel::observe_slice(&cfg, job, work, elapsed);
        }
        let (measured, prev) = match st.job(id) {
            Some(j) => (j.measured_work(), j.class),
            None => return,
        };
        let new = self.baseline.classify(measured, prev);
        st.set_class(id, new);
    }

    fn on_complete(&mut self, st: &mut SchedulerState, id: JobId, _now: Duration) {
        // Only non-large completions feed the baseline sample; large jobs can never
        // move B (design §4.1). Jobs that report no executed work (indivisible
        // handlers/connections have work units of zero by nature) must not feed it
        // either: sampling them as "ordinary work of size 1" collapses the baseline
        // under normal HTTP traffic and starves divisible quotas derived from it.
        let (class, executed) = match st.job(id) {
            Some(j) => (j.class, j.executed_work),
            None => return,
        };
        if class != JobClass::Large && executed > 0 {
            self.baseline.observe_completed(executed);
        }
    }

    fn baseline(&self) -> Option<Work> {
        Some(self.baseline.value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResourceConfig;
    use crate::job::{JobRecord, NewJob};

    /// A state holding one active divisible job with a large declared total, so the
    /// round budget never binds and only the policy's own arithmetic is under test.
    fn state(total: Work) -> (SchedulerState, JobId) {
        let mut st = SchedulerState::new(SchedulingConfig::default(), ResourceConfig::default());
        let id = JobId(0);
        let req = NewJob::divisible(Some(total), 0);
        st.jobs
            .insert(id, JobRecord::new(id, 0, &req, Duration::ZERO));
        (st, id)
    }

    /// One round the way the engine runs it: ask for a quota, record it as the
    /// grant, then report how much of it the driver actually executed.
    fn round(p: &mut DeficitRoundRobin, st: &mut SchedulerState, id: JobId, used: Work) -> Work {
        let q = p.quota(st, id, Duration::ZERO, Work::MAX / 8);
        st.job_mut(id).unwrap().last_grant = q;
        p.on_slice(st, id, used, Duration::ZERO);
        q
    }

    #[test]
    fn deficit_round_robin_repays_the_banked_credit_instead_of_re_billing_it() {
        let mut p = DeficitRoundRobin::default();
        let (mut st, id) = state(10_000);
        let quantum = st.cfg.default_baseline;

        assert_eq!(round(&mut p, &mut st, id, quantum), quantum, "no debt yet");
        // Use 20 of 100: the 80 it could not spend is owed back once.
        assert_eq!(
            round(&mut p, &mut st, id, 20),
            quantum,
            "the previous grant was fully used, so nothing is owed"
        );
        assert_eq!(
            round(&mut p, &mut st, id, quantum + 80),
            quantum + 80,
            "quantum + deficit"
        );
        // The round above spent the whole credit, so the debt is settled: a job that
        // fully uses its enlarged grant must fall back to one plain quantum, not
        // accumulate `quantum + owed` forever (which made this policy behave exactly
        // like plain round robin).
        assert_eq!(
            round(&mut p, &mut st, id, quantum),
            quantum,
            "settled debt is not re-billed"
        );
        assert!(p.deficit.is_empty(), "a fully served grant banks nothing");
    }

    #[test]
    fn deficit_round_robin_drops_state_when_a_job_is_cancelled() {
        let mut p = DeficitRoundRobin::default();
        let (mut st, id) = state(10_000);
        round(&mut p, &mut st, id, 20);
        assert_eq!(p.deficit.len(), 1, "the under-used grant banked a deficit");
        p.on_cancel(&mut st, id);
        assert!(
            p.deficit.is_empty(),
            "cancelled jobs never complete, so their debt must be dropped"
        );
    }
}
