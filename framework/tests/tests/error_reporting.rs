//! The redacted half of the handler-error contract, over a real socket.
//!
//! A 5xx body carries `canonical_reason(status)` and never the internal message. PROB-0009's
//! residual was that the message then went nowhere: an operator had a status and no cause.
//! These tests pin the replacement — the message reaches the installed reporter under the same
//! `X-Request-ID` the client receives — and the three shapes that must *not* report: a 4xx whose
//! detail is in the body, a 5xx that publishes its own detail, and a type that does not opt in.
//!
//! One reporter per process is the design, so the whole file is one test: installing it twice
//! would be testing `OnceLock` rather than the contract.

use aether::prelude::*;
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
struct Seen {
    request_id: Option<String>,
    status: u16,
    code: String,
    detail: String,
}

static SINK: Mutex<Option<Arc<Mutex<Vec<Seen>>>>> = Mutex::new(None);

fn seen() -> Arc<Mutex<Vec<Seen>>> {
    let mut slot = SINK.lock().unwrap();
    if let Some(sink) = slot.as_ref() {
        return Arc::clone(sink);
    }
    let sink: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let target = Arc::clone(&sink);
    aether::set_error_reporter(move |report| {
        target.lock().unwrap().push(Seen {
            request_id: report.request_id.map(str::to_string),
            status: report.status,
            code: report.code.to_string(),
            detail: report.detail.to_string(),
        });
    })
    .expect("the first install must succeed");
    *slot = Some(Arc::clone(&sink));
    sink
}

/// A user error that redacts in the body and opts into the diagnostics channel.
struct SecretFailure;

impl IntoResponseError for SecretFailure {
    fn error_status(&self) -> u16 {
        500
    }

    fn redacted_detail(&self) -> Option<String> {
        Some("connection string names a password".to_string())
    }
}

/// A user error that puts its detail in the body, so there is nothing to report.
struct OpenFailure;

impl IntoResponseError for OpenFailure {
    fn error_status(&self) -> u16 {
        500
    }

    fn error_detail(&self) -> Option<String> {
        Some("visible detail".to_string())
    }
}

#[tokio::test]
async fn a_redacted_5xx_detail_reaches_the_reporter_under_the_response_request_id() {
    let sink = seen();

    let mut app = AetherApp::new();
    app = app.get_async("/io", |_req: RequestContext| async move {
        Err::<&'static str, AetherError>(AetherError::Io(std::io::Error::other("disk says no")))
    });
    app = app.get_async("/string", |_req: RequestContext| async move {
        Err::<&'static str, &'static str>("tell the client this")
    });
    app = app.get_async("/secret", |_req: RequestContext| async move {
        Err::<&'static str, SecretFailure>(SecretFailure)
    });
    app = app.get_async("/open", |_req: RequestContext| async move {
        Err::<&'static str, OpenFailure>(OpenFailure)
    });

    let client = TestClient::start(app).await;

    // The AetherError shape: redacted on the wire, reported with the id the client can read.
    let res = client.get("/io").send().await;
    assert_eq!(res.status, 500);
    let body: Value = res.json().unwrap();
    assert_eq!(
        body["error"], "Internal Server Error",
        "the internal message must not reach the body"
    );
    assert_eq!(body["code"], "E5002");
    assert!(!res.text().contains("disk says no"));

    let header_id = res
        .header("x-request-id")
        .expect("the default config emits X-Request-ID")
        .to_string();
    let reports = sink.lock().unwrap().clone();
    assert_eq!(
        reports.len(),
        1,
        "exactly one report for one redacted 5xx: {reports:?}"
    );
    assert_eq!(reports[0].status, 500);
    assert_eq!(reports[0].code, "E5002");
    assert!(
        reports[0].detail.contains("disk says no"),
        "the report must carry the dropped detail, got {:?}",
        reports[0].detail
    );
    assert_eq!(
        reports[0].request_id.as_deref(),
        Some(header_id.as_str()),
        "the report must name the same request the client was told"
    );

    // A 4xx keeps its detail in the body, so reporting it again would be a duplicate.
    let res = client.get("/string").send().await;
    assert_eq!(res.status, 400);
    let body: Value = res.json().unwrap();
    assert_eq!(body["error"], "tell the client this");
    assert_eq!(sink.lock().unwrap().len(), 1, "a 4xx must not report");

    // A user type that opts in is reported too, with its own id.
    let res = client.get("/secret").send().await;
    assert_eq!(res.status, 500);
    assert_eq!(
        res.json::<Value>().unwrap()["error"],
        "Internal Server Error"
    );
    let secret_id = res.header("x-request-id").unwrap().to_string();
    let reports = sink.lock().unwrap().clone();
    assert_eq!(reports.len(), 2, "the opt-in type must report: {reports:?}");
    assert_eq!(reports[1].detail, "connection string names a password");
    assert_eq!(reports[1].request_id.as_deref(), Some(secret_id.as_str()));

    // A 5xx that publishes its own detail is already visible; it must not report.
    let res = client.get("/open").send().await;
    assert_eq!(res.status, 500);
    assert_eq!(res.json::<Value>().unwrap()["error"], "visible detail");
    assert_eq!(
        sink.lock().unwrap().len(),
        2,
        "a published detail must not report"
    );

    client.shutdown().await;
}
