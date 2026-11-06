//! A fixed-rate (open-loop) HTTP/1.1 load generator, for measuring the server's throughput
//! ceiling without the client's own concurrency limiting the answer.
//!
//! A closed-loop client — N connections, each sending its next request only after the previous
//! one returns — measures `concurrency / latency`, not the server's ceiling: when the server
//! slows down, the client silently sends fewer requests, so the dip never reaches the offered
//! rate and latency percentiles omit the requests that were never sent (coordinated omission).
//! This generator instead puts request *k* on the wire at `t0 + k / rate`, whether or not the
//! previous one has returned. The connection count is therefore a capacity limit, not the
//! offered load: when it is too small, `lag` grows and the run exits 2 instead of reporting a
//! number that the generator, not the server, produced.
//!
//! Not part of `cargo test`: it has no assertions and needs a live listener. Build and run it
//! against a plaintext endpoint (the owner runs the measurement; this tool only offers the load):
//!
//! ```text
//! cargo build --release -p aether-tests --example openloop_loadgen
//! openloop_loadgen --url http://127.0.0.1:3000/health --rate 20000 --duration 10 --connections 200
//! openloop_loadgen --url http://127.0.0.1:3000/health --rate 5000 --duration 5 \
//!                  --connections 20 --expect '{"status":"healthy","engine":"Aether Rust"}'
//! ```
//!
//! `--connections` is a sizing decision, not a knob to leave at a default: with one in-flight
//! request per connection the sustained rate is `connections / latency`, so it must be at least
//! `rate × the latency you expect` — below that, `lag` grows, the achieved rate plateaus and the
//! run exits 2. On Windows each connection also needs `rate / connections ≤ 500/s`, because a
//! wakeup cannot be scheduled finer than the 1 ms timer.
//!
//! `--expect` compares the response body byte for byte. When the shell cannot be trusted to
//! deliver it intact, `--expect-file PATH` reads the same bytes from a file: PowerShell 5.1 hands a
//! native exe `{"status":"healthy","engine":"Aether Rust"}` (43 bytes) as 35 bytes with every quote
//! stripped, which turns a correct server into a reported body mismatch on every rep. The two flags
//! fill one slot, so the last on the command line wins.
//!
//! `--client-threads` (default: half the available cores, clamped to 2..=4) sizes the
//! generator's runtime. When the generator and the server share one machine, a runtime that
//! claims every core takes CPU the server needs and the run measures the contention; a few
//! threads pace tens of thousands of requests per second while leaving the server its cores.
//! Raise it only when `lag` shows the generator itself behind; lower it when the box is small.
//!
//! Exit codes: **0** every scheduled request got a 2xx with the expected body and the offered
//! rate was delivered; **1** at least one request failed, answered non-2xx, or returned the wrong
//! body; **2** the run cannot be used as a throughput measurement — it answered less than 99% of
//! the requested rate, its median request went out later than one per-connection slot, or it
//! could not open its connections. Exit 2 says the rate was not delivered; it does not say which
//! side failed, and the pool arithmetic printed beside it says whether a bigger connection pool
//! could even have delivered it. Treating 2 as "the client's fault" is not safe: measured here, a
//! one-worker server answered 17,699/s at 128 connections and 14,707/s at 256, so the shortfall
//! moved the wrong way when the pool grew.
//!
//! Deliberate limits, so a result is never read wider than the tool: plaintext HTTP/1.1 only
//! (no TLS, no HTTP/2), one in-flight request per connection (answers "how many rps can the
//! server answer", not how it multiplexes), and `Content-Length` responses only (a chunked
//! response is counted as a failure rather than guessed at).

use std::collections::BTreeMap;
use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant as StdInstant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep_until, timeout};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Below this fraction of the requested rate the run is refused: the offered load was not
/// delivered, and which side failed is decided by the pool arithmetic printed at exit, not here.
const MIN_RATE_FRACTION: f64 = 0.99;
/// A worker sends one request at a time, so its slot spacing is `connections / rate`. Windows
/// sleeps in ~15.6 ms quanta unless the process asks for 1 ms (`timeBeginPeriod`), and even then
/// an overshoot of up to 1 ms is possible, so a per-connection rate above this leaves the
/// generator, not the server, as the limit. The check below refuses such a run instead of
/// publishing its number.
#[cfg(windows)]
const MAX_RATE_PER_CONNECTION: f64 = 500.0;
#[cfg(not(windows))]
const MAX_RATE_PER_CONNECTION: f64 = 5_000.0;

/// Raises the process timer resolution to 1 ms for the duration of the run. Since Windows 10
/// 2004 this is per-process, so it does not change the server's timers; without it every
/// `sleep_until` can overshoot by a full 15.6 ms quantum and the offered rate collapses.
#[cfg(windows)]
struct HighResTimers;

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

#[cfg(windows)]
impl HighResTimers {
    fn acquire() -> Self {
        // Safety: both calls take a plain integer and only affect this process's timer
        // resolution; `Drop` pairs them.
        unsafe {
            timeBeginPeriod(1);
        }
        Self
    }
}

#[cfg(windows)]
impl Drop for HighResTimers {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
        }
    }
}

#[cfg(not(windows))]
struct HighResTimers;

#[cfg(not(windows))]
impl HighResTimers {
    fn acquire() -> Self {
        Self
    }
}

struct Config {
    url: String,
    host: String,
    port: u16,
    path: String,
    rate: f64,
    duration: u64,
    connections: usize,
    client_threads: usize,
    expect: Option<Vec<u8>>,
}

#[derive(Default)]
struct Stats {
    started: u64,
    completed: u64,
    non_success: u64,
    failed: u64,
    mismatched: u64,
    statuses: BTreeMap<u16, u64>,
    lag_us: Vec<u32>,
    latency_us: Vec<u32>,
    first_error: Option<String>,
}

impl Stats {
    fn merge(&mut self, other: Stats) {
        self.started += other.started;
        self.completed += other.completed;
        self.non_success += other.non_success;
        self.failed += other.failed;
        self.mismatched += other.mismatched;
        for (code, n) in other.statuses {
            *self.statuses.entry(code).or_insert(0) += n;
        }
        self.lag_us.extend(other.lag_us);
        self.latency_us.extend(other.latency_us);
        if self.first_error.is_none() {
            self.first_error = other.first_error;
        }
    }
}

struct Conn {
    rd: BufReader<ReadHalf<TcpStream>>,
    wr: WriteHalf<TcpStream>,
}

enum Exchange {
    Success,
    NonSuccess(u16),
    Mismatch(String),
    Failed(String),
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("openloop-loadgen: {msg}");
            eprintln!(
                "usage: openloop_loadgen --url http://host:port/path [--rate N] [--duration S] \
                 [--connections N] [--client-threads N] [--expect BODY | --expect-file PATH]"
            );
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cfg.client_threads)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("openloop-loadgen: runtime: {err}");
            return ExitCode::from(2);
        }
    };
    let _timers = HighResTimers::acquire();
    match runtime.block_on(run(cfg)) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("openloop-loadgen: {msg}");
            ExitCode::from(2)
        }
    }
}

fn default_client_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |cores| (cores.get() / 2).clamp(2, 4))
}

fn parse_args() -> Result<Config, String> {
    let mut url: Option<String> = None;
    let mut rate = 1000.0_f64;
    let mut duration = 10_u64;
    let mut connections = 8_usize;
    let mut client_threads = default_client_threads();
    let mut expect: Option<Vec<u8>> = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--url" => url = Some(value()?),
            "--rate" => {
                rate = value()?
                    .parse()
                    .map_err(|_| "--rate must be a positive number".to_string())?;
            }
            "--duration" => {
                duration = value()?
                    .parse()
                    .map_err(|_| "--duration must be whole seconds".to_string())?;
            }
            "--connections" => {
                connections = value()?
                    .parse()
                    .map_err(|_| "--connections must be a positive integer".to_string())?;
            }
            "--client-threads" => {
                client_threads = value()?
                    .parse()
                    .map_err(|_| "--client-threads must be a positive integer".to_string())?;
            }
            "--expect" => expect = Some(value()?.into_bytes()),
            // The same bytes as `--expect`, read from a file instead of from argv. This exists
            // because a JSON body is not safe to put on a PowerShell 5.1 command line: the
            // generator received `{"status":"healthy","engine":"Aether Rust"}` (43 bytes) as
            // 35 bytes with every quote stripped, so a driver from PowerShell writes the body to a
            // file and passes this flag. Both flags fill the same slot, so the last one on the
            // command line wins.
            "--expect-file" => {
                let path = value()?;
                expect = Some(
                    std::fs::read(&path).map_err(|err| format!("--expect-file {path}: {err}"))?,
                );
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let url = url.ok_or("--url is required")?;
    if !(rate.is_finite() && rate > 0.0) {
        return Err("--rate must be a positive number".into());
    }
    if duration == 0 {
        return Err("--duration must be at least 1 second".into());
    }
    if connections == 0 {
        return Err("--connections must be at least 1".into());
    }
    if client_threads == 0 {
        return Err("--client-threads must be at least 1".into());
    }
    let rest = url
        .strip_prefix("http://")
        .ok_or("--url must start with http:// (this generator has no TLS)")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.to_string();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| "--url port is not a number".to_string())?,
        ),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err("--url has no host".into());
    }

    Ok(Config {
        url,
        host,
        port,
        path,
        rate,
        duration,
        connections,
        client_threads,
        expect,
    })
}

async fn run(cfg: Config) -> Result<ExitCode, String> {
    let interval_s = 1.0 / cfg.rate;
    let slots = (cfg.duration as f64 / interval_s).floor() as u64;
    if slots == 0 {
        return Err(format!(
            "--rate {} over {}s schedules no requests",
            cfg.rate, cfg.duration
        ));
    }
    if (cfg.connections as u64) > slots {
        eprintln!(
            "openloop-loadgen: note: {} connections but only {} slots; some connections idle",
            cfg.connections, slots
        );
    }
    let per_connection = cfg.rate / cfg.connections as f64;
    if per_connection > MAX_RATE_PER_CONNECTION {
        return Err(format!(
            "{} rps over {} connections is {}/s per connection, above the {}/s this platform \
             paces reliably with one in-flight request per connection — raise --connections to at \
             least {}",
            fmt_rate(cfg.rate),
            cfg.connections,
            fmt_rate(per_connection),
            fmt_rate(MAX_RATE_PER_CONNECTION),
            (cfg.rate / MAX_RATE_PER_CONNECTION).ceil() as u64,
        ));
    }
    println!(
        "openloop-loadgen: target={} rate={}/s duration={}s connections={} per_conn={}/s \
         client_threads={} expect={}",
        cfg.url,
        fmt_rate(cfg.rate),
        cfg.duration,
        cfg.connections,
        fmt_rate(per_connection),
        cfg.client_threads,
        if cfg.expect.is_some() { "yes" } else { "no" },
    );

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: keep-alive\r\n\r\n",
        cfg.path, cfg.host, cfg.port
    )
    .into_bytes();

    // Every connection is established before t0 so connection setup is not charged to the run.
    let mut initial = Vec::with_capacity(cfg.connections);
    for i in 0..cfg.connections {
        initial.push(
            connect(&cfg.host, cfg.port)
                .await
                .map_err(|err| format!("connect {i}: {err}"))?,
        );
    }

    let t0 = Instant::now();
    let mut tasks = Vec::with_capacity(cfg.connections);
    for (i, first) in initial.into_iter().enumerate() {
        let host = cfg.host.clone();
        let port = cfg.port;
        let request = request.clone();
        let expect = cfg.expect.clone();
        let step = cfg.connections as u64;
        tasks.push(tokio::spawn(async move {
            let mut stats = Stats::default();
            let mut conn = Some(first);
            let mut k = i as u64;
            while k < slots {
                let due = t0 + Duration::from_secs_f64(k as f64 * interval_s);
                sleep_until(due).await;
                stats.lag_us.push(usize_to_us(due.elapsed()));

                let begun = StdInstant::now();
                if conn.is_none() {
                    match connect(&host, port).await {
                        Ok(fresh) => conn = Some(fresh),
                        Err(err) => {
                            stats.started += 1;
                            stats.failed += 1;
                            if stats.first_error.is_none() {
                                stats.first_error = Some(err);
                            }
                            stats.latency_us.push(usize_to_us(begun.elapsed()));
                            k += step;
                            continue;
                        }
                    }
                }
                let exchange = match conn.as_mut() {
                    Some(c) => exchange(c, &request, &expect).await,
                    None => Exchange::Failed("no connection".into()),
                };
                stats.started += 1;
                match exchange {
                    Exchange::Success => stats.completed += 1,
                    Exchange::NonSuccess(code) => {
                        stats.completed += 1;
                        stats.non_success += 1;
                        *stats.statuses.entry(code).or_insert(0) += 1;
                    }
                    Exchange::Mismatch(why) => {
                        stats.completed += 1;
                        stats.mismatched += 1;
                        if stats.first_error.is_none() {
                            stats.first_error = Some(why);
                        }
                    }
                    Exchange::Failed(why) => {
                        stats.failed += 1;
                        if stats.first_error.is_none() {
                            stats.first_error = Some(why);
                        }
                        conn = None;
                    }
                }
                stats.latency_us.push(usize_to_us(begun.elapsed()));
                k += step;
            }
            stats
        }));
    }

    let mut total = Stats::default();
    for task in tasks {
        total.merge(task.await.map_err(|err| format!("worker: {err}"))?);
    }
    let wall = t0.elapsed();
    total.lag_us.sort_unstable();
    total.latency_us.sort_unstable();

    // The throughput a run may claim is answers per second, not sends per second.
    let achieved = total.completed as f64 / wall.as_secs_f64();
    // A worker takes every `connections`-th slot, so its own spacing — the deadline it must
    // wake for — is the global slot multiplied by the connection count.
    let worker_slot_us = (interval_s * cfg.connections as f64 * 1e6) as u64;
    let statuses = total
        .statuses
        .iter()
        .map(|(code, n)| format!("{code}={n}"))
        .collect::<Vec<_>>()
        .join(" ");

    println!(
        "scheduled={} started={} completed={} failed={} mismatch={}",
        slots, total.started, total.completed, total.failed, total.mismatched
    );
    if !statuses.is_empty() {
        println!("status: {statuses}");
    }
    print_percentiles("lag_us", &total.lag_us);
    print_percentiles("latency_us", &total.latency_us);
    println!(
        "wall_s={:.3} achieved_rps={}",
        wall.as_secs_f64(),
        fmt_rate(achieved)
    );
    if let Some(err) = total.first_error {
        println!("first_error: {err}");
    }

    if total.failed > 0 || total.mismatched > 0 || total.non_success > 0 {
        println!("result: FAIL (requests did not all succeed)");
        return Ok(ExitCode::from(1));
    }
    // One late wakeup is Windows timer jitter; a median request later than its whole slot means
    // the schedule was never caught up, so the offered load was never delivered. A run that
    // delivered ≥99% of the rate is kept, with the lag percentiles printed as its own error bars.
    let lag_p50_us = percentile(&total.lag_us, 50.0);
    let rate_short = achieved < cfg.rate * MIN_RATE_FRACTION;
    let lag_over_slot = u64::from(lag_p50_us) > worker_slot_us;
    if rate_short || lag_over_slot {
        // What this exit code proves is only "the offered rate was not delivered". Who failed to
        // deliver it is a separate question, and printing a fixed answer for it is how a server's
        // own saturation gets filed under "client problem". Measured on 2026-10-25: the same
        // one-worker binary at the same 20,000/s answered 17,699/s at 128 connections and
        // 14,707/s at 256 — the number moved the WRONG way when the pool grew, so the old advice
        // ("raise --connections") was not a remedy for that run.
        //
        // `connections / latency` is the pool's ceiling at the latency this run observed. That
        // figure is asymmetric, and only one direction is safe:
        //   * capacity >= rate  -> decisive. The pool could have carried the rate at the latency
        //     it saw, so pool size is not the explanation, and the remaining candidates are the
        //     server and this co-located generator.
        //   * capacity <  rate  -> NOT decisive. Under saturation the observed latency is mostly
        //     queue delay, which inflates it and deflates this capacity number, so a pool that
        //     looks too small may simply have been waiting on the server.
        let latency_p50_us = percentile(&total.latency_us, 50.0);
        let pool_capacity = cfg.connections as f64 * 1e6_f64 / f64::from(latency_p50_us).max(1.0);
        println!(
            "result: NOT-DELIVERED — answered {} of {}/s over a {}s schedule; fired: {} \
             lag p50 {lag_p50_us}us vs a {worker_slot_us}us per-connection slot",
            fmt_rate(achieved),
            fmt_rate(cfg.rate),
            cfg.duration,
            if rate_short {
                "answered < 99% of rate,"
            } else {
                ""
            },
        );
        if pool_capacity >= cfg.rate {
            println!(
                "  one in-flight request per connection at latency p50 {latency_p50_us}us caps this \
                 pool at {cap} req/s — ABOVE the offered rate, so the pool size is ruled out and \
                 this shortfall belongs to the server's own rate or to this generator running on \
                 the same box. Read it as evidence about the server, not as a client artifact.",
                cap = fmt_rate(pool_capacity)
            );
        } else {
            println!(
                "  one in-flight request per connection at latency p50 {latency_p50_us}us caps this \
                 pool at {cap} req/s — below the offered rate, but that is inconclusive: under \
                 saturation this latency is queue delay, which inflates it. Re-run at a larger \
                 --connections AND a lower --rate; a number that moves with the client's shape is \
                 not a server ceiling, so publish neither.",
                cap = fmt_rate(pool_capacity)
            );
        }
        return Ok(ExitCode::from(2));
    }
    println!("result: PASS");
    Ok(ExitCode::SUCCESS)
}

async fn connect(host: &str, port: u16) -> Result<Conn, String> {
    let stream = timeout(REQUEST_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|err| err.to_string())?;
    let _ = stream.set_nodelay(true);
    let (rd, wr) = tokio::io::split(stream);
    Ok(Conn {
        rd: BufReader::new(rd),
        wr,
    })
}

/// One request/response exchange on a keep-alive connection. Only a transport failure drops the
/// connection; a non-2xx or a body mismatch is a measurement of the server, not a client fault.
async fn exchange(conn: &mut Conn, request: &[u8], expect: &Option<Vec<u8>>) -> Exchange {
    let result = timeout(REQUEST_TIMEOUT, async {
        conn.wr
            .write_all(request)
            .await
            .map_err(|e| ReadError::Io(format!("write: {e}")))?;
        conn.wr
            .flush()
            .await
            .map_err(|e| ReadError::Io(format!("flush: {e}")))?;
        read_response(&mut conn.rd, expect).await
    })
    .await;
    match result {
        Ok(Ok(Response::Ok)) => Exchange::Success,
        Ok(Ok(Response::NonSuccess(code))) => Exchange::NonSuccess(code),
        Ok(Err(ReadError::Mismatch(why))) => Exchange::Mismatch(why),
        Ok(Err(ReadError::Io(why))) => Exchange::Failed(why),
        Err(_) => Exchange::Failed("request timed out".into()),
    }
}

enum Response {
    Ok,
    NonSuccess(u16),
}

enum ReadError {
    Mismatch(String),
    Io(String),
}

async fn read_response(
    rd: &mut BufReader<ReadHalf<TcpStream>>,
    expect: &Option<Vec<u8>>,
) -> Result<Response, ReadError> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = rd
            .read_until(b'\n', &mut line)
            .await
            .map_err(|err| ReadError::Io(format!("read head: {err}")))?;
        if n == 0 {
            return Err(ReadError::Io("peer closed before a response".into()));
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD_BYTES {
            return Err(ReadError::Io("response head over 64 KiB".into()));
        }
        if line == b"\r\n" {
            break;
        }
    }

    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| ReadError::Io("empty status line".into()))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ReadError::Io(format!("bad status line {status_line:?}")))?
        .parse()
        .map_err(|_| ReadError::Io(format!("bad status line {status_line:?}")))?;
    let content_length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or_else(|| {
            ReadError::Io(
                "response has no Content-Length (chunked responses are not supported)".into(),
            )
        })?;

    let mut body = vec![0_u8; content_length];
    rd.read_exact(&mut body)
        .await
        .map_err(|err| ReadError::Io(format!("read body: {err}")))?;
    if let Some(want) = expect
        && &body != want
    {
        return Err(ReadError::Mismatch(format!(
            "body mismatch: got {} bytes {:?}, want {} bytes",
            body.len(),
            String::from_utf8_lossy(&body[..body.len().min(40)]),
            want.len()
        )));
    }
    if (200..300).contains(&status) {
        Ok(Response::Ok)
    } else {
        Ok(Response::NonSuccess(status))
    }
}

fn fmt_rate(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Saturating microseconds, so a 5-second timeout (and the odd multi-second stall) stays in the
/// histogram instead of wrapping or panicking.
fn usize_to_us(d: Duration) -> u32 {
    d.as_micros().min(u32::MAX as u128) as u32
}

fn percentile(sorted: &[u32], p: f64) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn print_percentiles(label: &str, sorted: &[u32]) {
    if sorted.is_empty() {
        println!("{label}: (none)");
        return;
    }
    println!(
        "{label}: p50={} p95={} p99={} p99.9={} max={}",
        percentile(sorted, 50.0),
        percentile(sorted, 95.0),
        percentile(sorted, 99.0),
        percentile(sorted, 99.9),
        sorted[sorted.len() - 1],
    );
}
