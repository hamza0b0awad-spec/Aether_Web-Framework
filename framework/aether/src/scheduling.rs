//! Opt-in adaptive scheduling for the Aether serve path.
//!
//! [`SchedulingLayer`] wraps the protocol-independent `aether-scheduler` engine
//! and exposes three integration points, each honest about what it gates:
//!
//! 1. **Connection admission** — bounded queue + rejection (503) instead of the
//!    default unbounded blocking wait on the connection semaphore.
//! 2. **Handler execution admission** — a per-handler permit taken *before* the
//!    handler task is spawned. Handlers are indivisible: the scheduler never
//!    pretends to preempt one; it only bounds how many run at once.
//! 3. **Divisible work driver** ([`SchedulingLayer::run_slices`]) — for caller
//!    code with explicit safe yield points; the only place where the adaptive
//!    tree's per-round work quotas actually slice execution.
//!
//! Every guard is RAII: slot and memory registration are released exactly once on
//! normal completion, panic, timeout-abort, or future drop (cancellation), so no
//! path can leak capacity. Admission failures map to protocol-valid responses the
//! caller writes before any handler state exists.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aether_scheduler::{
    BlockedReason, JobId, JobState, NewJob, RejectReason, ResourceConfig, ResourceSnapshot,
    SchedulerEngine, SchedulerMetrics, SchedulingConfig, Work,
};
use tokio::sync::Notify;

/// Why admission did not produce a running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitFailure {
    QueueFull,
    MemoryBudget,
    TimedOut,
}

/// Outcome of one slice of divisible work: units actually executed, and whether
/// the job is finished.
#[derive(Debug, Clone, Copy)]
pub struct SliceProgress {
    pub work: Work,
    pub done: bool,
}

impl SliceProgress {
    #[must_use]
    pub fn progressed(work: Work) -> Self {
        Self { work, done: false }
    }

    #[must_use]
    pub fn finished(work: Work) -> Self {
        Self { work, done: true }
    }
}

/// Per-socket overhead a connection is charged against the memory budget while it holds
/// nothing. The bytes a socket actually buffers are charged on top of this as they
/// arrive, so an idle keep-alive connection no longer reserves the largest body it could
/// ever reach for the whole life of the connection.
pub const CONN_BASE_MEMORY: u64 = 32 * 1024;

/// The read buffer every HTTP/1 connection allocates before it has read a byte. It is part of
/// what [`CONN_BASE_MEMORY`] pays for, so the buffered charge a connection carries is its
/// buffer's *capacity* minus this figure — the allocator hands out a block that doubles as a
/// body grows, and reserving 2 MiB to hold a 1.5 MiB body is 2 MiB of RSS, not 1.5 MiB.
/// Kept beside the allocation in `app.rs` so the two cannot drift apart.
pub const CONN_BUF_INITIAL_CAPACITY: usize = 8192;

/// The write buffer every HTTP/1 connection allocates before it has serialized a response.
/// Like [`CONN_BUF_INITIAL_CAPACITY`] it is inside what [`CONN_BASE_MEMORY`] pays for, so a
/// connection that has only served small responses carries no write charge; the charge is the
/// block a large response made it reserve, which `clear()` never shrinks.
pub const WRITE_BUF_INITIAL_CAPACITY: usize = 4096;

/// What one entry of a request's [`Extensions`](crate::extensions::Extensions) map costs the
/// allocator apart from the value stored in it, charged on top of
/// [`size_of`] of the value itself.
///
/// Measured 2026-10-26 by a counting allocator around the map shape the framework actually builds
/// (`Option<Box<HashMap<TypeId, Entry>>>`, `Entry` being the boxed value plus its two `u64` price
/// fields, 32 bytes): the first entry costs 264 bytes (the 48-byte boxed map plus its hashbrown table
/// and first slot) and each further entry ~148 bytes marginal — 16 entries cost 2 492 bytes, with 2, 4
/// and 8 at 268, 684 and 1 288. 160 rounds that marginal cost up, because over-pricing a container is
/// safe and under-pricing it is what these constants exist to avoid. The residual is a one-time ~104
/// bytes on the first entry of a map (264 − 160), constant per request and stated rather than smoothed
/// away. `framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs` is the probe that produced
/// that table; it measures the pre-pricing shape (`HashMap<TypeId, Box<dyn Any + Send + Sync>>`, 16
/// bytes less per slot) in the same run — 200 bytes for its first entry, 1 724 for 16, ~101 marginal —
/// which is where the earlier draft of this constant (128) came from and is why it was raised: a
/// container price belongs to the shape that ships, not to the one that was convenient to measure.
pub const EXTENSION_ENTRY_MEMORY: u64 = 160;

/// Read-only view of why the queue is not draining. Obtained from
/// [`SchedulingLayer::queue_diag`]; computing it changes no scheduling decision.
///
/// `admissible_now` is the number that matters when the server looks stuck: queued
/// work the engine could promote but has not reached yet (the admission walk scans a
/// bounded window per pump). If it stays non-zero while `completed` does not move,
/// the walk is the problem; if everything is blocked on memory, the reservation
/// budget is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueDiag {
    pub queued: usize,
    pub memory_available: u64,
    pub blocked_on_memory: usize,
    pub blocked_on_slots: usize,
    pub admissible_now: usize,
}

/// Shared scheduling layer for one server. Cheap to clone behind an `Arc`.
pub struct SchedulingLayer {
    engine: Mutex<SchedulerEngine>,
    notify: Notify,
    epoch: Instant,
    queue_wait: Duration,
}

/// RAII admission token. Released as `complete` (explicit) or `cancel` (drop ⇒
/// disconnect, timeout-abort, panic, or shutdown) exactly once.
pub struct AdmissionGuard {
    layer: Arc<SchedulingLayer>,
    id: JobId,
    finished: bool,
}

impl SchedulingLayer {
    #[must_use]
    pub fn new(cfg: SchedulingConfig, rcfg: ResourceConfig) -> Arc<Self> {
        let queue_wait = cfg.max_queue_wait;
        Arc::new(Self {
            engine: Mutex::new(SchedulerEngine::new(cfg, rcfg)),
            notify: Notify::new(),
            epoch: Instant::now(),
            queue_wait,
        })
    }

    #[must_use]
    pub fn policy_name(&self) -> &'static str {
        self.lock().policy_name()
    }

    /// Live counters: admissions, rejections, starvation promotions, class moves.
    #[must_use]
    pub fn metrics(&self) -> SchedulerMetrics {
        self.lock().metrics().clone()
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.lock().resources()
    }

    /// Why the queue is not draining right now. Walks the whole queued set, so this
    /// is a diagnostic for a monitor thread or a debugger, not a hot path.
    #[must_use]
    pub fn queue_diag(&self) -> QueueDiag {
        let e = self.lock();
        let mut d = QueueDiag {
            queued: e.queued_len(),
            memory_available: e.memory_available(),
            blocked_on_memory: 0,
            blocked_on_slots: 0,
            admissible_now: 0,
        };
        for id in e.queued_ids() {
            match e.blocked_reason(id) {
                Some(BlockedReason::MemoryBudget) => d.blocked_on_memory += 1,
                Some(BlockedReason::SlotsFull) => d.blocked_on_slots += 1,
                None => d.admissible_now += 1,
            }
        }
        d
    }

    /// Current stable baseline (AdaptiveTree); `default_baseline` otherwise.
    #[must_use]
    pub fn baseline(&self) -> Work {
        self.lock().baseline()
    }

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn lock(&self) -> MutexGuard<'_, SchedulerEngine> {
        // The engine is pure sync code that never panics internally; a foreign
        // panic while locked must not brick the server (fail-open policy, design §8).
        self.engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pump_and_state(&self, id: JobId) -> Option<JobState> {
        let mut e = self.lock();
        e.pump(self.now());
        e.job_state(id)
    }

    /// Re-price a live job's memory registration. A shrink gives the freed bytes to the
    /// queue exactly like a release does (pump under the lock, broadcast outside it);
    /// a grow that the budget cannot cover returns `false` and changes nothing.
    pub(crate) fn adjust_memory(&self, id: JobId, delta: i64) -> bool {
        let mut freed = false;
        let ok = {
            let mut e = self.lock();
            let ok = e.adjust_memory(id, delta);
            if ok && delta < 0 {
                e.pump(self.now());
                freed = true;
            }
            ok
        };
        if freed {
            self.notify.notify_waiters();
        }
        ok
    }

    fn release(&self, id: JobId, completed: bool) {
        {
            let mut e = self.lock();
            let now = self.now();
            if completed {
                e.complete(id, now);
            } else {
                e.cancel(id, now);
            }
            // Freed capacity: give the oldest eligible waiter its shot immediately.
            e.pump(now);
        }
        self.notify.notify_waiters();
    }

    /// Admit a job and wait (bounded by `SchedulingConfig::max_queue_wait`) until
    /// the engine marks it Active. Queue overflow and impossible-fit jobs fail
    /// fast; wait expiry cancels the queued job so nothing lingers. Dropping the
    /// returned future while still queued is also safe: a `QueueToken` cancels
    /// the job exactly once, so aborts cannot leave orphaned queue entries.
    pub async fn admit(self: &Arc<Self>, job: NewJob) -> Result<AdmissionGuard, AdmitFailure> {
        let (id, active) = {
            let mut e = self.lock();
            let now = self.now();
            let id = e.admit(job, now).map_err(|r| match r {
                RejectReason::QueueFull => AdmitFailure::QueueFull,
                RejectReason::MemoryBudget => AdmitFailure::MemoryBudget,
            })?;
            e.pump(now);
            // Read under the lock that admitted: no other holder can see this id
            // yet, so its state cannot change between the pump and this read.
            (id, e.job_state(id) == Some(JobState::Active))
        };
        // Covers the window between engine admission and AdmissionGuard creation.
        let mut token = QueueToken {
            layer: Arc::clone(self),
            id,
            armed: true,
        };
        if active {
            token.disarm();
            return Ok(self.guard(id));
        }
        let deadline = Instant::now() + self.queue_wait;
        loop {
            // Register the wakeup BEFORE re-pumping so a release racing with the
            // check cannot be missed.
            let wait = self.notify.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            match self.pump_and_state(id) {
                Some(JobState::Active) => {
                    token.disarm();
                    return Ok(self.guard(id));
                }
                None => {
                    token.disarm();
                    return Err(AdmitFailure::QueueFull);
                }
                _ => {}
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                self.lock().cancel(id, self.now());
                token.disarm();
                return Err(AdmitFailure::TimedOut);
            };
            if tokio::time::timeout(remaining, wait).await.is_err() {
                self.lock().cancel(id, self.now());
                token.disarm();
                return Err(AdmitFailure::TimedOut);
            }
        }
    }

    fn guard(self: &Arc<Self>, id: JobId) -> AdmissionGuard {
        AdmissionGuard {
            layer: Arc::clone(self),
            id,
            finished: false,
        }
    }

    /// Divisible-work driver: runs `f` one granted slice at a time, yielding
    /// between slices. `f` must return quickly enough to keep its slice honest —
    /// measured wall time feeds the quota adaptation. The job is cancelled if the
    /// driver future itself is dropped.
    pub async fn run_slices<F, Fut>(
        self: &Arc<Self>,
        work_hint: Option<Work>,
        memory_bytes: u64,
        mut f: F,
    ) -> Result<Work, AdmitFailure>
    where
        F: FnMut(Work) -> Fut,
        Fut: std::future::Future<Output = SliceProgress>,
    {
        let guard = self
            .admit(NewJob::divisible(work_hint, memory_bytes))
            .await?;
        let id = guard.id;
        let mut executed: Work = 0;
        loop {
            let grant = {
                let mut e = self.lock();
                let now = self.now();
                e.pump(now)
                    .grants
                    .into_iter()
                    .find(|g| g.job == id)
                    .map(|g| g.quota)
            };
            let Some(quota) = grant.filter(|q| *q > 0) else {
                let wait = self.notify.notified();
                tokio::pin!(wait);
                wait.as_mut().enable();
                // Safety poll bound: grants can also come from time-driven
                // re-evaluation, not only from releases.
                let _ = tokio::time::timeout(Duration::from_millis(25), wait).await;
                continue;
            };
            let started = Instant::now();
            let progress = f(quota).await;
            let elapsed = started.elapsed();
            executed = executed.saturating_add(progress.work);
            self.lock()
                .report_slice(id, progress.work, elapsed, self.now());
            if progress.done {
                break;
            }
        }
        guard.complete();
        Ok(executed)
    }
}

impl AdmissionGuard {
    /// The job succeeded; releases capacity and lets the next waiter in.
    pub fn complete(mut self) {
        self.finished = true;
        self.layer.release(self.id, true);
    }

    /// Re-price this job's registered memory while it runs: charge `delta` more, or
    /// give `delta` back when negative. Returns `false` when the budget cannot cover
    /// the increase — the caller must shed the work it was about to absorb — or when
    /// the job is no longer running. Never changes a scheduling decision on its own.
    pub fn adjust_memory(&self, delta: i64) -> bool {
        if self.finished {
            return false;
        }
        self.layer.adjust_memory(self.id, delta)
    }

    /// Explicit cancellation (client gone, shutdown): same release path, counted
    /// separately in metrics and never fed to baseline samples.
    pub fn cancel(mut self) {
        self.finished = true;
        self.layer.release(self.id, false);
    }

    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if !self.finished {
            // Panic, abort, or future-drop: cancel exactly once.
            self.layer.release(self.id, false);
        }
    }
}

/// RAII containment for the waiting window of [`SchedulingLayer::admit`]: if the
/// future is dropped (task abort) before an `AdmissionGuard` exists, the queued
/// job is cancelled exactly once so the queue can never accumulate orphans.
struct QueueToken {
    layer: Arc<SchedulingLayer>,
    id: JobId,
    armed: bool,
}

impl QueueToken {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for QueueToken {
    fn drop(&mut self) {
        if self.armed {
            self.layer.release(self.id, false);
        }
    }
}
