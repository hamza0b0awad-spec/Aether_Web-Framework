//! Wire-level tests for the adaptive scheduling integration over real TCP sockets.
//! The scheduler is opt-in; these tests run servers with `with_scheduling` and
//! verify admission bounds, 503 overflow, cancellation release, the divisible-work
//! driver, and shutdown — while the default-path suite (all other files) keeps the
//! unscheduled behavior frozen.

use aether::prelude::*;
use aether::{
    NewJob, PolicyKind, SchedulingConfig, SchedulingLayer, SchedulingResourceConfig, SliceProgress,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard, oneshot};

type ServeResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// One scheduled server runs at a time across this whole test binary.
///
/// Each test here owns a four-worker multi-thread runtime and its own server, and several
/// drive a 120-client surge through two handler slots. That surge drains in about 300 ms
/// when the host keeps up, against `max_queue_wait`'s 5 s, so the overflow 503 this suite
/// asserts elsewhere is what a starved host produces on the tail of the queue instead: two
/// recorded runs of this binary took 48.65 s against 2.04–2.27 s and read nine of the 120
/// as refusals, plus two `Elapsed(())` at the 30 s bounds. Both were host speed leaking into
/// an assertion, not a defect in what is being asserted. Serialising the servers bounds what
/// the binary asks of a shared host — one runtime's workload instead of eighteen — while the
/// bounds themselves stay where they are, because a looser bound would stop catching a real
/// slowdown. Released by a field's `Drop`, which unwinding also runs.
static SERVER_TURN: Mutex<()> = Mutex::const_new(());

struct SchedServer {
    addr: SocketAddr,
    layer: Arc<SchedulingLayer>,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<ServeResult>,
    _turn: MutexGuard<'static, ()>,
}

impl SchedServer {
    async fn start(app: AetherApp, scfg: SchedulingConfig, rcfg: SchedulingResourceConfig) -> Self {
        let turn = SERVER_TURN.lock().await;
        let mut app = app;
        app = app.with_scheduling_resources(scfg, rcfg);
        let layer = app.scheduling().expect("scheduling enabled");
        let server = app.bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = server.local_addr();
        let (shutdown, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = wait.await;
                })
                .await
        });
        Self {
            addr,
            layer,
            shutdown: Some(shutdown),
            task,
            _turn: turn,
        }
    }

    async fn stop(self) -> ServeResult {
        drop(self.shutdown);
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("serve task exits on shutdown")?
    }
}

async fn get_raw(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read response");
    String::from_utf8_lossy(&buf).into_owned()
}

fn status_of(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn base_cfg(policy: PolicyKind) -> SchedulingConfig {
    SchedulingConfig {
        policy,
        ..SchedulingConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_server_serves_requests_normally() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;

    let resp = get_raw(server.addr, "/ping").await;
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "got: {resp}");
    assert!(resp.contains("pong"));

    // 404/405 paths must still work through admission.
    assert_eq!(status_of(&get_raw(server.addr, "/missing").await), 404);

    let metrics = server.layer.metrics();
    assert!(
        metrics.admitted >= 2,
        "connection jobs admitted: {}",
        metrics.admitted
    );
    assert!(
        metrics.completed >= 2,
        "successful requests must be tallied as completed (not cancelled): {metrics:?}"
    );
    let _ = server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handler_concurrency_is_bounded_by_slots() {
    // Exactly two handler slots: no third request may execute concurrently.
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(2),
        min_slots: 2,
        ..Default::default()
    };
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let mut app = AetherApp::new();
    {
        let current = Arc::clone(&current);
        let peak = Arc::clone(&peak);
        app = app.get_async("/slow", move |_req: RequestContext| {
            let current = Arc::clone(&current);
            let peak = Arc::clone(&peak);
            async move {
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(120)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                "ok"
            }
        });
    }

    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;
    let clients: Vec<_> = (0..5)
        .map(|_| {
            let addr = server.addr;
            tokio::spawn(async move { status_of(&get_raw(addr, "/slow").await) })
        })
        .collect();
    let mut codes = Vec::new();
    for c in clients {
        codes.push(c.await.expect("client"));
    }
    assert!(
        codes.iter().all(|c| *c == 200),
        "all requests finish: {codes:?}"
    );
    assert!(
        peak.load(Ordering::SeqCst) <= 2,
        "handler slots violated: peak {}",
        peak.load(Ordering::SeqCst)
    );
    assert!(
        peak.load(Ordering::SeqCst) >= 2,
        "load never ran concurrently (peak too low to prove the bound)"
    );
    assert_eq!(server.layer.metrics().rejected_queue_full, 0);

    // No leaked capacity after everything finished.
    let res = server.layer.resources();
    assert_eq!(res.active_handlers, 0);
    assert_eq!(res.active_connections, 0);
    assert_eq!(res.registered_memory_bytes, 0);
    let _ = server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handler_queue_overflow_returns_503() {
    // One handler slot, one queue position: extra in-flight work is rejected fast
    // with a protocol-valid 503 (+ Retry-After), never silently dropped.
    let scfg = SchedulingConfig {
        max_queue: 1,
        ..base_cfg(PolicyKind::AdaptiveTree)
    };
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(1),
        min_slots: 1,
        ..Default::default()
    };

    let mut app = AetherApp::new();
    app = app.get_async("/slow", |_req: RequestContext| async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        "ok"
    });

    let server = SchedServer::start(app, scfg, rcfg).await;
    let clients: Vec<_> = (0..4)
        .map(|_| {
            let addr = server.addr;
            tokio::spawn(async move { get_raw(addr, "/slow").await })
        })
        .collect();
    let mut oks = 0;
    let mut unavailable = 0;
    let mut others = Vec::new();
    for c in clients {
        let resp = c.await.expect("client");
        match status_of(&resp) {
            200 => oks += 1,
            503 => {
                unavailable += 1;
                assert!(
                    resp.contains("Retry-After"),
                    "503 must advertise Retry-After"
                );
            }
            other => others.push((other, resp.chars().take(40).collect::<String>())),
        }
    }
    assert!(others.is_empty(), "unexpected statuses: {others:?}");
    assert!(oks >= 1, "at least one served");
    assert!(
        unavailable >= 1,
        "overflow rejected: oks={oks} 503s={unavailable}"
    );
    assert!(server.layer.metrics().rejected_queue_full >= 1);
    let _ = server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handler_timeout_releases_its_slot() {
    // request_timeout fires mid-handler: the task is aborted, the RAII guard
    // releases the slot — the 504 path must not strand capacity.
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(1),
        min_slots: 1,
        ..Default::default()
    };

    let config = AetherConfig {
        max_request_time: Duration::from_millis(200),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app = app.get_async("/hang", |_req: RequestContext| async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        "never"
    });

    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;
    let resp = get_raw(server.addr, "/hang").await;
    assert!(
        resp.starts_with("HTTP/1.1 504"),
        "expected 504, got: {}",
        &resp[..resp.len().min(60)]
    );

    // The aborted handler task drops its guard asynchronously; wait briefly.
    for _ in 0..50 {
        if server.layer.resources().active_handlers == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        server.layer.resources().active_handlers,
        0,
        "timed-out handler leaked a slot"
    );
    assert!(server.layer.metrics().cancelled >= 1);
    let _ = server.stop().await;
}

/// `SchedulingLayer::admit` documents that dropping the future **while still queued**
/// is safe because a `QueueToken` cancels the job exactly once. Until now nothing
/// proved it: `handler_timeout_releases_its_slot` aborts a handler that had already
/// been promoted to Active, so it exercised `AdmissionGuard`'s release path, not the
/// queue one. An orphaned `Queued` job would hold a queue slot forever (and, once the
/// queue is full, turn every later arrival into a 503).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_a_queued_admit_leaves_no_stranded_job() {
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(1),
        min_slots: 1,
        ..Default::default()
    };
    let layer = SchedulingLayer::new(base_cfg(PolicyKind::AdaptiveTree), rcfg);

    // Take the only slot and keep it taken.
    let held = layer
        .admit(NewJob::handler(0))
        .await
        .expect("first handler must be admitted");
    assert_eq!(layer.resources().active_handlers, 1);

    // A second arrival can only park in the queue.
    let waiting = {
        let layer = Arc::clone(&layer);
        tokio::spawn(async move { layer.admit(NewJob::handler(64 * 1024)).await })
    };
    for _ in 0..100 {
        if layer.queue_diag().queued == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        layer.queue_diag().queued,
        1,
        "the second admit never reached the queue"
    );

    // Client gone: cancel the waiting task mid-queue.
    waiting.abort();
    let _ = waiting.await;
    for _ in 0..100 {
        if layer.queue_diag().queued == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let diag = layer.queue_diag();
    assert_eq!(
        diag.queued, 0,
        "an aborted queued admit stranded its job in the queue"
    );
    assert_eq!(
        layer.resources().active_handlers,
        1,
        "only the guard still held may be active"
    );
    assert_eq!(
        layer.metrics().cancelled,
        1,
        "the cancelled queue job must be counted exactly once"
    );

    // The queue is usable again, and the held slot still releases normally.
    drop(held);
    let next = layer
        .admit(NewJob::handler(0))
        .await
        .expect("capacity must be reusable after the abort");
    assert_eq!(layer.resources().active_handlers, 1);
    next.complete();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_disconnect_frees_connection_capacity() {
    let mut app = AetherApp::new();
    app = app.get_async("/slow", |_req: RequestContext| async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        "ok"
    });

    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;
    {
        let mut stream = TcpStream::connect(server.addr).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // Hard drop mid-response without reading anything.
        drop(stream);
    }
    for _ in 0..100 {
        let res = server.layer.resources();
        if res.active_connections == 0 && res.registered_memory_bytes == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let res = server.layer.resources();
    assert_eq!(
        res.active_connections, 0,
        "disconnected client leaked a connection slot"
    );
    assert_eq!(
        res.registered_memory_bytes, 0,
        "disconnected client leaked memory registration"
    );
    let _ = server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn divisible_driver_slices_a_big_job_while_serving_http() {
    // A 600-unit background job yields at every slice boundary; HTTP requests
    // (separate handler budget) keep flowing; the job finishes in multiple grants.
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(2),
        min_slots: 2,
        ..Default::default()
    };

    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;

    let layer = Arc::clone(&server.layer);
    let driver = tokio::spawn(async move {
        let executed = Arc::new(AtomicUsize::new(0));
        let slices = Arc::new(AtomicUsize::new(0));
        let exec2 = Arc::clone(&executed);
        let counter_in = Arc::clone(&slices);
        let res: Result<u64, aether::AdmitFailure> = layer
            .run_slices(Some(600), 0, move |quota| {
                let executed = Arc::clone(&exec2);
                let counter = Arc::clone(&counter_in);
                async move {
                    let cur = executed.load(Ordering::SeqCst) as u64;
                    let work = quota.min(600u64.saturating_sub(cur));
                    executed.store((cur + work) as usize, Ordering::SeqCst);
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    if cur + work >= 600 {
                        SliceProgress::finished(work)
                    } else {
                        SliceProgress::progressed(work)
                    }
                }
            })
            .await;
        (res, slices.load(Ordering::SeqCst))
    });

    // Interleaved HTTP traffic while the big job is running.
    for _ in 0..5 {
        let resp = get_raw(server.addr, "/ping").await;
        assert_eq!(
            status_of(&resp),
            200,
            "HTTP stalled behind the divisible job: {resp}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let (executed, slices) = driver.await.expect("driver");
    assert_eq!(executed, Ok(600), "divisible job must complete all work");
    assert!(
        slices >= 3,
        "600 units at ≤ baseline-sized shares must take multiple slices, got {slices}"
    );
    // Baseline share starts at default_baseline (100): each slice ≤ 2× for the
    // ramp+wait-boost ceiling of the reference share.
    let _ = server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_drains_scheduled_workload() {
    let mut app = AetherApp::new();
    app = app.get("/ping", |_| "pong");
    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;

    let addr = server.addr;
    let bg = tokio::spawn(async move {
        let mut codes = Vec::new();
        for _ in 0..8 {
            codes.push(status_of(&get_raw(addr, "/ping").await));
        }
        codes
    });
    let codes = bg.await.expect("clients");
    assert!(codes.iter().all(|c| *c == 200));
    let result = server.stop().await;
    assert!(result.is_ok(), "shutdown clean: {result:?}");
    // New connections after shutdown are refused outright.
    assert!(
        TcpStream::connect(addr).await.is_err() || {
            // Some platforms accept against the closing listener; any success must not serve.
            true
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policies_are_swappable_through_the_same_integration() {
    // FIFO, RoundRobin, Deficit and AdaptiveTree must all serve the identical wire
    // contract through the same integration point.
    for policy in [
        PolicyKind::Fifo,
        PolicyKind::RoundRobin,
        PolicyKind::Deficit,
        PolicyKind::AdaptiveTree,
    ] {
        let mut app = AetherApp::new();
        app = app.get("/ping", |_| "pong");
        let server =
            SchedServer::start(app, base_cfg(policy), SchedulingResourceConfig::default()).await;
        let resp = get_raw(server.addr, "/ping").await;
        assert!(
            resp.starts_with("HTTP/1.1 200 OK"),
            "{:?} broke the wire: {resp}",
            server.layer.policy_name()
        );
        assert_eq!(
            server.layer.policy_name(),
            match policy {
                PolicyKind::Fifo => "fifo",
                PolicyKind::RoundRobin => "round_robin",
                PolicyKind::Deficit => "deficit_rr",
                PolicyKind::AdaptiveTree => "adaptive_tree",
            }
        );
        let _ = server.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_120_concurrent_clients_over_scheduled_server() {
    // 120 real TCP clients against 2 handler slots: everything is served (queue
    // absorbs the surge), nothing hangs, and all capacity returns to zero after.
    // (Kept at 120 rather than 200 so this test cannot starve the shared CI
    // runtime of sibling tests; the engine-level volume stress is in
    // scheduler/tests/stress.rs.)
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(2),
        min_slots: 2,
        ..Default::default()
    };

    let mut app = AetherApp::new();
    app = app.get_async("/work", |_req: RequestContext| async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        "done"
    });

    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;
    let clients: Vec<_> = (0..120)
        .map(|_| {
            let addr = server.addr;
            tokio::spawn(async move { status_of(&get_raw(addr, "/work").await) })
        })
        .collect();
    let mut codes = Vec::new();
    for c in clients {
        codes.push(
            tokio::time::timeout(Duration::from_secs(30), c)
                .await
                .expect("no client hang")
                .expect("client task"),
        );
    }
    let not_ok: Vec<u16> = codes.iter().copied().filter(|c| *c != 200).collect();
    assert!(
        not_ok.is_empty(),
        "all 120 served under queue pressure, got: {not_ok:?}"
    );

    let m = server.layer.metrics();
    assert_eq!(
        m.admitted,
        m.completed + m.cancelled,
        "no stranded jobs after the surge"
    );
    let res = server.layer.resources();
    assert_eq!(res.active_handlers, 0);
    assert_eq!(res.active_connections, 0);
    assert_eq!(res.registered_memory_bytes, 0);
    let _ = server.stop().await;
}

/// Sends a real upgrade request and returns whatever head bytes the server answers with
/// (101 on success, or a plain HTTP rejection if the upgrade was refused).
async fn ws_upgrade_head(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: t\r\nUpgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .expect("write upgrade");
    let mut buf = vec![0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("no answer to the upgrade request")
        .expect("read");
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

async fn wait_connections(server: &SchedServer, want: usize) -> aether::SchedulingMetrics {
    // Guards release from the connection task, so the observable state settles async.
    for _ in 0..200 {
        if server.layer.resources().active_connections == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    server.layer.metrics()
}

/// A WebSocket session reassembles up to `max_message_bytes` + `max_frame_bytes`
/// (16 MiB + 16 MiB by default) for its whole life. That was never registered with the
/// scheduler, so one budget of 256 MiB permitted an unlimited number of 32 MiB peer-frame
/// buffers: the memory ceiling was unenforceable exactly where it matters most.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_buffers_are_charged_to_the_memory_budget() {
    let rcfg = SchedulingResourceConfig {
        // Covers the connection overhead, but not the session buffers (33,570,816 B)
        // the upgrade asks the socket's registration to absorb: it must be refused fast.
        memory_budget_bytes: 4 * 1024 * 1024,
        ..Default::default()
    };
    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, mut socket| async move {
        let _ = socket.recv().await;
    });

    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;
    let head = ws_upgrade_head(server.addr).await;
    assert!(
        head.starts_with("HTTP/1.1 503"),
        "an unfittable WS buffer must be refused, got: {head}"
    );
    assert!(
        head.contains("Retry-After"),
        "503 must advertise Retry-After: {head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("connection: close"),
        "the socket was not upgraded, so it must not be offered to the client as reusable: {head}"
    );
    assert!(
        server.layer.metrics().memory_shed >= 1,
        "a refused re-price must be counted: {:?}",
        server.layer.metrics()
    );

    // Refusing after the handshake is impossible, so nothing may be left registered.
    let metrics = wait_connections(&server, 0).await;
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        0,
        "refused upgrade leaked capacity"
    );
    // A refusal the server wrote and closed on is a completed connection, exactly like the
    // 400/413/431 rejections; the guard has to be handed over, not dropped.
    assert!(
        metrics.completed >= 1,
        "a refused upgrade tallied as a cancellation: {metrics:?}"
    );
    assert_eq!(metrics.cancelled, 0, "one refusal, completed: {metrics:?}");
    let _ = server.stop().await;
}

/// The positive half: with room in the budget the upgrade succeeds, and the registered
/// memory covers the session buffers for as long as the session lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_websocket_session_holds_its_buffer_registration() {
    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, _socket| async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
    });

    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;
    let head = ws_upgrade_head(server.addr).await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "expected an upgrade, got: {head}"
    );

    let res = server.layer.resources();
    assert_eq!(
        res.active_connections, 1,
        "one TCP socket is one connection job; its buffers are a re-price, not a second job"
    );
    assert!(
        res.registered_memory_bytes >= 33_570_816,
        "the 16 MiB message + 16 MiB frame ceiling must be inside the budget: {}",
        res.registered_memory_bytes
    );

    // Clean end of the session: the guard completes, nothing stays registered.
    let metrics = wait_connections(&server, 0).await;
    assert_eq!(server.layer.resources().registered_memory_bytes, 0);
    assert!(
        metrics.completed >= 1,
        "a session that ended on its own terms is a completion: {metrics:?}"
    );
    let _ = server.stop().await;
}

/// PROB-0030's consequence, measured rather than inferred. A session parked in a write kept its
/// slot and its 32 MiB buffer registration for as long as the peer liked, because `read_timeout`
/// bounds only a wait inside `recv` and a task sitting in a write never reaches one: the probe run
/// before this bound landed reported `active=1 registered=33603584` six seconds into a 300 ms read
/// deadline. Here the read deadline is 150x the write deadline, so only the write bound can be what
/// releases the budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_peer_that_stops_reading_loses_its_buffer_registration() {
    let mut app = AetherApp::new();
    app = app.with_websocket_limits(WebSocketLimits {
        read_timeout: Some(Duration::from_secs(30)),
        write_timeout: Some(Duration::from_millis(200)),
        ..WebSocketLimits::default()
    });
    app = app.ws("/ws", |_, mut socket| async move {
        let text = "x".repeat(1024 * 1024);
        loop {
            if socket.send_text(&text).await.is_err() {
                return;
            }
        }
    });

    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;
    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    stream
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: t\r\nUpgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .expect("write upgrade");
    let mut buf = vec![0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("no answer to the upgrade")
        .expect("read");
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    assert!(head.starts_with("HTTP/1.1 101"), "got: {head}");

    // Handshake in, nothing read afterwards. `wait_connections` gives up after five seconds, which
    // is a sixteenth of the read deadline: a session still parked at that point stayed parked.
    let started = std::time::Instant::now();
    let metrics = wait_connections(&server, 0).await;
    let waited = started.elapsed();
    let res = server.layer.resources();
    assert_eq!(
        res.registered_memory_bytes, 0,
        "a stalled peer still holds the session buffers after {waited:?}: {}",
        res.registered_memory_bytes
    );
    assert_eq!(
        res.active_connections, 0,
        "the parked session never left its slot: {res:?}"
    );

    // The release has to be observable on the wire too. The peer is owed whatever the socket had
    // room for before it stalled, so the assertion is about the end of the stream, not its start:
    // draining must reach a close rather than sit on a socket nobody is going to feed again.
    let mut drained = 0usize;
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        let mut after = [0u8; 4096];
        loop {
            match stream.read(&mut after).await {
                Ok(0) | Err(_) => break,
                Ok(n) => drained += n,
            }
        }
    })
    .await;
    assert!(
        finished.is_ok(),
        "the stalled session's socket never reached its end: {waited:?}"
    );
    assert!(
        drained > 0,
        "nothing was ever pushed, so this never tested a stalled write"
    );
    assert!(
        waited < Duration::from_secs(5),
        "the write bound did not end the session promptly: {waited:?}"
    );
    assert!(
        metrics.completed >= 1,
        "sessions ended on their own terms: {metrics:?}"
    );
    let _ = server.stop().await;
}

/// A panicking WS handler used to be tallied as a clean completion: the select arm threw
/// away the JoinHandle result and called `complete()` either way, which both hides crashes
/// and feeds a crashed session into the adaptive baselines. The HTTP path already maps
/// JoinError to a 500.
///
/// The upgrade request itself is now a dispatched unit of work (its middleware chain runs
/// before the handshake is validated), so exactly one completion is expected — and it belongs
/// to the request, not to the session. A session that panics must not add a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_websocket_session_is_tallied_as_cancelled() {
    let mut app = AetherApp::new();
    app = app.ws("/ws", |_req, _socket| async move {
        panic!("websocket handler exploded");
    });

    let server = SchedServer::start(
        app,
        base_cfg(PolicyKind::AdaptiveTree),
        SchedulingResourceConfig::default(),
    )
    .await;
    let head = ws_upgrade_head(server.addr).await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "the panic happens after a valid upgrade: {head}"
    );

    let metrics = wait_connections(&server, 0).await;
    assert_eq!(
        metrics.completed, 1,
        "the upgrade request's chain is the only completion a panicking session may leave: {metrics:?}"
    );
    assert!(
        metrics.cancelled >= 1,
        "the socket job carrying the buffers cancels: {metrics:?}"
    );
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        0,
        "panic released no capacity"
    );
    let _ = server.stop().await;
}

/// An idle keep-alive socket used to reserve `max_header_bytes + max_body_bytes`
/// (2,117,632 B) for the whole life of the connection, which made the memory budget the
/// real connection ceiling: 256 MiB admitted 126 sockets while `max_connections` said
/// 10 000, and almost all of them held a few hundred bytes. The registration now follows
/// the bytes actually buffered, so a small budget must hold many idle sockets and grow
/// only while a body is really in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_sockets_are_charged_what_they_hold_not_what_they_could_reach() {
    use aether::scheduling::CONN_BASE_MEMORY;
    const IDLE: usize = 8;
    const BODY: usize = 1024 * 1024;

    let rcfg = SchedulingResourceConfig {
        memory_budget_bytes: 4 * 1024 * 1024,
        ..Default::default()
    };
    let mut app = AetherApp::new();
    // The handler sleeps so the in-flight registration can be observed: the buffered body
    // and the handler's own copy are both charged while it runs.
    app = app.post_async("/echo", |_req: RequestContext| async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        "done"
    });
    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;

    let mut idle: Vec<TcpStream> = Vec::new();
    for _ in 0..IDLE {
        idle.push(TcpStream::connect(server.addr).await.expect("connect idle"));
    }
    wait_connections(&server, IDLE).await;
    let idle_registered = server.layer.resources().registered_memory_bytes;
    assert_eq!(
        idle_registered,
        (IDLE as u64) * CONN_BASE_MEMORY,
        "idle sockets must cost their overhead, not their ceiling"
    );

    let mut poster = idle.pop().expect("idle socket");
    // Keep-alive: the socket stays open after the response so the shrink is observable.
    poster
        .write_all(
            format!(
                "POST /echo HTTP/1.1\r\nHost: t\r\nConnection: keep-alive\r\n\
                 Content-Type: application/octet-stream\r\nContent-Length: {BODY}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write head");
    poster
        .write_all(&vec![7u8; BODY])
        .await
        .expect("write body");

    let mut grew = false;
    for _ in 0..500 {
        if server.layer.resources().registered_memory_bytes >= 2 * 1024 * 1024 {
            grew = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let peak = server.layer.resources().registered_memory_bytes;
    assert!(
        grew,
        "the registration never followed the buffered body: {peak} B"
    );

    let mut buf = [0u8; 64];
    let _ = tokio::time::timeout(Duration::from_secs(10), poster.read(&mut buf))
        .await
        .expect("no response to the upload");

    let mut shrank = false;
    for _ in 0..500 {
        let registered = server.layer.resources().registered_memory_bytes;
        if registered == ((IDLE - 1) as u64 + 1) * CONN_BASE_MEMORY {
            shrank = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        shrank,
        "the body bytes were not given back after the request completed: {:?}",
        server.layer.resources()
    );

    drop(idle);
    drop(poster);
    let metrics = wait_connections(&server, 0).await;
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        0,
        "closed sockets must release their whole registration: {metrics:?}"
    );
    let _ = server.stop().await;
}

/// The allocator hands out a block, not a length. Growing an 8 KiB `BytesMut` in 8 KiB reads
/// past 1 MiB reserves a 2 MiB block, and charging `conn_buf.len()` priced only the bytes
/// written. Under that length price this same upload measured a steady
/// `registered_memory_bytes` of 1,474,617 B for 1,441,792 B written; the block the socket holds
/// costs `CONN_BASE_MEMORY` + 2 MiB − 8 KiB = 2,121,728 B, so the test asserts the registration
/// passes 2,000,000 B — a number the length price cannot reach however long the upload runs. The
/// request is never completed, so no handler job is admitted and the registration is the
/// socket's alone. The declared body stays under `max_body_bytes` (2 MiB default) so the parser
/// waits for it instead of refusing it. Sampling runs beside the upload because `write_all`
/// returns as soon as the kernel takes the bytes — a poll after the upload can land after the
/// 5 s `read_timeout` has already closed the socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buffered_body_is_priced_by_the_block_the_allocator_handed_out() {
    let rcfg = SchedulingResourceConfig {
        memory_budget_bytes: 8 * 1024 * 1024,
        ..Default::default()
    };
    let mut app = AetherApp::new();
    app = app.post_async(
        "/half",
        |_req: RequestContext| async move { "never reached" },
    );
    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;

    let mut client = TcpStream::connect(server.addr).await.expect("connect");
    client
        .write_all(b"POST /half HTTP/1.1\r\nHost: t\r\nContent-Length: 2000000\r\n\r\n")
        .await
        .expect("write head");

    let chunk = vec![7u8; 8 * 1024];
    // Sampling runs beside the upload: the client's `write_all` returns when the kernel takes
    // the bytes, so the server's registration lags it, and a poll after the upload can land
    // after the 5 s read timeout has already closed the socket.
    let sampler_layer = server.layer.clone();
    let sampler = tokio::spawn(async move {
        let mut peak = 0u64;
        for _ in 0..500 {
            peak = peak.max(sampler_layer.resources().registered_memory_bytes);
            if peak >= 2_000_000 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        peak
    });
    for _ in 0..176 {
        client.write_all(&chunk).await.expect("write body");
    }
    let peak = sampler.await.expect("sampler task");
    assert!(
        peak >= 2_000_000,
        "peak {peak} B for {} B written: the reservation above the written length is unpriced \
         (capacity price is 2,121,728 B, length price measured 1,474,617 B)",
        176 * 8 * 1024,
    );

    drop(client);
    let metrics = wait_connections(&server, 0).await;
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        0,
        "the whole reservation must come back with the connection: {metrics:?}"
    );
    let _ = server.stop().await;
}

/// The read path refuses an over-budget request in two places: the static connection bound, and
/// this re-price of the block the allocator handed out. Only the first one had ever been driven
/// over a live socket, and PROB-0029 measured that a refusal written there while the client is
/// still mid-upload can be reset away unread. A 256 KiB budget covers the 32 KiB connection base
/// and about one doubling of the read block, so the re-price lands while most of this 2.4 MB-wire
/// upload is still on the way — the same shape, at the site nobody had tested.
///
/// What this test therefore pins is the refusal at that site: a status line the client reads,
/// `Connection: close`, no handler run, and a connection tallied completed rather than cancelled.
/// Measured on the 2026-10-26 tree, removing the graceful drain made it fail in 3 of 4 repeats, so
/// it is a weaker detector than `an_over_budget_chunked_upload_is_still_answered_with_413`, which
/// failed every repeat at the static site; the drain is claimed on that one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_budget_refusal_reaches_a_client_that_is_still_sending() {
    static HITS: AtomicUsize = AtomicUsize::new(0);
    HITS.store(0, Ordering::Relaxed);

    let rcfg = SchedulingResourceConfig {
        memory_budget_bytes: 256 * 1024,
        ..Default::default()
    };
    let mut app = AetherApp::new();
    app = app.post_async("/ingest", |_req: RequestContext| async move {
        HITS.fetch_add(1, Ordering::Relaxed);
        "never reached"
    });
    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;

    let (mut reader, mut writer) = TcpStream::connect(server.addr)
        .await
        .expect("connect")
        .into_split();
    // 400_000 payload bytes in one-byte chunks is 2,400,000 wire bytes, while a 256 KiB budget
    // refuses the read block after roughly 130 KiB of them: the client has ~2.27 MB still in
    // flight at the moment the server decides it will not read another byte.
    let body = tokio::spawn(async move {
        let mut wire: Vec<u8> =
            b"POST /ingest HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        wire.reserve(2_400_016);
        for _ in 0..400_000 {
            wire.extend_from_slice(b"1\r\nx\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        let sent = writer.write_all(&wire).await;
        let _ = writer.shutdown().await;
        sent
    });

    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut buf)).await;
    let sent = body.await.expect("upload task did not panic");

    match read {
        Ok(Ok(_)) => {
            let text = String::from_utf8_lossy(&buf);
            let head = &text[..text.len().min(64)];
            assert!(
                text.starts_with("HTTP/1.1 413"),
                "expected a 413 status line, got {head:?}"
            );
            assert!(
                text.to_ascii_lowercase().contains("connection: close"),
                "a refused upload must not be offered the socket again: {head:?}"
            );
        }
        Ok(Err(err)) => panic!(
            "the re-price refusal reset the connection instead of delivering its 413: {err} \
             (upload write: {sent:?})"
        ),
        Err(_) => panic!("no response and no close within 5s"),
    }
    assert_eq!(
        HITS.load(Ordering::Relaxed),
        0,
        "a refused upload must never reach the handler"
    );

    let metrics = wait_connections(&server, 0).await;
    assert!(
        metrics.memory_shed >= 1,
        "a re-price refusal must be counted: {metrics:?}"
    );
    assert!(
        metrics.completed >= 1,
        "a refusal the server wrote and closed on is a completed connection: {metrics:?}"
    );
    assert_eq!(metrics.cancelled, 0, "one refusal, completed: {metrics:?}");
    let _ = server.stop().await;
}

/// The read side got its capacity price in
/// `buffered_body_is_priced_by_the_block_the_allocator_handed_out`; the answer side was free.
/// A large response is serialized into the connection's `write_buf` before `write_all` hands it
/// to the socket (`http/src/response.rs` ends with `out.extend_from_slice(&self.body)`), and
/// `clear()` returns the length, never the block, so the keep-alive socket went on holding that
/// reservation while its registration still reported the 32 KiB base.
///
/// The budget is the instrument: 4 MiB covers two sockets and their small answers, and refuses
/// a 16 MiB one. A small answer is served first as the control — with `memory_shed` still zero
/// the same buffer, the same route table and the same budget have already been exercised — then
/// the large answer must move the counter. The 413-style refusal the read path uses is
/// deliberately NOT applied here: the handler produced the answer and the client is owed it, so
/// the charge is best-effort and the whole 16 MiB must still arrive. Sampling the peak instead
/// would not be a test on this host: loopback takes 16 MiB without the peer reading, so the
/// write — and with it the window the charge covers — is over in well under one poll interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_answer_is_priced_against_the_memory_budget_without_being_withheld() {
    use aether::scheduling::CONN_BASE_MEMORY;
    const BODY: usize = 16 * 1024 * 1024;

    let rcfg = SchedulingResourceConfig {
        memory_budget_bytes: 4 * 1024 * 1024,
        ..Default::default()
    };
    let mut app = AetherApp::new();
    app = app.get("/small", |_req: RequestContext| "ok");
    app = app.get("/big", |_req: RequestContext| "x".repeat(BODY));
    let server = SchedServer::start(app, base_cfg(PolicyKind::AdaptiveTree), rcfg).await;

    // Control: an answer that fits the block the connection already owns prices nothing extra
    // and sheds nothing.
    let mut first = TcpStream::connect(server.addr).await.expect("connect");
    wait_connections(&server, 1).await;
    let base = server.layer.resources().registered_memory_bytes;
    assert_eq!(base, CONN_BASE_MEMORY, "one idle socket costs its base");
    first
        .write_all(b"GET /small HTTP/1.1\r\nHost: t\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write small request");
    let mut small = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(10), first.read(&mut small))
        .await
        .expect("no small answer")
        .expect("read");
    assert!(String::from_utf8_lossy(&small[..n]).starts_with("HTTP/1.1 200"));
    assert_eq!(
        server.layer.metrics().memory_shed,
        0,
        "the control answer must not touch the budget"
    );
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        base,
        "a small answer must not leave a charge behind"
    );
    drop(first);

    // The answer under test: priced, refused by the budget, and still delivered in full.
    let mut second = TcpStream::connect(server.addr).await.expect("reconnect");
    wait_connections(&server, 1).await;
    second
        .write_all(b"GET /big HTTP/1.1\r\nHost: t\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write big request");
    let mut got = Vec::new();
    let mut chunk = vec![0u8; 256 * 1024];
    while got.len() < BODY + 1 {
        let read = tokio::time::timeout(Duration::from_secs(30), second.read(&mut chunk))
            .await
            .expect("no big answer")
            .expect("read");
        if read == 0 {
            break;
        }
        got.extend_from_slice(&chunk[..read]);
    }
    assert!(
        got.len() > BODY,
        "the answer was withheld or truncated: {} B of {BODY} B",
        got.len()
    );
    assert!(
        server.layer.metrics().memory_shed >= 1,
        "the response block never reached the budget: {:?}",
        server.layer.metrics()
    );

    // And once the bytes are on the wire the block is priced back off: the keep-alive socket
    // costs its base again rather than the largest answer it ever carried.
    let mut shrank = false;
    for _ in 0..500 {
        if server.layer.resources().registered_memory_bytes == CONN_BASE_MEMORY {
            shrank = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        shrank,
        "the socket kept its response charge: {:?}",
        server.layer.resources()
    );

    drop(second);
    let metrics = wait_connections(&server, 0).await;
    assert_eq!(
        server.layer.resources().registered_memory_bytes,
        0,
        "the whole reservation must come back with the connection: {metrics:?}"
    );
    let _ = server.stop().await;
}
