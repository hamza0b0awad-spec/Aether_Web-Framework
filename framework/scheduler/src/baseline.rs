use std::collections::VecDeque;

use crate::config::SchedulingConfig;
use crate::job::{JobClass, Work};

/// Robust stable-baseline estimator (design §4.1).
///
/// `B` is the median of the last `sample_window` completed non-large jobs. Large
/// jobs never enter the sample, so one huge job cannot destabilize `B`, and the
/// median additionally discards outliers inside the sample. Before `min_samples`
/// completions the configured `default_baseline` is used.
#[derive(Debug, Clone)]
pub struct Baseline {
    samples: VecDeque<Work>,
    window: usize,
    min_samples: usize,
    default_baseline: Work,
    hi: f64,
    lo: f64,
}

impl Baseline {
    #[must_use]
    pub fn from_cfg(cfg: &SchedulingConfig) -> Self {
        Self {
            samples: VecDeque::new(),
            window: cfg.sample_window.max(1),
            // Zero would let `value()` index an empty sample set (see `value` below).
            min_samples: cfg.min_samples.max(1),
            default_baseline: cfg.default_baseline.max(1),
            hi: cfg.large_factor_hi,
            lo: cfg.large_factor_lo,
        }
    }

    /// Record a completed non-large job. Large completions must never be passed here.
    pub fn observe_completed(&mut self, work: Work) {
        self.samples.push_back(work.max(1));
        while self.samples.len() > self.window {
            self.samples.pop_front();
        }
    }

    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn value(&self) -> Work {
        if self.samples.len() < self.min_samples {
            return self.default_baseline;
        }
        let mut v: Vec<Work> = self.samples.iter().copied().collect();
        v.sort_unstable();
        let mid = v.len() / 2;
        if v.len() % 2 == 1 {
            v[mid]
        } else {
            // The offset form is exact and overflow-free: `(a + b) / 2` wraps for
            // large work values, and saturating it would bias the median upward.
            let (low, high) = (v[mid - 1], v[mid]);
            low + (high - low) / 2
        }
        .max(1)
    }

    /// Classify `measured` work against the current baseline with hysteresis:
    /// crossing into Large needs `> hi × B`, crossing out needs `< lo × B`. Inside
    /// the dead band the previous class sticks, so tiny baseline movements cannot
    /// flap a job between classes.
    #[must_use]
    pub fn classify(&self, measured: Work, prev: JobClass) -> JobClass {
        let b = self.value().max(1) as f64;
        let hi = (b * self.hi).floor() as Work;
        let lo = (b * self.lo).floor() as Work;
        if measured > hi {
            JobClass::Large
        } else if measured < lo {
            JobClass::Small
        } else if prev == JobClass::Unknown {
            // Inside the band with no history: keep it unknown (capped shares),
            // it converges once executed work crosses either edge.
            JobClass::Unknown
        } else {
            prev
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(min_samples: usize) -> SchedulingConfig {
        SchedulingConfig {
            min_samples,
            ..SchedulingConfig::default()
        }
    }

    /// `min_samples: 0` is a legal configuration and must not index an empty sample set
    /// inside the engine mutex, where a panic would take down every admission decision.
    #[test]
    fn zero_min_samples_falls_back_instead_of_indexing_an_empty_set() {
        let b = Baseline::from_cfg(&cfg_with(0));
        assert_eq!(b.sample_count(), 0);
        assert_eq!(
            b.value(),
            SchedulingConfig::default().default_baseline.max(1)
        );
    }

    #[test]
    fn median_of_an_even_sample_set_averages_the_two_centres() {
        let mut b = Baseline::from_cfg(&SchedulingConfig::default());
        for w in [10 as Work, 20, 30, 1000] {
            b.observe_completed(w);
        }
        assert_eq!(b.value(), 25);
    }

    #[test]
    fn median_does_not_overflow_on_extreme_samples() {
        let mut b = Baseline::from_cfg(&SchedulingConfig::default());
        for _ in 0..4 {
            b.observe_completed(Work::MAX);
        }
        assert_eq!(b.value(), Work::MAX);
    }
}
