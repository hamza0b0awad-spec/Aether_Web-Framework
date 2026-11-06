//! The legacy worker loop is a byte-stream frontend, so a request it cannot reassemble is a
//! request it cannot answer. Both cases here are the ones the loop used to drop: one request split
//! across two reads, and two requests arriving in one read.

use std::sync::Arc;
use std::time::Duration;

use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_router::trie::RadixRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn demo_router() -> Arc<RadixRouter> {
    let mut router = RadixRouter::new();
    router.add_route(Method::Get, "/health", 2);
    router.add_route(Method::Get, "/users/{id}", 4);
    Arc::new(router)
}

/// Serve exactly one connection, then let the task end with it.
async fn serve_once(router: Arc<RadixRouter>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let _ =
            aether_runtime::handle_connection(stream, router, Arc::new(ServerLimits::default()))
                .await;
    });
    addr
}

/// Send each chunk as its own write, let the server see it, then close the write half so the
/// answer is read to a clean EOF instead of a timeout.
async fn exchange(addr: std::net::SocketAddr, chunks: &[&str]) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    for chunk in chunks {
        stream.write_all(chunk.as_bytes()).await.expect("write");
        stream.flush().await.expect("flush");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stream.shutdown().await.expect("shutdown write half");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out))
        .await
        .expect("the server must answer within the deadline")
        .expect("read");
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn a_request_split_across_two_reads_is_answered_once() {
    let addr = serve_once(demo_router()).await;
    let response = exchange(
        addr,
        &["GET /health HT", "TP/1.1\r\nHost: localhost\r\n\r\n"],
    )
    .await;

    assert_eq!(
        response.matches("HTTP/1.1 ").count(),
        1,
        "one request is one answer, whatever its framing: {response:?}"
    );
    assert!(
        response.contains("healthy"),
        "the answer must be the health handler's body: {response:?}"
    );
}

#[tokio::test]
async fn two_pipelined_requests_in_one_read_are_both_answered() {
    let addr = serve_once(demo_router()).await;
    let response = exchange(
        addr,
        &["GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n\
             GET /users/7 HTTP/1.1\r\nHost: localhost\r\n\r\n"],
    )
    .await;

    assert_eq!(
        response.matches("HTTP/1.1 ").count(),
        2,
        "two pipelined requests are two answers: {response:?}"
    );
    assert!(
        response.contains("healthy"),
        "the first answer is the health handler's: {response:?}"
    );
    assert!(
        response.contains("\"user_id\":\"7\""),
        "the second answer is the captured parameter, not the first response twice: {response:?}"
    );
}

#[tokio::test]
async fn a_malformed_request_is_answered_400_and_the_connection_closes() {
    let addr = serve_once(demo_router()).await;
    let response = exchange(addr, &["GET\r\n\r\n"]).await;

    assert_eq!(
        response.matches("HTTP/1.1 ").count(),
        1,
        "a framing error is one answer, not silence: {response:?}"
    );
    assert!(
        response.starts_with("HTTP/1.1 400 "),
        "a request line with no target is a bad request: {response:?}"
    );
    assert!(
        response.to_ascii_lowercase().contains("connection: close"),
        "the rest of the stream is undecidable, so the answer must close: {response:?}"
    );
}

/// The legacy loop reads the same shared head parser as `aether`, so an unmet expectation is
/// refused with the status that means it. A rule that answers `417` on one frontend and `400` on
/// another is two rules, and clients only see one of them.
#[tokio::test]
async fn an_unsupported_expectation_is_refused_by_the_legacy_loop() {
    let addr = serve_once(demo_router()).await;
    let response = exchange(
        addr,
        &["POST /health HTTP/1.1\r\nHost: localhost\r\nExpect: foo=bar\r\nContent-Length: 5\r\n\r\nhello"],
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 417 "),
        "the legacy loop did not refuse the unmet expectation: {response:?}"
    );
}
