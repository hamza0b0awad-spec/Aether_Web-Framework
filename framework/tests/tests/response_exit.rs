//! The response exit: the last place a status, an identifier and a reason phrase can be
//! corrected before a client reads them.
//!
//! Six claims, one per behaviour. A handler-declared error status outside `400..=599` must not
//! become a success or an unparseable status line. `X-Request-ID` names one request and one
//! access-log row, so a handler must not be able to publish a second, different value, and the
//! `x_request_id` setting must actually gate the field. A `1xx`
//! from a handler is not a protocol transition — only `101` can be, and the WebSocket tests pin
//! that separately — so every `1xx` is refused here. A `413` must carry RFC 9110's phrase
//! wherever it is built, not only where it happens to go through `canonical_reason`. And the
//! `code` a handler-declared status earns is derived from that status, so a declared `417` cannot
//! answer with the generic handler code.

use std::net::SocketAddr;
use std::time::Duration;

use aether::prelude::*;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// An error whose implementor names the status by hand. The trait is public, so the range the
/// blanket impl accepts has to be enforced rather than assumed.
struct DeclaredStatus(u16);

impl IntoResponseError for DeclaredStatus {
    fn error_status(&self) -> u16 {
        self.0
    }

    fn error_detail(&self) -> Option<String> {
        Some("a message written for a status that is not an error".to_string())
    }
}

/// Read the response head over a raw socket. `TestClient` reads a body to `Content-Length` or
/// EOF, and a leaked `1xx` carries neither framing header, so a helper that stops at the end of
/// the head is what keeps a failing case failing fast instead of timing out.
async fn read_head(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .expect("timed out waiting for the response head")
            .expect("read");
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn an_error_status_outside_the_error_range_answers_500() {
    let mut app = AetherApp::new();
    app = app.get_async("/declared-success", |_req: RequestContext| async move {
        Err::<&'static str, DeclaredStatus>(DeclaredStatus(200))
    });
    app = app.get_async("/declared-invalid", |_req: RequestContext| async move {
        Err::<&'static str, DeclaredStatus>(DeclaredStatus(700))
    });
    app = app.get_async("/declared-teapot", |_req: RequestContext| async move {
        Err::<&'static str, DeclaredStatus>(DeclaredStatus(418))
    });

    let client = TestClient::start(app).await;

    for path in ["/declared-success", "/declared-invalid"] {
        let res = client.get(path).send().await;
        assert_eq!(
            res.status, 500,
            "{path} must not answer with the out-of-range status it declared"
        );
        assert_eq!(
            res.status_text, "Internal Server Error",
            "{path} must carry the phrase of the status it actually answered"
        );
        let body: Value = res.json().expect("an error answer is JSON");
        assert_eq!(body["code"], "E5002", "{path}: {body}");
        assert_eq!(
            body["error"], "Internal Server Error",
            "{path} must not publish a message written for another status: {body}"
        );
        assert!(
            !res.text().contains("a message written for a status"),
            "{path} leaked the dropped detail"
        );
    }

    // The refusal is a boundary, not a rewrite: an in-range status the type chose is the answer.
    let res = client.get("/declared-teapot").send().await;
    assert_eq!(res.status, 418);
    assert_eq!(res.status_text, "I'm a Teapot");
    let body: Value = res.json().unwrap();
    assert_eq!(body["code"], "E4003");
    assert_eq!(
        body["error"], "a message written for a status that is not an error",
        "an in-range 4xx keeps its detail"
    );

    client.shutdown().await;
}

/// A 417 built by a handler, not by the parser: the `code` field is derived from the status the
/// error type declared, so a declared expectation refusal carries the expectation code rather than
/// the generic handler one. This is the same derivation the parser's refusal leans on, and a user
/// type reaches it without overriding `error_code`.
#[tokio::test]
async fn a_declared_expectation_refusal_inherits_the_code_of_its_status() {
    let mut app = AetherApp::new();
    app = app.get_async("/declared-417", |_req: RequestContext| async move {
        Err::<&'static str, DeclaredStatus>(DeclaredStatus(417))
    });

    let client = TestClient::start(app).await;

    let res = client.get("/declared-417").send().await;
    assert_eq!(res.status, 417);
    assert_eq!(res.status_text, "Expectation Failed");
    let body: Value = res.json().expect("an error answer is JSON");
    assert_eq!(body["code"], "E3003", "{body}");
    assert_eq!(
        body["error"], "a message written for a status that is not an error",
        "a 4xx keeps its detail: {body}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn a_handler_cannot_publish_a_second_x_request_id() {
    let mut app = AetherApp::new();
    app = app.get("/claims-id", |_req: RequestContext| {
        Response::status(200, "OK")
            .header("x-request-id", "forged-by-handler")
            .header("X-Kept", "yes")
    });

    let client = TestClient::start(app).await;
    let res = client.get("/claims-id").send().await;

    let ids: Vec<&str> = res
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-request-id"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(ids.len(), 1, "one request has one identifier, got {ids:?}");
    assert_ne!(
        ids[0], "forged-by-handler",
        "the framework's identifier must win: it is the one the access log names"
    );
    assert!(!ids[0].is_empty());
    assert_eq!(
        res.header("X-Kept"),
        Some("yes"),
        "removing the duplicate must not remove the handler's other headers"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn every_1xx_from_a_handler_is_refused_not_only_101() {
    let mut app = AetherApp::new();
    app = app.get("/early-hints", |_req: RequestContext| {
        Response::status(103, "Early Hints")
    });
    app = app.get("/unknown-informational", |_req: RequestContext| {
        Response::status(199, "Whatever")
    });

    let client = TestClient::start(app).await;
    for path in ["/early-hints", "/unknown-informational"] {
        let head = read_head(
            client.addr(),
            &format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            head.starts_with("HTTP/1.1 500 Internal Server Error"),
            "{path} must not send an informational status it cannot transition to: {head:?}"
        );
    }

    client.shutdown().await;
}

#[tokio::test]
async fn the_request_id_header_follows_the_config_flag() {
    let mut app = AetherApp::with_config(AetherConfig {
        x_request_id: false,
        ..AetherConfig::default()
    });
    app = app.get("/plain", |_req: RequestContext| "ok".to_string());

    let client = TestClient::start(app).await;
    let res = client.get("/plain").send().await;
    assert_eq!(res.status, 200);
    assert!(
        res.header("x-request-id").is_none(),
        "a config that disables the field must keep it off the wire: {:?}",
        res.headers
    );

    client.shutdown().await;
}

#[tokio::test]
async fn a_body_limit_refusal_names_the_canonical_413_phrase() {
    let mut app = AetherApp::new();
    app = app.get_async("/declared-413", |_req: RequestContext| async move {
        Err::<&'static str, AetherError>(AetherError::BodyLimitExceeded)
    });

    let client = TestClient::start(app).await;

    // The handler-error path already builds its phrase from `canonical_reason`; it is the
    // control that keeps the refusal below from passing for the wrong reason.
    let res = client.get("/declared-413").send().await;
    assert_eq!(res.status, 413);
    assert_eq!(res.status_text, "Content Too Large");

    // A head whose `Content-Length` alone crosses the limit: refused before any body arrives.
    // The default limit is 2 MiB, so 3 MiB is over it and far from overflowing the sum.
    let res = client
        .post("/upload")
        .header("Content-Length", "3000000")
        .send()
        .await;
    assert_eq!(res.status, 413);
    assert_eq!(
        res.status_text, "Content Too Large",
        "RFC 9110 renamed 413; the wire must not carry the old phrase"
    );

    client.shutdown().await;
}
