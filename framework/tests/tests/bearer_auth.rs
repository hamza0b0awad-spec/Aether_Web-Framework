//! End-to-end tests for the bearer-authentication layer (F-13).
//!
//! The tokens here are minted by the test with the same HMAC primitive the server verifies with;
//! the server never signs, so no token-issuing path was added to `aether` to make these pass.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aether::prelude::*;
use aether::security::jwt::encode_base64url;
use bytes::Bytes;
use ring::hmac;
use serde_json::{Value, json};

const SECRET: &[u8] = b"wire-bearer-secret-0123456789abcdef";
/// 2100-01-01T00:00:00Z — valid for the lifetime of any run.
const FAR_FUTURE: u64 = 4_102_444_800;
/// 2001-09-09T01:46:40Z — expired long before any run.
const LONG_PAST: u64 = 1_000_000_000;

fn mint(secret: &[u8], header: &str, payload: &str) -> String {
    let signed = format!(
        "{}.{}",
        encode_base64url(header.as_bytes()),
        encode_base64url(payload.as_bytes())
    );
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let tag = hmac::sign(&key, signed.as_bytes());
    format!("{signed}.{}", encode_base64url(tag.as_ref()))
}

fn bearer_token(subject: &str) -> String {
    mint(
        SECRET,
        r#"{"alg":"HS256","typ":"JWT"}"#,
        &format!(r#"{{"sub":"{subject}","role":"admin","exp":{FAR_FUTURE}}}"#),
    )
}

/// Every credential that is not a valid token gets one answer, and the route never runs.
#[tokio::test]
async fn every_invalid_credential_gets_the_same_401_and_the_route_never_runs() {
    let auth = BearerAuth::new(SECRET, JwtValidationOptions::default());
    let ran = Arc::new(AtomicUsize::new(0));

    let mut app = AetherApp::new();
    let counter = Arc::clone(&ran);
    app = app.route_sync_with(Method::Get, "/private", require_bearer(&auth), move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        "secret"
    });
    let client = TestClient::start(app).await;

    let good = bearer_token("alice");
    let forged = mint(
        b"a-different-secret",
        r#"{"alg":"HS256"}"#,
        &format!(r#"{{"sub":"alice","exp":{FAR_FUTURE}}}"#),
    );
    let expired = mint(
        SECRET,
        r#"{"alg":"HS256"}"#,
        &format!(r#"{{"sub":"alice","exp":{LONG_PAST}}}"#),
    );
    let wrong_alg = mint(
        SECRET,
        r#"{"alg":"none"}"#,
        &format!(r#"{{"sub":"alice","exp":{FAR_FUTURE}}}"#),
    );
    let two_segments = good.rsplit_once('.').expect("three segments").0.to_string();

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no Authorization field", vec![]),
        (
            "an empty value",
            vec![("Authorization".into(), String::new())],
        ),
        (
            "another scheme",
            vec![("Authorization".into(), format!("Basic {good}"))],
        ),
        (
            "a scheme without credentials",
            vec![("Authorization".into(), "Bearer".into())],
        ),
        (
            "a token with two segments",
            vec![("Authorization".into(), format!("Bearer {two_segments}"))],
        ),
        (
            "a forged signature",
            vec![("Authorization".into(), format!("Bearer {forged}"))],
        ),
        (
            "an expired token",
            vec![("Authorization".into(), format!("Bearer {expired}"))],
        ),
        (
            "an alg other than the pinned one",
            vec![("Authorization".into(), format!("Bearer {wrong_alg}"))],
        ),
        (
            "two credentials on one field",
            vec![
                ("Authorization".into(), format!("Bearer {good}")),
                ("Authorization".into(), format!("Bearer {forged}")),
            ],
        ),
    ];

    let mut first: Option<(u16, Option<String>, String)> = None;
    for (label, headers) in cases {
        let mut request = client.get("/private");
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        let response = request.send().await;
        let shape = (
            response.status,
            response.header("www-authenticate").map(str::to_owned),
            response.text().into_owned(),
        );
        assert_eq!(shape.0, 401, "{label}: status");
        assert_eq!(
            shape.1.as_deref(),
            Some("Bearer error=\"invalid_token\""),
            "{label}: challenge"
        );
        assert!(
            shape.2.is_empty(),
            "{label}: the body must not explain the refusal"
        );
        match &first {
            None => first = Some(shape),
            Some(first) => assert_eq!(first, &shape, "{label}: refusal differs from the first"),
        }
    }

    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "the handler must not run for any refused request"
    );
    client.shutdown().await;
}

/// A verified token puts its subject and claims where the handler reads them.
#[tokio::test]
async fn a_valid_token_reaches_the_handler_as_a_principal() {
    let auth = BearerAuth::new(SECRET, JwtValidationOptions::default());
    let mut app = AetherApp::new();
    app = app.route_sync_with(
        Method::Get,
        "/me",
        require_bearer(&auth),
        |ctx: RequestContext| match ctx.extension::<Principal>() {
            Some(principal) => Response::json(Bytes::from(
                json!({
                    "subject": principal.subject,
                    "role": principal.claims.get("role").cloned().unwrap_or(Value::Null),
                })
                .to_string(),
            )),
            None => Response::status(500, "authenticated request carried no principal"),
        },
    );
    let client = TestClient::start(app).await;

    let token = bearer_token("alice");
    for credential in [format!("Bearer {token}"), format!("bearer {token}")] {
        let response = client
            .get("/me")
            .header("authorization", &credential)
            .send()
            .await;
        assert_eq!(response.status, 200, "credential {credential}");
        let body: Value = response.json().expect("JSON body");
        assert_eq!(body["subject"], "alice");
        assert_eq!(body["role"], "admin");
    }
    client.shutdown().await;
}

/// Registering the layer on one route does not touch the routes beside it.
#[tokio::test]
async fn a_per_route_layer_leaves_public_routes_alone() {
    let auth = BearerAuth::new(SECRET, JwtValidationOptions::default());
    let mut app = AetherApp::new();
    app = app.get("/public", |_| "open");
    app = app.route_sync_with(Method::Get, "/private", require_bearer(&auth), |_| "closed");
    // Registered after the layer, and still public: the chain is bound to the registration.
    app = app.get("/also-public", |_| "open");
    let client = TestClient::start(app).await;

    for path in ["/public", "/also-public"] {
        let response = client.get(path).send().await;
        assert_eq!(response.status, 200, "{path} must not require a token");
        assert_eq!(response.text(), "open");
        assert_eq!(
            response.header("www-authenticate"),
            None,
            "{path} is not a bearer route"
        );
    }

    let token = bearer_token("alice");
    let response = client
        .get("/private")
        .header("authorization", &format!("Bearer {token}"))
        .send()
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "closed");

    let refused = client.get("/private").send().await;
    assert_eq!(refused.status, 401);

    client.shutdown().await;
}
