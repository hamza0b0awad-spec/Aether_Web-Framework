//! The credential-shaped values the framework hands out must not carry their secret into a log.
//!
//! Track 103 decided secrets stay constructor arguments — the deployment docs' environment
//! variables — rather than a wrapper type nobody is required to use. What replaces the wrapper is
//! this: the types that *hold* a key implement no `Debug` (a `compile_fail` doc test on each one
//! fails if that changes), the one credential-shaped value that is formattable prints a
//! placeholder, and the refusal a bad credential earns carries neither the credential nor a
//! reason.

use aether_security::{BearerAuth, BearerError, CsrfError, CsrfToken, JwtValidationOptions};

const SECRET: &[u8] = b"track-103-secret-sentinel-0123456789";

#[test]
fn the_csrf_token_renders_a_placeholder() {
    let sentinel = "csrf-token-sentinel-9f3a41";
    let token = CsrfToken::new(sentinel);
    assert_eq!(
        token.as_str(),
        sentinel,
        "the deliberate accessor must still return the text"
    );

    let rendered = format!("{token:?}");
    assert!(
        !rendered.contains(sentinel),
        "Debug must not carry the token: {rendered}"
    );
    assert!(
        rendered.contains("redacted"),
        "Debug must say that it redacted: {rendered}"
    );
}

#[test]
fn a_refusal_carries_no_credential() {
    let sentinel = "credential-sentinel-4c1d";
    let refusal = BearerAuth::new(SECRET, JwtValidationOptions::default())
        .authenticate(&[&format!("Bearer {sentinel}")])
        .expect_err("an unsigned credential is refused");
    assert_eq!(refusal, BearerError);

    for rendered in [refusal.to_string(), format!("{refusal:?}")] {
        assert!(
            !rendered.contains(sentinel),
            "the refusal must not echo the credential: {rendered}"
        );
    }
    let csrf = format!("{CsrfError} {CsrfError:?}");
    assert!(
        csrf.contains("entropy"),
        "the CSRF refusal must name the cause: {csrf}"
    );
}
