//! End-to-end tests for signed cookie values (PRR §8 item 7, the half this track closed).
//!
//! Every request here goes over a real socket: what a signed cookie protects against is a value
//! that arrives on the wire from somewhere other than this server's own `Set-Cookie`, so a test
//! that called `CookieSigner` directly would prove the cryptography and nothing about the request
//! path — not the duplicate-name refusal, not the attribute encoder leaving the value alone, not
//! the literal name comparison a browser actually answers.

use std::sync::Arc;

use aether::prelude::*;
use aether::security::CookieSigner;
use aether::security::jwt::encode_base64url;

const SECRET: &[u8] = b"wire-cookie-secret-0123456789abcdef";
const OTHER_SECRET: &[u8] = b"the-attackers-own-secret-0123456789";

fn signer() -> Arc<CookieSigner> {
    Arc::new(CookieSigner::new(SECRET))
}

/// The four outcomes, answered with four statuses, so the wire can tell them apart.
fn classify(req: &RequestContext, signer: &CookieSigner) -> Response {
    match req.signed_cookie(signer, "session") {
        SignedCookie::Present(payload) => Response::text_dynamic(payload),
        SignedCookie::Absent => Response::status(401, "Unauthorized"),
        SignedCookie::Invalid => Response::status(403, "Forbidden"),
        SignedCookie::Ambiguous => Response::status(409, "Conflict"),
    }
}

/// `GET /login` sets one signed `session` cookie; `GET /whoami` classifies the one it receives.
fn app(signer: Arc<CookieSigner>) -> AetherApp {
    let mut app = AetherApp::new();
    let set_side = Arc::clone(&signer);
    app = app.get("/login", move |_: RequestContext| {
        let value = set_side.value("session", "user-42").expect("a token name");
        Response::ok().set_cookie("session", &value, true, true, "Lax", Some(3600))
    });
    app = app.get("/whoami", move |req: RequestContext| {
        classify(&req, &signer)
    });
    app
}

/// The single `Set-Cookie` line a response carried, with its attributes.
fn issued(response: &TestResponse) -> String {
    let mut found: Option<String> = None;
    for (name, value) in &response.headers {
        if name != "Set-Cookie" {
            continue;
        }
        assert!(found.is_none(), "two Set-Cookie lines on one response");
        found = Some(value.clone());
    }
    found.expect("a Set-Cookie line")
}

/// The `session=<value>` pair out of that line, attributes dropped.
fn signed_pair(response: &TestResponse) -> String {
    issued(response)
        .split(';')
        .next()
        .expect("the name=value pair")
        .to_owned()
}

/// The signed value for `session`, produced without a socket: the tests below need the bytes, not
/// a response to read them out of.
fn issued_value(signer: &CookieSigner) -> String {
    signer.value("session", "user-42").expect("a token name")
}

#[tokio::test]
async fn a_signed_cookie_round_trips_over_the_wire() {
    let client = TestClient::start(app(signer())).await;

    let login = client.get("/login").send().await;
    assert_eq!(login.status, 200);
    let pair = signed_pair(&login);
    assert!(pair.starts_with("session="), "{pair}");

    let whoami = client.get("/whoami").header("cookie", &pair).send().await;
    assert_eq!(whoami.status, 200);
    assert_eq!(whoami.text(), "user-42");

    client.shutdown().await;
}

#[tokio::test]
async fn the_signed_value_is_sent_unencoded_and_its_attributes_survive() {
    // The payload is `user-42`, the wire carries base64url of it. Neither half needs
    // percent-encoding, so what the request lookup compares literally is byte-for-byte what
    // `CookieSigner::value` wrote — the framework's own attribute encoder changed nothing.
    let client = TestClient::start(app(signer())).await;
    let login = client.get("/login").send().await;
    let line = issued(&login);

    let (pair, attributes) = line.split_once(';').expect("attributes after the pair");
    let (_, value) = pair.split_once('=').expect("a value");
    assert!(
        value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "{value}"
    );
    assert_eq!(value.split('.').count(), 2, "{value}");
    for expected in [
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Path=/",
        "Max-Age=3600",
    ] {
        assert!(attributes.contains(expected), "{attributes}");
    }

    client.shutdown().await;
}

#[tokio::test]
async fn a_payload_with_cookie_delimiters_still_round_trips() {
    // The signature makes the value opaque, so a payload full of `;`, `,`, quotes and spaces
    // survives both the encoder on the way out and the parser on the way back in.
    let signer = Arc::new(CookieSigner::new(SECRET));
    let mut app = AetherApp::new();
    let set_side = Arc::clone(&signer);
    app = app.get("/login", move |_: RequestContext| {
        let value = set_side
            .value("session", "user 42; role=admin, \"x\"")
            .expect("a token name");
        Response::ok().set_cookie("session", &value, false, false, "Lax", None)
    });
    app = app.get("/whoami", move |req: RequestContext| {
        match req.signed_cookie(&signer, "session") {
            SignedCookie::Present(payload) => Response::text_dynamic(payload),
            SignedCookie::Absent => Response::status(401, "Unauthorized"),
            SignedCookie::Invalid => Response::status(403, "Forbidden"),
            SignedCookie::Ambiguous => Response::status(409, "Conflict"),
        }
    });
    let client = TestClient::start(app).await;

    let login = client.get("/login").send().await;
    let pair = signed_pair(&login);
    let whoami = client.get("/whoami").header("cookie", &pair).send().await;
    assert_eq!(whoami.status, 200);
    assert_eq!(whoami.text(), "user 42; role=admin, \"x\"");

    client.shutdown().await;
}

#[tokio::test]
async fn a_missing_cookie_is_absent_not_invalid() {
    let client = TestClient::start(app(signer())).await;

    let response = client.get("/whoami").send().await;
    assert_eq!(response.status, 401);

    client.shutdown().await;
}

#[tokio::test]
async fn a_plain_unsigned_cookie_is_refused() {
    // The shape an attacker reaches for first: no cryptography at all, just the cookie the app
    // would have trusted before this type existed.
    let client = TestClient::start(app(signer())).await;

    let response = client
        .get("/whoami")
        .header("cookie", "session=user-42")
        .send()
        .await;
    assert_eq!(response.status, 403);

    client.shutdown().await;
}

#[tokio::test]
async fn a_value_signed_with_another_secret_is_refused() {
    // Exactly what a sibling subdomain or an XSS payload can plant: the right shape, a real MAC,
    // and no key this server accepts behind it.
    let planted = CookieSigner::new(OTHER_SECRET)
        .value("session", "user-admin")
        .expect("a token name");
    let client = TestClient::start(app(signer())).await;

    let response = client
        .get("/whoami")
        .header("cookie", &format!("session={planted}"))
        .send()
        .await;
    assert_eq!(response.status, 403);

    client.shutdown().await;
}

#[tokio::test]
async fn a_value_issued_for_another_name_is_refused() {
    // The transplant the name is bound into the MAC for: `theme` carries a value this server
    // signed, and the caller offers it as `session`.
    let theme = signer().value("theme", "user-42").expect("a token name");
    let client = TestClient::start(app(signer())).await;

    let response = client
        .get("/whoami")
        .header("cookie", &format!("session={theme}"))
        .send()
        .await;
    assert_eq!(response.status, 403);

    client.shutdown().await;
}

#[tokio::test]
async fn a_tampered_payload_with_a_real_mac_is_refused() {
    let client = TestClient::start(app(signer())).await;
    let login = client.get("/login").send().await;
    let pair = signed_pair(&login);
    let mac = pair
        .split_once('=')
        .expect("a value")
        .1
        .split_once('.')
        .expect("two halves")
        .1
        .to_owned();
    let forged = format!("session={}.{}", encode_base64url(b"user-43"), mac);

    let response = client.get("/whoami").header("cookie", &forged).send().await;
    assert_eq!(response.status, 403);

    client.shutdown().await;
}

#[tokio::test]
async fn one_name_carried_twice_is_ambiguous() {
    // Two values under one name, whether split across `Cookie` lines or joined on one: the same
    // refusal `CsrfTokens` makes against cookie tossing, and the same reason — the server has no
    // basis to choose, and taking the first or the last is a coin flip it should not accept.
    let signer = signer();
    let good = format!("session={}", issued_value(&signer));
    let planted = format!(
        "session={}",
        CookieSigner::new(OTHER_SECRET)
            .value("session", "user-admin")
            .expect("a token name")
    );
    let both_on_one_line = format!("{good}; {planted}");
    let two_lines = [good.clone(), planted.clone()];
    let the_same_twice = [good.clone(), good.clone()];

    let client = TestClient::start(app(signer)).await;
    for headers in [
        vec![both_on_one_line.as_str()],
        vec![two_lines[0].as_str(), two_lines[1].as_str()],
        vec![the_same_twice[0].as_str(), the_same_twice[1].as_str()],
    ] {
        let mut request = client.get("/whoami");
        for header in headers {
            request = request.header("cookie", header);
        }
        let response = request.send().await;
        assert_eq!(
            response.status, 409,
            "a duplicated name resolved by taking one"
        );
    }

    client.shutdown().await;
}

#[tokio::test]
async fn a_name_that_is_a_prefix_or_a_case_variant_does_not_match() {
    // Whole-name comparison, case-sensitively as RFC 6265 §5.2 stores it: none of these is
    // `session`, so the request carries nothing the accessor was asked for.
    let signer = signer();
    let value = issued_value(&signer);
    let client = TestClient::start(app(signer)).await;

    for header in [
        format!("Session={value}"),
        format!("SESSION={value}"),
        format!("sessions={value}"),
        format!("x-session={value}"),
        format!("sess={value}"),
    ] {
        let response = client.get("/whoami").header("cookie", &header).send().await;
        assert_eq!(response.status, 401, "{header} matched `session`");
    }

    client.shutdown().await;
}

#[tokio::test]
async fn a_signed_value_for_one_name_is_readable_under_that_name_only() {
    // The positive control for the two refusals above: the same bytes, asked for under the name
    // that signed them, are `Present`.
    let signer = signer();
    let theme = signer.value("theme", "dark").expect("a token name");
    let mut app = AetherApp::new();
    let reader = Arc::clone(&signer);
    app = app.get("/theme", move |req: RequestContext| {
        match req.signed_cookie(&reader, "theme") {
            SignedCookie::Present(payload) => Response::text_dynamic(payload),
            SignedCookie::Absent => Response::status(401, "Unauthorized"),
            SignedCookie::Invalid => Response::status(403, "Forbidden"),
            SignedCookie::Ambiguous => Response::status(409, "Conflict"),
        }
    });
    let client = TestClient::start(app).await;

    let response = client
        .get("/theme")
        .header("cookie", &format!("theme={theme}"))
        .send()
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "dark");

    client.shutdown().await;
}
