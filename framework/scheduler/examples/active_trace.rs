//! Deterministic behavioural trace of the engine, used to prove an internal change is
//! decision-preserving.
//!
//! Every clock value is passed in explicitly and every job id comes from the arrival
//! sequence, so the same script produces byte-identical output on two builds. Run it
//! before and after a change and diff the two files: if any admitted set, grant, quota,
//! class, counter or resource snapshot moves, the change altered scheduling behaviour.
//!
//! The script deliberately covers the paths that touch the active index from both sides:
//! connections and handlers (which must never be rotated), divisible jobs that get grants
//! and re-classify, jobs cancelled while still queued, starvation promotions, a queue
//! deeper than the admission scan window, and a memory budget tight enough to skip jobs.
//!
//!   cargo run --release -p aether-scheduler --example active_trace > before.txt

use std::time::Duration;

use aether_scheduler::{NewJob, ResourceConfig, SchedulerEngine, SchedulingConfig, Work};

fn ns(x: u64) -> Duration {
    Duration::from_nanos(x)
}

fn dump(e: &SchedulerEngine, label: &str, out: &aether_scheduler::RoundOutcome) {
    println!("== {label}");
    println!("   admitted = {:?}", out.admitted);
    println!(
        "   grants   = {:?}",
        out.grants
            .iter()
            .map(|g| (g.job.0, g.quota))
            .collect::<Vec<_>>()
    );
    println!("   capacity = {:?}", out.capacity_changed);
    let m = e.metrics();
    println!(
        "   metrics  = admitted={} completed={} cancelled={} rejected_q={} rejected_m={} \
         starve={} promo={} demo={} grants={} slices={} work={} peak_q={} skip={} halt={} shed={}",
        m.admitted,
        m.completed,
        m.cancelled,
        m.rejected_queue_full,
        m.rejected_memory,
        m.starvation_promotions,
        m.class_promotions,
        m.class_demotions,
        m.grants_issued,
        m.slice_reports,
        m.work_executed,
        m.peak_queue,
        m.skipped_unfit,
        m.scan_window_halts,
        m.memory_shed
    );
    let r = e.resources();
    println!(
        "   res      = conn={} hnd={} cpu={} div={} slots={} mem={} avail={}",
        r.active_connections,
        r.active_handlers,
        r.active_cpu,
        r.active_divisible,
        r.handler_slots,
        r.registered_memory_bytes,
        e.memory_available()
    );
    println!(
        "   queued   = {:?}",
        e.queued_ids().iter().map(|i| i.0).collect::<Vec<_>>()
    );
    println!("   baseline = {}", e.baseline());
    for d in e.decisions() {
        println!("   decision t={:?} job={} {:?}", d.now, d.job.0, d.kind);
    }
}

/// The rotation-sensitive scenario: several divisible jobs running *at the same time as*
/// live connections. This is the only place where restricting the index to divisible jobs
/// could change an observable outcome, because the rotation cursor walks the index by
/// position — so a connection sitting between two divisible jobs currently moves the
/// starting point. Everything else in the engine filters the index by job kind.
///
/// It is printed job-by-job rather than as a bare grant list, so a diff shows exactly which
/// job got which quota in which round.
fn scene_rotation() {
    println!("\n########## scene: rotation with live connections present ##########");
    let rcfg = ResourceConfig {
        memory_budget_bytes: 64 * 1024 * 1024,
        // One handler slot makes the round budget equal to a single quota, so exactly one
        // divisible job is granted per round and the rotation position decides which one.
        // With a wide budget every job is served anyway and the ordering is untestable.
        max_active_handlers: Some(1),
        ..ResourceConfig::default()
    };
    let cfg = SchedulingConfig {
        round_factor: 1,
        ..SchedulingConfig::default()
    };
    let mut e = SchedulerEngine::new(cfg, rcfg);
    let mut t: u64 = 0;

    // Three background jobs with declared work, admitted in a known order.
    let mut divs = Vec::new();
    for i in 0..3 {
        t += 1_000;
        let now = ns(t);
        divs.push(
            e.admit(NewJob::divisible(Some(10_000 + i * 1_000), 4 * 1024), now)
                .unwrap(),
        );
        let out = e.pump(now);
        dump(
            &e,
            &format!("divisible {i} id={}", divs[i as usize].0),
            &out,
        );
    }

    // Connections interleaved between the divisible jobs in the index, arriving and
    // leaving while the background jobs keep taking slices.
    let mut conns = Vec::new();
    for round in 0..10 {
        t += 2_000;
        let now = ns(t);
        let c = e.admit(NewJob::connection(4 * 1024), now).unwrap();
        conns.push(c);
        let out = e.pump(now);
        println!(
            "-- round {round}: live connection {} granted {}",
            c.0,
            out.admitted.len()
        );
        for g in &out.grants {
            println!("   grant job={} quota={}", g.job.0, g.quota);
        }
        // Each running divisible job executes its grant.
        let grants: Vec<(u64, Work)> = out.grants.iter().map(|g| (g.job.0, g.quota)).collect();
        for (job, quota) in grants {
            if quota > 0 {
                e.report_slice(
                    aether_scheduler::JobId(job),
                    quota,
                    Duration::from_micros(100),
                    now,
                );
            }
        }
        // Close the oldest still-open connection, which is what re-enters the walk.
        if round % 2 == 1 {
            let old = conns[round / 2 % conns.len()];
            e.complete(old, now);
            let out = e.pump(now);
            println!(
                "   closed connection {}, re-admitted {}",
                old.0,
                out.admitted.len()
            );
            for g in &out.grants {
                println!("   grant-after-close job={} quota={}", g.job.0, g.quota);
            }
        }
        let r = e.resources();
        println!(
            "   state: conns={} div={} queued={:?} grants_total={} work={}",
            r.active_connections,
            r.active_divisible,
            e.queued_ids().iter().map(|i| i.0).collect::<Vec<_>>(),
            e.metrics().grants_issued,
            e.metrics().work_executed
        );
    }

    // Drain: finish everything, divisible jobs completing with real executed work so the
    // baseline samples and class moves are part of the comparison too.
    for d in &divs {
        t += 500;
        let now = ns(t);
        e.complete(*d, now);
        let out = e.pump(now);
        println!("   drained divisible {} grants={}", d.0, out.grants.len());
    }
    for c in &conns {
        let _ = e.cancel(*c, ns(t));
    }
    let out = e.pump(ns(t));
    dump(&e, "rotation scene end", &out);
}

fn main() {
    let cfg = SchedulingConfig {
        // A small window so the "queue deeper than the window" branch is actually taken.
        ..SchedulingConfig::default()
    };
    let rcfg = ResourceConfig {
        // Tight enough that a 64 KiB job can be refused while a 4 KiB one still fits,
        // which is what makes the walk skip rather than stall.
        memory_budget_bytes: 1024 * 1024,
        max_connections: 8,
        max_active_handlers: Some(4),
        ..ResourceConfig::default()
    };
    let mut e = SchedulerEngine::new(cfg, rcfg);
    let mut t: u64 = 0;

    // 1. Plain connections: admitted, served, released. These are the jobs that must
    //    stop appearing in the rotation index.
    let mut live = Vec::new();
    for i in 0..4 {
        t += 1_000;
        let now = ns(t);
        let id = e.admit(NewJob::connection(32 * 1024), now).unwrap();
        let out = e.pump(now);
        live.push(id);
        dump(&e, &format!("connection {i} id={}", id.0), &out);
    }

    // 2. A divisible job with a declared large hint: it is granted, executes slices, and
    //    re-classifies as its measured work grows.
    t += 1_000;
    let div_now = ns(t);
    let div = e
        .admit(NewJob::divisible(Some(100_000), 8 * 1024), div_now)
        .unwrap();
    let out = e.pump(div_now);
    dump(&e, &format!("divisible id={}", div.0), &out);
    for s in 0..6 {
        t += 5_000;
        let now = ns(t);
        let outcome = e.pump(now);
        let grant = outcome
            .grants
            .iter()
            .find(|g| g.job == div)
            .map(|g| g.quota);
        let w: Work = grant.unwrap_or(100);
        e.report_slice(div, w, Duration::from_micros(250), now);
        println!("-- slice {s} report {w} at t={now:?}");
        dump(
            &e,
            &format!("divisible slice {s} grant={grant:?}"),
            &outcome,
        );
    }
    t += 1_000;
    let done = ns(t);
    e.complete(div, done);
    let out = e.pump(done);
    dump(&e, "divisible completed", &out);

    // 3. Handler jobs (indivisible, slot-consuming).
    t += 1_000;
    let hn = ns(t);
    let h1 = e.admit(NewJob::handler(16 * 1024), hn).unwrap();
    let h2 = e.admit(NewJob::handler(16 * 1024), hn).unwrap();
    let c1 = e.admit(NewJob::cpu_heavy(16 * 1024), hn).unwrap();
    let out = e.pump(hn);
    dump(
        &e,
        &format!("handler+cpu admitted h={} {} {}", h1.0, h2.0, c1.0),
        &out,
    );
    t += 2_000;
    let hn2 = ns(t);
    e.cancel(h1, hn2);
    let out = e.pump(hn2);
    dump(&e, "handler cancelled", &out);

    // 4. Queue deeper than the admission scan window, mixed sizes, so the walk skips
    //    unfittable jobs and halts on the window bound.
    let mut deep = Vec::new();
    for i in 0..40 {
        t += 500;
        let now = ns(t);
        let mem = if i % 3 == 0 { 128 * 1024 } else { 4 * 1024 };
        match e.admit(NewJob::connection(mem), now) {
            Ok(id) => deep.push(id),
            Err(r) => println!("!! deep admit {i} rejected {r:?}"),
        }
    }
    let out = e.pump(ns(t));
    dump(&e, "deep queue loaded", &out);

    // 5. Release live connections one by one: each release re-enters the rotation walk,
    //    so this is where an index holding non-divisible jobs shows up as work.
    for conn in deep.iter().take(3) {
        t += 1_000;
        let now = ns(t);
        e.complete(*conn, now);
        let out = e.pump(now);
        dump(&e, &format!("released connection {}", conn.0), &out);
    }

    // 6. Free capacity, then jump the clock past the starvation guard, then pump once.
    //    Promotion is tallied at the walk, so the freeing pump must come first or the
    //    starved branch is never reached and the trace would prove nothing about it.
    t += 1_000;
    let fr = ns(t);
    for id in live.iter().chain([h2, c1].iter()) {
        e.cancel(*id, fr);
    }
    t += 3_000_000_000;
    let later = ns(t);
    let out = e.pump(later);
    dump(&e, "freed + starvation jump, one pump", &out);

    // 7. Cancel everything remaining, both queued and active, and finish with a pump.
    for (i, id) in deep.iter().enumerate().skip(3) {
        let now = ns(t + 10 + i as u64);
        e.cancel(*id, now);
    }
    let out = e.pump(ns(t));
    dump(&e, "final", &out);
    println!(
        "== end live_connections={}",
        e.resources().active_connections
    );
    scene_rotation();
}
