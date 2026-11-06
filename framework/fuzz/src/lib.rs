//! # Deterministic HTTP parser fuzz campaign
//!
//! `cargo-fuzz` needs a nightly toolchain and an ASan runtime; neither is available on the
//! platform this repository is developed on. This crate therefore ships its own runner:
//!
//! ```text
//! cargo run -p aether-fuzz --release -- --seconds 120 --seed 20261030
//! ```
//!
//! It mutates a committed seed corpus with a seeded SplitMix64 generator, runs every input
//! through [`exercise`] inside `catch_unwind`, and keeps inputs that reach a new outcome
//! bucket so later mutations splice from them. A panic is written to `fuzz/artifacts/` and
//! ends the campaign with a non-zero exit, the way a sanitizer crash ends a libFuzzer run.
//! An artifact replays as a seed: copy it into `fuzz/corpus/http_parse/` and the campaign
//! exercises every seed before it mutates anything.
//!
//! Two limits are stated rather than implied. First, there is no coverage instrumentation:
//! a "bucket" is a hash of the parse outcomes (found / incomplete / refused, consumed length,
//! message length), so this is outcome-guided fuzzing, not coverage-guided fuzzing. Second,
//! a panic that is *not* accompanied by memory corruption is what this catches; unsafe code
//! and allocator misuse still need ASan (see the register entry for track 102).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aether_http::limits::ServerLimits;
use aether_http::parser::{
    ChunkedBodyDecoder, RequestView, awaits_100_continue, parse_request, parse_request_head,
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn mix(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(FNV_PRIME);
}

/// A logarithmic size class. Linear buckets made almost every mutated length its own outcome,
/// which filled the corpus with near-duplicates in seconds; what actually distinguishes one
/// parse from another is the order of magnitude of how much it consumed.
fn scale(n: usize) -> u64 {
    u64::from(usize::BITS - n.max(1).leading_zeros())
}

/// SplitMix64: tiny, deterministic, and good enough to spread mutations across a corpus.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // A zero state is a fixed point of the generator.
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }
}

/// Run every parser entry point over `data` and return a hash of the outcome shape.
///
/// The asserts here are the fuzz oracle: they state relationships the parser promises for
/// *any* input, and a violated one aborts the campaign with the input saved.
pub fn exercise(data: &[u8]) -> u64 {
    let limits = ServerLimits::default();
    let mut hash = FNV_OFFSET;

    // The Content-Length-only entry point.
    match RequestView::parse(data, &limits) {
        Ok(None) => mix(&mut hash, 1),
        Ok(Some((view, consumed))) => {
            assert!(
                consumed <= data.len(),
                "consumed more bytes than were given"
            );
            assert_eq!(
                view.path.len() + view.query_str.map_or(0, |q| q.len() + 1),
                view.raw_uri.len(),
                "the request target split lost or invented bytes"
            );
            mix(&mut hash, 2);
            mix(&mut hash, scale(consumed));
        }
        Err(_) => mix(&mut hash, 3),
    }

    // The streaming head and the buffered runtime path.
    match parse_request_head(data, &limits) {
        Ok(Some(head)) => {
            assert!(head.header_len <= data.len());
            assert!(head.view.body.is_empty());
            // No declared-length ceiling is asserted here on purpose: this entry point applies
            // none, because which ceiling a `Content-Length` must fit is the receiving route's
            // decision and is taken after the head parses. The buffered entry point below still
            // applies `max_body_bytes` to the same bytes.
            mix(&mut hash, 10);
            mix(&mut hash, scale(head.header_len));
        }
        Ok(None) => mix(&mut hash, 11),
        Err(_) => mix(&mut hash, 12),
    }

    match parse_request(data, &limits) {
        Ok(Some((view, owned, consumed))) => {
            assert!(consumed <= data.len());
            if owned.is_some() {
                assert!(
                    view.body.is_empty(),
                    "a decoded body must not also be borrowed"
                );
            }
            mix(&mut hash, 20);
            mix(&mut hash, scale(consumed));
        }
        Ok(None) => mix(&mut hash, 21),
        Err(_) => mix(&mut hash, 22),
    }

    let _ = awaits_100_continue(data, &limits);

    // Feed the same bytes to the chunked decoder in steps derived from the bytes themselves,
    // so a crash is reproducible from the artifact alone.
    let mut decoder = ChunkedBodyDecoder::new();
    let mut out: Vec<u8> = Vec::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let step = 1 + (data[offset] as usize % 9);
        let end = (offset + step).min(data.len());
        buffer.extend_from_slice(&data[offset..end]);
        offset = end;
        match decoder.stream_feed(&buffer, &limits, &mut out) {
            Ok(consumed) => {
                assert!(consumed <= buffer.len());
                buffer.drain(..consumed);
            }
            Err(_) => break,
        }
        if decoder.is_complete() {
            break;
        }
    }
    assert!(
        out.len() <= data.len(),
        "the decoder produced more bytes than it was given"
    );
    mix(&mut hash, 30);
    mix(&mut hash, scale(out.len()));
    mix(&mut hash, decoder.is_complete() as u64);

    mix(&mut hash, scale(data.len()));
    hash
}

/// The documented single-input entry point.
pub fn fuzz_target(data: &[u8]) {
    let _ = exercise(data);
}

// ---------------------------------------------------------------------------------------------
// Corpus and mutation
// ---------------------------------------------------------------------------------------------

pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join("http_parse")
}

fn read_corpus(dir: &Path) -> Vec<Vec<u8>> {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    let corpus: Vec<Vec<u8>> = paths.iter().filter_map(|p| std::fs::read(p).ok()).collect();
    if corpus.is_empty() {
        panic!(
            "no seed corpus under {}; commit at least one seed before running a campaign",
            dir.display()
        );
    }
    corpus
}

/// One mutation of `input`, drawn from `corpus` for splices and capped at `max_len`.
pub fn mutate(rng: &mut Rng, input: &[u8], corpus: &[Vec<u8>], max_len: usize) -> Vec<u8> {
    let mut out = input.to_vec();
    let rounds = 1 + rng.below(8) as usize;
    for _ in 0..rounds {
        if out.is_empty() {
            out.push(rng.byte());
            continue;
        }
        match rng.below(8) {
            0 => {
                let at = rng.below(out.len() as u64) as usize;
                out[at] ^= 1 << rng.below(8);
            }
            1 => {
                let at = rng.below(out.len() as u64) as usize;
                out[at] = rng.byte();
            }
            2 => {
                let at = rng.below(out.len() as u64 + 1) as usize;
                out.insert(at, rng.byte());
            }
            3 => {
                let at = rng.below(out.len() as u64) as usize;
                let len = 1 + rng.below(16) as usize;
                let end = (at + len).min(out.len());
                out.drain(at..end);
            }
            4 => {
                if out.len() >= 2 {
                    let at = rng.below(out.len() as u64) as usize;
                    let len = 1 + rng.below((out.len() - at) as u64) as usize;
                    let block: Vec<u8> = out[at..at + len].to_vec();
                    let to = rng.below(out.len() as u64 + 1) as usize;
                    out.splice(to..to, block);
                }
            }
            5 => {
                let len = rng.below(out.len() as u64 + 1) as usize;
                out.truncate(len);
            }
            6 => {
                let donor = &corpus[rng.below(corpus.len() as u64) as usize];
                if !donor.is_empty() {
                    let from = rng.below(donor.len() as u64) as usize;
                    let take = 1 + rng.below((donor.len() - from) as u64) as usize;
                    let at = rng.below(out.len() as u64 + 1) as usize;
                    out.splice(at..at, donor[from..from + take].iter().copied());
                }
            }
            _ => {
                // Splice a structural byte in, so mutation can create CRLFs a byte flip cannot.
                let at = rng.below(out.len() as u64 + 1) as usize;
                const STRUCTURAL: &[u8] = b"\r\n:;/?=&*{}";
                let byte = STRUCTURAL[rng.below(STRUCTURAL.len() as u64) as usize];
                out.insert(at, byte);
            }
        }
        if out.len() > max_len {
            out.truncate(max_len);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Campaign
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    /// Wall-clock budget. The loop only consults the clock every few thousand executions.
    pub seconds: f64,
    /// Stop after this many executions; 0 means "only the clock stops it".
    pub max_iterations: u64,
    pub seed: u64,
    pub max_input_len: usize,
    /// Ceiling on the in-memory corpus, so an outcome-rich input stream cannot grow it forever.
    pub corpus_cap: usize,
    pub heartbeat: Option<Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seconds: 60.0,
            max_iterations: 0,
            seed: 0x51ee_d1ce_2026_1030,
            max_input_len: 8 * 1024,
            corpus_cap: 4096,
            heartbeat: Some(Duration::from_secs(10)),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub seed: u64,
    pub iterations: u64,
    pub elapsed: Duration,
    pub initial_corpus: usize,
    pub corpus: usize,
    pub buckets: usize,
    pub crashes: Vec<PathBuf>,
}

impl Report {
    pub fn iterations_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs == 0.0 {
            0.0
        } else {
            self.iterations as f64 / secs
        }
    }

    pub fn print(&self) {
        println!(
            "seed={} iterations={} elapsed={:.3}s rate={:.0}/s corpus={}->{} buckets={} crashes={}",
            self.seed,
            self.iterations,
            self.elapsed.as_secs_f64(),
            self.iterations_per_second(),
            self.initial_corpus,
            self.corpus,
            self.buckets,
            self.crashes.len()
        );
        for crash in &self.crashes {
            println!("crash artifact: {}", crash.display());
        }
    }
}

fn artifact_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts")
}

fn save_crash(input: &[u8], seed: u64, iteration: u64) -> PathBuf {
    let dir = artifact_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("crash-seed{seed}-iter{iteration}.bin"));
    let _ = std::fs::write(&path, input);
    path
}

/// Run one campaign against the committed corpus.
pub fn campaign(config: &Config) -> Report {
    let corpus_dir = corpus_dir();
    let mut corpus = read_corpus(&corpus_dir);
    let mut rng = Rng::new(config.seed);
    let mut buckets: HashSet<u64> = HashSet::new();
    let mut report = Report {
        seed: config.seed,
        initial_corpus: corpus.len(),
        corpus: corpus.len(),
        ..Report::default()
    };

    // The committed seeds themselves must pass: a broken seed is a broken campaign.
    for seed in &corpus {
        let bucket = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(seed)))
            .unwrap_or_else(|_| {
                let path = save_crash(seed, config.seed, u64::MAX);
                report.crashes.push(path);
                u64::MAX
            });
        buckets.insert(bucket);
    }

    let start = Instant::now();
    let mut last_heartbeat = start;
    while report.crashes.is_empty() {
        if config.max_iterations != 0 && report.iterations >= config.max_iterations {
            break;
        }
        if report.iterations.is_multiple_of(4096) && start.elapsed().as_secs_f64() >= config.seconds
        {
            break;
        }

        let base = &corpus[rng.below(corpus.len() as u64) as usize];
        let input = mutate(&mut rng, base, &corpus, config.max_input_len);
        report.iterations += 1;

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&input))) {
            Ok(bucket) => {
                if !buckets.contains(&bucket) && corpus.len() < config.corpus_cap {
                    buckets.insert(bucket);
                    corpus.push(input.clone());
                }
            }
            Err(_) => {
                let path = save_crash(&input, config.seed, report.iterations);
                report.crashes.push(path);
            }
        }

        if let Some(period) = config.heartbeat
            && last_heartbeat.elapsed() >= period
        {
            eprintln!(
                "… {} executions, {:.1}s, corpus {}, {} buckets",
                report.iterations,
                start.elapsed().as_secs_f64(),
                corpus.len(),
                buckets.len()
            );
            last_heartbeat = Instant::now();
        }
    }

    report.elapsed = start.elapsed();
    report.corpus = corpus.len();
    report.buckets = buckets.len();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_bounded() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        for bound in [1u64, 2, 7, 64, 4096] {
            let mut rng = Rng::new(bound);
            for _ in 0..1000 {
                assert!(rng.below(bound) < bound);
            }
        }
    }

    #[test]
    fn every_committed_seed_passes_exercise() {
        let corpus = read_corpus(&corpus_dir());
        assert!(!corpus.is_empty());
        for seed in &corpus {
            let _ = exercise(seed);
        }
    }

    #[test]
    fn a_mutation_never_exceeds_the_input_cap() {
        let corpus = read_corpus(&corpus_dir());
        let mut rng = Rng::new(99);
        let mut input = corpus[0].clone();
        for _ in 0..2000 {
            input = mutate(&mut rng, &input, &corpus, 256);
            assert!(input.len() <= 256);
        }
    }

    #[test]
    fn a_bounded_campaign_runs_its_iterations_without_a_crash() {
        let config = Config {
            seconds: 1.0,
            max_iterations: 2000,
            seed: 5,
            heartbeat: None,
            ..Config::default()
        };
        let report = campaign(&config);
        assert_eq!(report.iterations, 2000);
        assert!(report.crashes.is_empty());
        assert!(report.buckets >= 1);
    }
}
