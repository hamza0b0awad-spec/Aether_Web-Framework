//! End-to-end tests for the signed double-submit CSRF layer (F-12).
//!
//! Every request here goes over a real socket: the gate runs before dispatch in the serving
//! core, so a test that called the token layer directly would prove nothing about the wire. The
//! planted token is signed with a different secret by the test itself, exactly as a sibling
//! subdomain's cookie would be by the attacker.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aether::prelude::*;
use aether::security::jwt::encode_base64url;
use ring::hmac;

const SECRET: &[u8] = b"wire-csrf-secret-0123456789abcdef";
const COOKIE: &str = "aether_csrf";
const HEADER: &str = "x-csrf-token";

fn tokens(secure: bool) -> CsrfTokens {
    CsrfTokens::new(
        SECRET,
        CsrfConfig {
            secure,
            ..CsrfConfig::default()
        },
    )
}

/// Every `Set-Cookie` a response carried, in order.
fn cookies(response: &TestResponse) -> Vec<&str> {
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.as_str())
        .collect()
}

/// The token in the one CSRF cookie a response set, or `None` when it set none.
fn issued(response: &TestResponse) -> Option<String> {
    let cookie = *cookies(response).first()?;
    let (pair, _) = cookie.split_once(';')?;
    let (name, value) = pair.split_once('=')?;
    assert_eq!(name, COOKIE, "unexpected cookie {cookie:?}");
    Some(value.to_owned())
}

/// A token with the right shape that this server cannot have signed.
fn planted() -> String {
    let nonce = "AAAAAAAAAAAAAAAAAAAAAA";
    let key = hmac::Key::new(hmac::HMAC_SHA256, b"the-attackers-secret");
    let mac = hmac::sign(&key, nonce.as_bytes());
    format!("{nonce}.{}", encode_base64url(mac.as_ref()))
}

fn pair(token: &str) -> [(&'static str, String); 2] {
    [
        ("cookie", format!("{COOKIE}={token}")),
        (HEADER, token.to_owned()),
    ]
}

/// A GET that renders whatever token the layer handed it, and a POST that records that it ran.
fn app_with_form(csrf: CsrfTokens, ran: Arc<AtomicUsize>) -> AetherApp {
    let mut app = AetherApp::new();
    app = app.enable_csrf(csrf);
    app = app.get("/form", |ctx: RequestContext| {
        ctx.extension::<CsrfToken>()
            .map(CsrfToken::as_str)
            .unwrap_or("")
            .to_owned()
    });
    app = app.post("/submit", move |_| {
        ran.fetch_add(1, Ordering::SeqCst);
        "submitted"
    });
    app
}

/// A token is issued on a successful safe response, never on a failure: an error page has no
/// form to protect, and the cookie is the browser's state, not the server's apology.
#[tokio::test]
async fn a_failed_response_does_not_issue_a_token() {
    let mut app = AetherApp::new();
    app = app.enable_csrf(tokens(true));
    app = app.get("/fail", |_| Response::status(500, "Internal Server Error"));
    let client = TestClient::start(app).await;

    let response = client.get("/fail").send().await;
    assert_eq!(response.status, 500);
    assert!(cookies(&response).is_empty(), "{:?}", cookies(&response));

    client.shutdown().await;
}

/// A safe response issues one cookie, with the attributes the threat note promises.
#[tokio::test]
async fn a_safe_response_issues_the_token_it_hands_the_page() {
    let client =
        TestClient::start(app_with_form(tokens(true), Arc::new(AtomicUsize::new(0)))).await;

    let response = client.get("/form").send().await;
    assert_eq!(response.status, 200);
    let all = cookies(&response);
    assert_eq!(all.len(), 1, "one token, one cookie: {all:?}");
    let cookie = all[0];
    for expected in ["SameSite=Lax", "Path=/", "Secure"] {
        assert!(
            cookie.contains(expected),
            "the issued cookie must carry {expected}: {cookie:?}"
        );
    }
    for forbidden in ["HttpOnly", "Domain"] {
        assert!(
            !cookie.contains(forbidden),
            "the issued cookie must not carry {forbidden}: {cookie:?}"
        );
    }
    let token = issued(&response).expect("a token was issued");
    assert_eq!(
        response.text(),
        token,
        "the page must receive exactly the token the cookie carries"
    );

    client.shutdown().await;
}

/// Both switches the configuration owns reach the wire.
#[tokio::test]
async fn the_cookie_attributes_follow_the_configuration() {
    let mut app = AetherApp::new();
    app = app.enable_csrf(CsrfTokens::new(
        SECRET,
        CsrfConfig {
            secure: false,
            same_site: CsrfSameSite::Strict,
            max_age: Some(600),
            ..CsrfConfig::default()
        },
    ));
    app = app.get("/form", |_| "form");
    let client = TestClient::start(app).await;

    let response = client.get("/form").send().await;
    let cookie = cookies(&response)[0];
    assert!(cookie.contains("SameSite=Strict"), "{cookie:?}");
    assert!(cookie.contains("Max-Age=600"), "{cookie:?}");
    assert!(!cookie.contains("Secure"), "{cookie:?}");

    client.shutdown().await;
}

/// The cheaper refusals outrank the gate: a request the limiter refuses gets the limiter's
/// answer, not a `403`, and a safe request it refuses is handed no token.
#[tokio::test]
async fn the_rate_limiter_outranks_the_csrf_gate() {
    let mut app = AetherApp::new();
    app = app.enable_csrf(tokens(true));
    app = app.enable_rate_limit(RateLimitConfig {
        burst: 1,
        per: Duration::from_secs(60),
        max_clients: 16,
    });
    app = app.get("/form", |_| "form");
    app = app.post("/submit", |_| "submitted");
    let client = TestClient::start(app).await;

    // The one request inside the budget is answered and issued the token its page renders.
    let first = client.get("/form").send().await;
    assert_eq!(first.status, 200);
    assert_eq!(
        cookies(&first).len(),
        1,
        "the admitted request gets a token"
    );

    // Over budget, an unsafe request is the limiter's `429`, not the gate's `403`.
    assert_eq!(client.post("/submit").send().await.status, 429);

    // And a safe one refused by the limiter is handed nothing: a refusal has no page to protect.
    let refused = client.get("/form").send().await;
    assert_eq!(refused.status, 429);
    assert!(
        cookies(&refused).is_empty(),
        "a refused request must not be issued a token: {:?}",
        cookies(&refused)
    );

    client.shutdown().await;
}

/// Without the header, with a header and no cookie, and with a pair that does not match: one
/// `403`, an empty body, no cookie, and the handler never runs.
#[tokio::test]
async fn every_missing_or_mismatched_token_is_refused_before_the_handler() {
    let ran = Arc::new(AtomicUsize::new(0));
    let client = TestClient::start(app_with_form(tokens(true), Arc::clone(&ran))).await;

    // Two tokens the server itself would accept, so "mismatch" is the only defect in the pair.
    let first = issued(&client.get("/form").send().await).expect("a token");
    let second = issued(&client.get("/form").send().await).expect("a token");
    assert_ne!(first, second, "a fresh token per unprimed request");

    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("no cookie and no header", vec![]),
        ("cookie only", vec![("cookie", format!("{COOKIE}={first}"))]),
        ("header only", vec![(HEADER, first.clone())]),
        (
            "the cookie and header name different tokens",
            vec![("cookie", format!("{COOKIE}={first}")), (HEADER, second)],
        ),
        (
            "a planted pair the attacker signed",
            vec![
                ("cookie", format!("{COOKIE}={}", planted())),
                (HEADER, planted()),
            ],
        ),
        (
            "the same token on two cookie lines",
            vec![
                ("cookie", format!("{COOKIE}={first}")),
                ("cookie", format!("{COOKIE}={first}")),
                (HEADER, first.clone()),
            ],
        ),
        (
            "the same token on two header lines",
            vec![
                ("cookie", format!("{COOKIE}={first}")),
                (HEADER, first.clone()),
                (HEADER, first.clone()),
            ],
        ),
    ];

    let mut first_shape: Option<(u16, String)> = None;
    for (label, headers) in cases {
        let mut request = client.post("/submit");
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        let response = request.send().await;
        let shape = (response.status, response.text().into_owned());
        assert_eq!(shape.0, 403, "{label}: status");
        assert!(shape.1.is_empty(), "{label}: the body must stay empty");
        assert!(
            cookies(&response).is_empty(),
            "{label}: a refusal must not set or refresh a cookie, got {:?}",
            cookies(&response)
        );
        match &first_shape {
            None => first_shape = Some(shape),
            Some(first) => assert_eq!(first, &shape, "{label}: refusal differs from the first"),
        }
    }

    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "no refused request may reach the handler"
    );
    client.shutdown().await;
}

/// The pair a page was rendered with is accepted.
#[tokio::test]
async fn the_pair_the_page_was_rendered_with_is_accepted() {
    let ran = Arc::new(AtomicUsize::new(0));
    let client = TestClient::start(app_with_form(tokens(true), Arc::clone(&ran))).await;

    let response = client.get("/form").send().await;
    let token = response.text().into_owned();
    assert!(!token.is_empty(), "the form was rendered without a token");

    let mut request = client.post("/submit");
    for (name, value) in pair(&token) {
        request = request.header(name, &value);
    }
    let response = request.send().await;
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "submitted");
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    client.shutdown().await;
}

/// A valid cookie is reused, not rotated: an open form keeps working across other page loads.
#[tokio::test]
async fn a_valid_cookie_is_reused_and_not_reissued() {
    let client =
        TestClient::start(app_with_form(tokens(true), Arc::new(AtomicUsize::new(0)))).await;

    let first = issued(&client.get("/form").send().await).expect("a token");
    let mut request = client.get("/form");
    request = request.header("cookie", &format!("{COOKIE}={first}"));
    let second = request.send().await;

    assert_eq!(second.status, 200);
    assert!(
        cookies(&second).is_empty(),
        "a request that already carried a valid token must not be issued another: {:?}",
        cookies(&second)
    );
    assert_eq!(
        second.text(),
        first,
        "the same token comes back to the page"
    );

    client.shutdown().await;
}

/// A planted cookie is replaced on the next safe response rather than trusted.
#[tokio::test]
async fn a_planted_cookie_is_replaced_by_a_signed_one() {
    let client =
        TestClient::start(app_with_form(tokens(true), Arc::new(AtomicUsize::new(0)))).await;

    let mut request = client.get("/form");
    request = request.header("cookie", &format!("{COOKIE}={}", planted()));
    let response = request.send().await;

    assert_eq!(response.status, 200);
    let fresh = issued(&response).expect("the planted value must be replaced");
    assert_ne!(fresh, planted());
    assert_eq!(response.text(), fresh);

    let mut request = client.post("/submit");
    for (name, value) in pair(&fresh) {
        request = request.header(name, &value);
    }
    assert_eq!(
        request.send().await.status,
        200,
        "the replacement token must be usable"
    );

    client.shutdown().await;
}

/// RFC 9110 §9.2.1 safe methods are never gated — a link or a probe must not start failing.
#[tokio::test]
async fn safe_methods_are_not_gated() {
    let client =
        TestClient::start(app_with_form(tokens(true), Arc::new(AtomicUsize::new(0)))).await;

    for (label, response) in [
        ("GET", client.get("/submit").send().await),
        ("HEAD", client.head("/submit").send().await),
        ("OPTIONS", client.options("/submit").send().await),
    ] {
        assert_ne!(
            response.status, 403,
            "{label} is a safe method and must not be refused by the CSRF gate"
        );
    }

    client.shutdown().await;
}

/// The exemption is by raw path prefix, and it covers only what it names.
#[tokio::test]
async fn an_exempt_prefix_is_not_gated() {
    let ran = Arc::new(AtomicUsize::new(0));
    let mut app = AetherApp::new();
    app = app.enable_csrf(CsrfTokens::new(
        SECRET,
        CsrfConfig {
            exempt_prefixes: vec!["/hooks/".to_owned()],
            ..CsrfConfig::default()
        },
    ));
    let counter = Arc::clone(&ran);
    app = app.post("/hooks/stripe", move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        "accepted"
    });
    app = app.post("/submit", |_| "submitted");
    let client = TestClient::start(app).await;

    assert_eq!(client.post("/hooks/stripe").send().await.status, 200);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(
        client.post("/submit").send().await.status,
        403,
        "a sibling route outside the prefix stays gated"
    );
    assert_eq!(
        client.post("/hooks").send().await.status,
        403,
        "the prefix ends where it ends: /hooks is not under /hooks/"
    );

    client.shutdown().await;
}
