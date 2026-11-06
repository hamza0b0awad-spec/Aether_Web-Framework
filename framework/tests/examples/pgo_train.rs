//! Profile-guided training run that terminates on its own.
//!
//! Why this exists: on this toolchain (rustc 1.98.1 / MSVC, measured 2026-10-25 with an
//! instrumented scratch binary) the instrumentation runtime writes its `.profraw` when the process
//! **returns from `main`** — including while other threads are still running — and writes
//! **nothing** when the process ends any other way: `std::process::exit(0)` reached mid-work,
//! `Stop-Process -Force`, and `taskkill` without `/F` each produced 0 files. A training run that
//! depends on a keypress reaching a server therefore cannot be trusted to produce a profile, and
//! a server launched detached (e.g. pinned with `cmd start /affinity`) has no console to press the
//! key in at all.
//!
//! So the training run carries its own load and its own exit: it serves the same routes as
//! `framework/examples/basic_api.rs`, drives a fixed number of keep-alive HTTP/1.1 requests at
//! them from inside the process, shuts the server down through the public API, waits for the
//! drain, and returns from `main`. Nothing external is needed, and the run is re-runnable.
//!
//! ```text
//! cargo build --release -p aether-tests --example pgo_train
//! AETHER_WORKERS=1 ./framework/target/release/examples/pgo_train.exe [requests] [connections]
//! ```
//!
//! Deliberate limits: plaintext HTTP/1.1 with `Content-Length` responses only, one in-flight
//! request per connection, and the client inside the measured process. The rate it prints is a
//! property of that arrangement — it is NOT a server throughput measurement, and must never be
//! quoted as one. Only the profile is a deliverable here.

use std::time::Instant;

use aether::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Requests offered in total across all connections.
const DEFAULT_REQUESTS: u64 = 50_000;
/// Keep-alive connections. Enough to keep the multi-thread runtime busy, few enough that the
/// in-process client does not starve the server of cores.
const DEFAULT_CONNECTIONS: usize = 16;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let requests = parse(&args, 0, DEFAULT_REQUESTS, "requests")?;
    let connections = parse(&args, 1, DEFAULT_CONNECTIONS as u64, "connections")? as usize;
    if connections == 0 {
        return Err("connections must be at least 1".into());
    }

    let config = AetherConfig::from_env()?;
    let mut app = AetherApp::with_config(config);
    // The same four routes as `basic_api`, so the profile describes the workload the A/B subject
    // binary actually serves: a static JSON route, a dynamic-parameter route, and a body reader.
    app = app.get("/health", |_req| {
        Response::json(bytes::Bytes::from(
            r#"{"status":"healthy","engine":"Aether Rust"}"#,
        ))
    });
    app = app.get("/api/users", |_req| {
        Response::json(bytes::Bytes::from(
            r#"[{"id":1,"name":"Hamza"},{"id":2,"name":"Sara"}]"#,
        ))
    });
    app = app.get("/users/{id}", |req| {
        let user_id = req.param("id").unwrap_or("unknown");
        let body = serde_json::json!({"user_id": user_id, "role": "architect"}).to_string();
        Response::json(bytes::Bytes::from(body))
    });
    app = app.post("/api/items", |req| {
        let content = req.body_str().unwrap_or("{}");
        let received = serde_json::from_str::<serde_json::Value>(content.trim())
            .unwrap_or_else(|_| serde_json::Value::String(content.to_string()));
        Response::json(bytes::Bytes::from(
            serde_json::json!({"success": true, "received_body": received}).to_string(),
        ))
    });

    // The runtime is built here, not by `#[tokio::main]`, so `AETHER_WORKERS` sizes the run that
    // produces the profile.
    let runtime = app.build_runtime()?;
    runtime.block_on(train(app, requests, connections))
}

fn parse(args: &[String], index: usize, default: u64, name: &str) -> Result<u64, String> {
    match args.get(index) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a positive integer, got '{raw}'")),
    }
}

/// Serve, load, shut down, return. Every step happens inside this process.
async fn train(
    app: AetherApp,
    requests: u64,
    connections: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Port 0: the bound port is read back from the socket, so two training runs can coexist and a
    // stale listener on a fixed port cannot silently absorb the load.
    let server = app.bind("127.0.0.1:0").await?;
    let port = server.local_addr().port();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server.serve_with_shutdown(async {
        // The sender is dropped when the load finishes; the error arm is still a shutdown signal.
        let _ = rx.await;
    }));

    let started = Instant::now();
    let answered = drive(port, requests, connections).await?;
    let elapsed = started.elapsed();

    drop(tx);
    // Waiting for the accept loop to finish keeps the shutdown path inside the profile too.
    serving.await??;

    let rate = answered as f64 / elapsed.as_secs_f64();
    println!(
        "trained: requests={answered} connections={connections} elapsed={:.1?} \
         offered_rate={:.0}/s (client in-process — not a server throughput measurement)",
        elapsed, rate
    );
    Ok(())
}

/// Drive `requests` keep-alive exchanges over `connections` tasks and count the 2xx answers.
async fn drive(port: u16, requests: u64, connections: usize) -> Result<u64, String> {
    let per_connection = (requests / connections as u64).max(1);
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..connections {
        tasks.spawn(async move { one_connection(port, per_connection, index).await });
    }
    let mut answered = 0u64;
    let mut first_error: Option<String> = None;
    while let Some(joined) = tasks.join_next().await {
        match joined.map_err(|e| format!("client task failed: {e}"))? {
            Ok(count) => answered += count,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    // A short run is still worth its profile, but a run with failures describes a server that was
    // not being exercised as intended, so it must not be treated as a clean training run.
    if let Some(error) = first_error {
        return Err(format!(
            "training load had failures ({answered} of {requests} answered): {error}"
        ));
    }
    Ok(answered)
}

/// One keep-alive connection, cycling through the three request shapes.
async fn one_connection(port: u16, count: u64, index: usize) -> Result<u64, String> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut answered = 0u64;
    for step in 0..count {
        let request = match (step + index as u64) % 3 {
            0 => b"GET /health HTTP/1.1\r\nHost: training\r\nConnection: keep-alive\r\n\r\n".as_slice(),
            1 => b"GET /users/hamza HTTP/1.1\r\nHost: training\r\nConnection: keep-alive\r\n\r\n".as_slice(),
            _ => b"POST /api/items HTTP/1.1\r\nHost: training\r\nContent-Length: 9\r\nConnection: keep-alive\r\n\r\n{\"id\":42}".as_slice(),
        };
        write_half
            .write_all(request)
            .await
            .map_err(|e| format!("write: {e}"))?;
        read_response(&mut reader).await?;
        answered += 1;
    }
    Ok(answered)
}

/// Read one `Content-Length` response, returning its status code.
async fn read_response<R: AsyncReadExt + Unpin>(reader: &mut BufReader<R>) -> Result<u16, String> {
    let mut status = 0u16;
    let mut length: usize = 0;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if read == 0 {
            return Err("connection closed mid-response".into());
        }
        if line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("http/1.1 ") {
            status = rest
                .get(..3)
                .and_then(|code| code.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = lower.strip_prefix("content-length:") {
            length = rest.trim().parse().unwrap_or(0);
        }
    }
    if !(200..300).contains(&status) {
        return Err(format!("server answered {status}"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| format!("body read: {e}"))?;
    Ok(status)
}
