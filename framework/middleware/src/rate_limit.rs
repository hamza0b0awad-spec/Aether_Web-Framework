//! Per-client flood mitigation (OWNER_VISION §5: Slowloris & Flood Mitigation).
//!
//! A token bucket per client, refilled continuously rather than in fixed windows. The
//! difference matters under real traffic: a window counter lets a client spend its whole
//! allowance in the last millisecond of one window and again in the first of the next, so
//! the true peak rate is double the configured one. A bucket caps the burst exactly at
//! `burst` and caps the sustained rate at `burst / per`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A single request slot refills after `per`; a client that has been idle can therefore
/// spend up to `burst` requests back to back.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub burst: u32,
    pub per: Duration,
    /// Distinct clients tracked before the idle slots are dropped. The map is keyed by
    /// source address, so a client pool that sprays addresses can grow it; this bound
    /// keeps that growth finite.
    pub max_clients: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        // A flood ceiling, not a fairness quota: 100 requests back to back and 100 req/s
        // sustained per client is far above what a browser session produces, so the
        // defaults cannot silently reject real traffic while still capping an abusive
        // client at a bounded rate.
        Self {
            burst: 100,
            per: Duration::from_millis(10),
            max_clients: 65_536,
        }
    }
}

/// Whether the next request from a client is admitted, and how long it must wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    /// `retry_after` is never zero: a client that is told to wait gets a real interval.
    Limited {
        retry_after: Duration,
    },
}

/// What the limiter has done since creation, for operators and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimitCounters {
    pub limited: u64,
    /// Requests admitted because the tracked-client bound was hit even after the idle
    /// sweep. Failing open is deliberate: rejecting everyone behind a shared NAT or a
    /// proxy pool because an attacker sprayed source addresses would turn mitigation into
    /// self-inflicted denial.
    pub failed_open: u64,
}

struct Slot {
    tokens: u32,
    /// When the last spent token was paid for. Advancing this by exactly `per` per token,
    /// instead of stamping `now`, keeps the sub-period remainder so rounding cannot hand
    /// out free tokens.
    last: Instant,
}

/// Shared, thread-safe token bucket keyed by client address.
pub struct RateLimiter {
    config: RateLimitConfig,
    slots: Mutex<HashMap<IpAddr, Slot>>,
    limited: AtomicU64,
    failed_open: AtomicU64,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            // Sized lazily: an empty limiter must not reserve its client bound up front.
            slots: Mutex::new(HashMap::new()),
            limited: AtomicU64::new(0),
            failed_open: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    pub fn counters(&self) -> RateLimitCounters {
        RateLimitCounters {
            limited: self.limited.load(Ordering::Relaxed),
            failed_open: self.failed_open.load(Ordering::Relaxed),
        }
    }

    /// Admit or defer one request from `client`.
    ///
    /// `now` is a parameter rather than an internal `Instant::now()`, because the refill
    /// arithmetic is the part worth testing and a clock-driven test would have to sleep to
    /// prove a `per` of one millisecond.
    pub fn check(&self, client: IpAddr, now: Instant) -> RateLimitDecision {
        // A `per` of zero would divide by zero; clamping keeps a misconfiguration loud in
        // the limits rather than a panic in the request path.
        let per = self.config.per.max(Duration::from_micros(1));
        let burst = self.config.burst.max(1);
        let mut slots = lock(&self.slots);

        if let Some(slot) = slots.get_mut(&client) {
            let elapsed = now.saturating_duration_since(slot.last);
            let periods = (elapsed.as_micros() / per.as_micros()).min(u64::MAX as u128) as u64;
            if periods > 0 {
                let projected = u64::from(slot.tokens).saturating_add(periods);
                if projected >= u64::from(burst) {
                    // Idle longer than a full refill: the surplus is not banked, so the
                    // stamp resyncs and a long quiet period cannot pre-pay the next burst.
                    slot.tokens = burst;
                    slot.last = now;
                } else {
                    slot.tokens = projected as u32;
                    slot.last = slot
                        .last
                        .checked_add(per.saturating_mul(periods as u32))
                        .unwrap_or(now);
                }
            }
            if slot.tokens > 0 {
                slot.tokens -= 1;
                return RateLimitDecision::Allowed;
            }
            self.limited.fetch_add(1, Ordering::Relaxed);
            return RateLimitDecision::Limited {
                retry_after: per
                    .saturating_sub(now.saturating_duration_since(slot.last))
                    .max(Duration::from_micros(1)),
            };
        }

        // First sight of this client: this request spends one of its tokens.
        if slots.len() >= self.config.max_clients {
            let idle_for = per.saturating_mul(burst.saturating_mul(2));
            slots.retain(|_, slot| now.saturating_duration_since(slot.last) < idle_for);
            if slots.len() >= self.config.max_clients {
                self.failed_open.fetch_add(1, Ordering::Relaxed);
                return RateLimitDecision::Allowed;
            }
        }
        slots.insert(
            client,
            Slot {
                tokens: burst - 1,
                last: now,
            },
        );
        RateLimitDecision::Allowed
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("config", &self.config)
            .field("tracked_clients", &lock(&self.slots).len())
            .field("counters", &self.counters())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(burst: u32, per_ms: u64, max_clients: usize) -> RateLimiter {
        RateLimiter::new(RateLimitConfig {
            burst,
            per: Duration::from_millis(per_ms),
            max_clients,
        })
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn admits_the_burst_then_limits() {
        let l = limiter(3, 1000, 16);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(l.check(ip(1), t0), RateLimitDecision::Allowed);
        }
        assert_eq!(
            l.check(ip(1), t0),
            RateLimitDecision::Limited {
                retry_after: Duration::from_millis(1000)
            }
        );
    }

    #[test]
    fn refills_one_token_per_period_and_never_over_the_burst() {
        let l = limiter(2, 1000, 16);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(2), t0), RateLimitDecision::Allowed);
        assert_eq!(l.check(ip(2), t0), RateLimitDecision::Allowed);
        assert!(matches!(
            l.check(ip(2), t0 + Duration::from_millis(999)),
            RateLimitDecision::Limited { .. }
        ));
        // Exactly one period buys exactly one token — not the whole burst.
        assert_eq!(
            l.check(ip(2), t0 + Duration::from_millis(1000)),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            l.check(ip(2), t0 + Duration::from_millis(1000)),
            RateLimitDecision::Limited { .. }
        ));
        // Two periods buy two tokens, and the third request at that instant is refused:
        // the bucket never exceeds the burst.
        assert_eq!(
            l.check(ip(2), t0 + Duration::from_millis(3000)),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            l.check(ip(2), t0 + Duration::from_millis(3000)),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            l.check(ip(2), t0 + Duration::from_millis(3000)),
            RateLimitDecision::Limited { .. }
        ));
    }

    #[test]
    fn fractional_time_is_carried_instead_of_rounded_away() {
        // burst = 1, per = 900ms. After spending the token at t0 the client must wait a
        // whole period: stamping `last = now` on every refill decision would let elapsed
        // microseconds accumulate into an early token.
        let l = limiter(1, 900, 16);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(3), t0), RateLimitDecision::Allowed);
        for offset_ms in [1, 450, 899] {
            assert!(
                matches!(
                    l.check(ip(3), t0 + Duration::from_millis(offset_ms)),
                    RateLimitDecision::Limited { .. }
                ),
                "refilled early at {offset_ms}ms of a 900ms period"
            );
        }
        assert_eq!(
            l.check(ip(3), t0 + Duration::from_millis(900)),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn idle_time_beyond_the_burst_is_not_banked() {
        let l = limiter(2, 1000, 16);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(11), t0), RateLimitDecision::Allowed);
        assert_eq!(l.check(ip(11), t0), RateLimitDecision::Allowed);
        // An hour later the client has its burst, and no more than its burst.
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(l.check(ip(11), later), RateLimitDecision::Allowed);
        assert_eq!(l.check(ip(11), later), RateLimitDecision::Allowed);
        assert!(matches!(
            l.check(ip(11), later),
            RateLimitDecision::Limited { .. }
        ));
    }

    #[test]
    fn clients_are_isolated() {
        let l = limiter(1, 1000, 16);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(4), t0), RateLimitDecision::Allowed);
        assert!(matches!(
            l.check(ip(4), t0),
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(l.check(ip(5), t0), RateLimitDecision::Allowed);
    }

    #[test]
    fn retry_after_never_promises_zero() {
        let l = limiter(1, 10, 16);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(6), t0), RateLimitDecision::Allowed);
        for _ in 0..5 {
            if let RateLimitDecision::Limited { retry_after } = l.check(ip(6), t0) {
                assert!(!retry_after.is_zero());
            }
        }
    }

    #[test]
    fn a_dead_client_stops_taking_space() {
        let l = limiter(2, 1000, 2);
        let t0 = Instant::now();
        for n in 1..=2 {
            assert_eq!(l.check(ip(n), t0), RateLimitDecision::Allowed);
        }
        // The bound is reached and both slots are still recent, so nothing is evicted and
        // the limiter fails open rather than dropping an active client or rejecting
        // everyone.
        assert_eq!(l.check(ip(9), t0), RateLimitDecision::Allowed);
        assert_eq!(
            l.counters(),
            RateLimitCounters {
                limited: 0,
                failed_open: 1
            }
        );

        // Long after every slot expired, the sweep makes room and tracking resumes.
        assert_eq!(
            l.check(ip(10), t0 + Duration::from_millis(5000)),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            l.counters().failed_open,
            1,
            "the sweep should have freed space"
        );
    }

    #[test]
    fn limits_are_counted() {
        let l = limiter(1, 1000, 4);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(7), t0), RateLimitDecision::Allowed);
        for _ in 0..2 {
            assert!(matches!(
                l.check(ip(7), t0),
                RateLimitDecision::Limited { .. }
            ));
        }
        assert_eq!(l.counters().limited, 2);
    }

    #[test]
    fn a_zero_period_cannot_panic_on_division() {
        let l = RateLimiter::new(RateLimitConfig {
            burst: 2,
            per: Duration::ZERO,
            max_clients: 4,
        });
        let t0 = Instant::now();
        assert_eq!(l.check(ip(8), t0), RateLimitDecision::Allowed);
        assert!(matches!(
            l.check(ip(8), t0),
            RateLimitDecision::Allowed | RateLimitDecision::Limited { .. }
        ));
    }

    #[test]
    fn slots_survive_a_poisoned_lock() {
        // A panicking caller must not turn the limiter into a panic on every later
        // request; the guard recovers instead.
        let l = limiter(2, 1000, 4);
        let t0 = Instant::now();
        assert_eq!(l.check(ip(12), t0), RateLimitDecision::Allowed);
        drop(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || {
                let mut guard = lock(&l.slots);
                guard.insert(
                    ip(13),
                    Slot {
                        tokens: 1,
                        last: t0,
                    },
                );
                panic!("poison it");
            },
        )));
        assert_eq!(l.check(ip(12), t0), RateLimitDecision::Allowed);
        assert!(lock(&l.slots).contains_key(&ip(13)));
    }
}
