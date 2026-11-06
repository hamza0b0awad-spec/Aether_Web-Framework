use std::collections::VecDeque;

use crate::job::{JobClass, JobId};

/// The adaptive tree's structure: a root (owned by the engine) indexing three
/// class nodes — `SMALL`, `LARGE`, `UNKNOWN` — with job leaves under their class.
/// A round is one deterministic traversal of this tree (design §4).
///
/// Queue membership operations scan linearly; queues are bounded by
/// `SchedulingConfig::max_queue` (default 1024), which keeps the scans cheap and
/// the traversal deterministic.
#[derive(Debug, Default)]
pub struct JobTree {
    queued: [VecDeque<JobId>; 3],
    /// Active **divisible** jobs, in rotation order — the invariant is structural: only
    /// [`JobKind::Divisible`] jobs are ever inserted, because this is the round-robin
    /// index for work that is granted slices, not a registry of live jobs.
    ///
    /// It used to hold every job kind, and that was the bug: a live HTTP connection is
    /// active for its whole keep-alive lifetime, can never receive a grant, and made
    /// every round scan and every close search-and-shift a list as long as the
    /// connection count — twice per request, under the engine lock.
    divisible: Vec<JobId>,
    /// Rotation cursor advanced by round-robin style policies.
    pub rotation_cursor: usize,
}

impl JobTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue(&mut self, id: JobId, class: JobClass) {
        self.queued[class.index()].push_back(id);
    }

    #[must_use]
    pub fn is_queued(&self, id: JobId, class: JobClass) -> bool {
        self.queued[class.index()].contains(&id)
    }

    pub fn unqueue(&mut self, id: JobId, class: JobClass) -> bool {
        let node = &mut self.queued[class.index()];
        match node.iter().position(|j| *j == id) {
            Some(p) => {
                node.remove(p);
                true
            }
            None => false,
        }
    }

    pub fn reclassify(&mut self, id: JobId, from: JobClass, to: JobClass) {
        if from == to {
            return;
        }
        self.unqueue(id, from);
        // Keep arrival precedence: insert into the target class preserving seq order
        // is the engine's responsibility (callers re-push; ordering across classes is
        // resolved by a seq sort at pick time, so tail insertion is sufficient).
        self.queue(id, to);
    }

    /// Insert a job into the rotation index. Only divisible jobs belong here;
    /// [`crate::engine::SchedulerEngine`] enforces that at the single admission point.
    pub fn promote_divisible(&mut self, id: JobId) {
        self.divisible.push(id);
    }

    pub fn remove_divisible(&mut self, id: JobId) -> bool {
        match self.divisible.iter().position(|j| *j == id) {
            Some(p) => {
                self.divisible.remove(p);
                if self.rotation_cursor > 0 && self.rotation_cursor >= self.divisible.len() {
                    self.rotation_cursor = self.divisible.len().saturating_sub(1);
                }
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn divisible_len(&self) -> usize {
        self.divisible.len()
    }

    /// Rotation index contents, for the invariant test.
    #[must_use]
    pub fn divisible_ids(&self) -> &[JobId] {
        &self.divisible
    }

    #[must_use]
    pub fn queued_count(&self) -> usize {
        self.queued.iter().map(|n| n.len()).sum()
    }

    #[must_use]
    pub fn queued_ids(&self) -> Vec<JobId> {
        self.queued.iter().flatten().copied().collect()
    }
}
