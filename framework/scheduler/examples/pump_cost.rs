//! Cost of one HTTP request's scheduling cycle as a function of live connection count.
//!
//! The serve path does exactly two engine passes per request: `admit` + `pump` when the
//! connection opens, and `complete` + `pump` when it closes. Each `pump` used to walk an
//! index that held every live socket, so this measures the per-request price of that walk
//! as a function of the connection count.
//!
//! Single-threaded on purpose: it isolates the work done *while holding* the global lock.
//! Contention between threads multiplies this number, it does not replace it.
//!
//!   cargo run --release -p aether-scheduler --example pump_cost

use std::time::{Duration, Instant};

use aether_scheduler::{NewJob, ResourceConfig, SchedulerEngine, SchedulingConfig};

/// One request, as the serve path performs it: open a connection job, promote it,
/// close it, and re-run the round that a close triggers.
fn request_cycle(e: &mut SchedulerEngine, now: Duration) {
    let id = e.admit(NewJob::connection(32 * 1024), now).unwrap();
    e.pump(now);
    e.complete(id, now);
    e.pump(now);
}

fn bench(
    name: &str,
    active: usize,
    reps: usize,
    mut f: impl FnMut(&mut SchedulerEngine, Duration),
) -> f64 {
    let mut e = SchedulerEngine::new(SchedulingConfig::default(), ResourceConfig::default());
    // Load the engine with `active` live connection jobs, promoted the same way the
    // server promotes them, so the state under test is the state production has.
    let mut keep = Vec::new();
    for i in 0..active {
        let id = e
            .admit(
                NewJob::connection(32 * 1024),
                Duration::from_micros(i as u64),
            )
            .unwrap();
        e.pump(Duration::from_micros(i as u64));
        keep.push(id);
    }
    assert_eq!(e.resources().active_connections, active, "load setup");

    // Warm-up so the allocations below are not first-touch dominated.
    for i in 0..(reps / 10).max(1) {
        f(&mut e, Duration::from_micros((active + i) as u64));
    }
    let started = Instant::now();
    let n = reps.max(1);
    for i in 0..n {
        f(&mut e, Duration::from_micros((active + i) as u64));
    }
    let total = started.elapsed();
    let per = total.as_nanos() as f64 / n as f64;
    println!(
        "{name:>34} active={active:<5} {per:>9.2} ns/op  ({:.2} us)",
        per / 1000.0
    );
    for id in keep {
        let _ = e.cancel(id, Duration::from_micros(active as u64));
    }
    per
}

fn main() {
    println!("cost of one request cycle (admit+pump+complete+pump) vs live connections\n");
    let reps = 200_000;
    let mut slope = Vec::new();
    for active in [1usize, 32, 64, 128, 256, 512, 1024] {
        let us = bench("request_cycle", active, reps, request_cycle) / 1000.0;
        slope.push((active, us));
        println!();
    }

    println!("--- breakdown at 256 live connections ---");
    let a = 256;
    let mut e = SchedulerEngine::new(SchedulingConfig::default(), ResourceConfig::default());
    let mut keep = Vec::new();
    for i in 0..a {
        let id = e
            .admit(
                NewJob::connection(32 * 1024),
                Duration::from_micros(i as u64),
            )
            .unwrap();
        e.pump(Duration::from_micros(i as u64));
        keep.push(id);
    }
    fn measure(
        e: &mut SchedulerEngine,
        label: &str,
        reps: usize,
        run: &mut dyn FnMut(&mut SchedulerEngine, Duration),
    ) {
        for i in 0..(reps / 10) {
            run(e, Duration::from_micros((256 + i) as u64));
        }
        let t = Instant::now();
        for i in 0..reps {
            run(e, Duration::from_micros((256 + i) as u64));
        }
        let per = t.elapsed().as_nanos() as f64 / reps as f64;
        println!("{label:>34} {per:>9.2} ns  ({:.2} us)", per / 1000.0);
    }
    measure(&mut e, "pump only", 200_000, &mut |e, n| {
        e.pump(n);
    });
    measure(&mut e, "admit + cancel (no pump)", 200_000, &mut |e, n| {
        let id = e.admit(NewJob::connection(32 * 1024), n).unwrap();
        let _ = e.cancel(id, n);
    });
    measure(&mut e, "complete of a fresh job", 200_000, &mut |e, n| {
        let id = e.admit(NewJob::connection(32 * 1024), n).unwrap();
        e.pump(n);
        let _ = e.complete(id, n);
    });

    println!("\n--- verdict ---");
    let base = slope
        .iter()
        .find(|(c, _)| *c == 32)
        .map(|(_, u)| *u)
        .unwrap_or(f64::NAN);
    let hi = slope
        .iter()
        .find(|(c, _)| *c == 256)
        .map(|(_, u)| *u)
        .unwrap_or(f64::NAN);
    println!("256 vs 32 live connections: {:.2}x", hi / base);
    for (c, u) in &slope {
        println!("active={c:<5} {u:.2} us/request");
    }
}
