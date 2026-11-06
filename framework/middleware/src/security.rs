use aether_http::response::Response;

/// The placeholder a configured policy uses to ask for a per-response nonce, e.g.
/// `script-src 'self' 'nonce-{nonce}'`. It is substituted with a fresh value on every response;
/// a policy without it is emitted byte for byte as configured.
pub const CSP_NONCE_PLACEHOLDER: &str = "{nonce}";

/// Production security headers middleware.
///
/// Defaults follow current OWASP and MDN guidance. Two deliberate notes:
///
/// * `xss_protection` is `"0"`, not `"1; mode=block"`. The legacy XSS auditor is removed from
///   every current browser and, while it existed, it was itself a source of cross-origin
///   information leaks and bypasses. Sending `0` disables it deterministically; sending
///   `1; mode=block` re-enables a known-harmful code path in the few clients that still
///   honour it.
/// * `content_security_policy` and `permissions_policy` default to `None`. Unlike the other
///   headers here, a wrong value for these breaks the application rather than hardening it,
///   so the framework refuses to guess.
///
/// Every header is applied with "set unless already present", so a handler that sets its own
/// `X-Frame-Options` or `Content-Security-Policy` wins over the middleware default instead of
/// producing a duplicate header of ambiguous meaning.
///
/// A policy may name [`CSP_NONCE_PLACEHOLDER`] to receive a fresh nonce on every response; the
/// framework draws it, hands the same value to the handler, and calls
/// [`SecurityHeadersMiddleware::apply_with_nonce`]. A policy without the placeholder is emitted
/// exactly as configured.
#[derive(Debug, Clone)]
pub struct SecurityHeadersMiddleware {
    pub hsts: bool,
    pub frame_options: &'static str,
    pub content_type_options: &'static str,
    pub xss_protection: &'static str,
    pub referrer_policy: Option<&'static str>,
    pub permissions_policy: Option<&'static str>,
    pub content_security_policy: Option<&'static str>,
    pub cross_origin_opener_policy: Option<&'static str>,
    pub cross_origin_resource_policy: Option<&'static str>,
}

impl Default for SecurityHeadersMiddleware {
    fn default() -> Self {
        Self {
            hsts: true,
            frame_options: "DENY",
            content_type_options: "nosniff",
            xss_protection: "0",
            // The browser default, stated explicitly so it cannot be changed by a
            // user-agent update or a proxy.
            referrer_policy: Some("strict-origin-when-cross-origin"),
            permissions_policy: None,
            content_security_policy: None,
            cross_origin_opener_policy: None,
            cross_origin_resource_policy: None,
        }
    }
}

impl SecurityHeadersMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the configured policy asks for a per-response nonce.
    ///
    /// The serving core reads this before dispatch: a nonce is drawn only for an application whose
    /// policy names [`CSP_NONCE_PLACEHOLDER`], so an app that configures no CSP — or one without
    /// inline-script permission — pays nothing for the feature.
    pub fn needs_nonce(&self) -> bool {
        self.content_security_policy
            .is_some_and(|policy| policy.contains(CSP_NONCE_PLACEHOLDER))
    }

    /// Apply security headers to the outgoing response
    ///
    /// Use this only for a policy without [`CSP_NONCE_PLACEHOLDER`]; with one, the placeholder is
    /// dropped rather than left in the header (see [`Self::apply_with_nonce`]).
    pub fn apply(&self, response: &mut Response) {
        self.apply_with_nonce(response, None);
    }

    /// Apply security headers, substituting `nonce` for every [`CSP_NONCE_PLACEHOLDER`].
    ///
    /// `nonce` is the bare token — the value `aether_security::CspNonce::as_str` returns, not its
    /// quoted source expression. It must be the value the handler also wrote on its inline tags;
    /// the framework passes one value to both.
    ///
    /// With `None` — or with an app that never asked the serving core for a nonce — the whole
    /// `'nonce-…'` source expression is *removed* instead of being emitted empty. Leaving
    /// `'nonce-'` in the header would be an invalid source expression that browsers ignore, so the
    /// policy would still allow everything else it names; removing it means a page whose inline
    /// script cannot be authorised is served with a policy that blocks it. That is the fail-closed
    /// direction: the alternative, skipping the header, would serve the page with no CSP at all.
    pub fn apply_with_nonce(&self, response: &mut Response, nonce: Option<&str>) {
        set_unless_present(
            response,
            "X-Content-Type-Options",
            self.content_type_options,
        );
        set_unless_present(response, "X-Frame-Options", self.frame_options);
        set_unless_present(response, "X-XSS-Protection", self.xss_protection);

        if let Some(v) = self.referrer_policy {
            set_unless_present(response, "Referrer-Policy", v);
        }
        if let Some(v) = self.permissions_policy {
            set_unless_present(response, "Permissions-Policy", v);
        }
        if let Some(policy) = self.content_security_policy {
            let rendered: std::borrow::Cow<'_, str> = if policy.contains(CSP_NONCE_PLACEHOLDER) {
                std::borrow::Cow::Owned(render_policy(policy, nonce))
            } else {
                // No placeholder: the configured bytes go out unchanged, with no scan for a
                // string the author did not write.
                std::borrow::Cow::Borrowed(policy)
            };
            set_unless_present(response, "Content-Security-Policy", &rendered);
        }
        if let Some(v) = self.cross_origin_opener_policy {
            set_unless_present(response, "Cross-Origin-Opener-Policy", v);
        }
        if let Some(v) = self.cross_origin_resource_policy {
            set_unless_present(response, "Cross-Origin-Resource-Policy", v);
        }

        if self.hsts {
            set_unless_present(
                response,
                "Strict-Transport-Security",
                "max-age=31536000; includeSubDomains",
            );
        }
    }
}

/// Fill the nonce placeholder in `policy`, or remove the source expression that asks for one.
fn render_policy(policy: &str, nonce: Option<&str>) -> String {
    if let Some(nonce) = nonce {
        return policy.replace(CSP_NONCE_PLACEHOLDER, nonce);
    }
    // With no nonce, drop every whitespace-delimited expression that mentions the placeholder —
    // `'nonce-{nonce}'` when the author quoted it, and the bare form if they did not. Re-joining
    // with single spaces is safe: a policy is a semicolon-separated list of directives, each a
    // whitespace-separated list of expressions, so only the separators carry meaning. A directive
    // left with no expressions (`script-src` alone) permits nothing, which is the strict answer
    // for the one permission that could not be granted.
    policy
        .split(';')
        .map(|directive| {
            directive
                .split_whitespace()
                .filter(|expression| !expression.contains(CSP_NONCE_PLACEHOLDER))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("; ")
        .trim()
        .to_string()
}

fn set_unless_present(response: &mut Response, key: &'static str, value: &str) {
    if response
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        return;
    }
    response.add_header(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(res: &Response, name: &str) -> Vec<String> {
        res.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .collect()
    }

    #[test]
    fn test_defaults_match_current_guidance() {
        let mw = SecurityHeadersMiddleware::new();
        // Regression: "1; mode=block" re-enables a removed, known-harmful XSS auditor.
        assert_eq!(mw.xss_protection, "0");
        assert_eq!(mw.content_type_options, "nosniff");
        assert_eq!(mw.frame_options, "DENY");
        assert!(mw.hsts);
    }

    #[test]
    fn test_apply_sets_baseline_headers_once() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        assert_eq!(header(&res, "X-Content-Type-Options"), vec!["nosniff"]);
        assert_eq!(header(&res, "X-Frame-Options"), vec!["DENY"]);
        assert_eq!(header(&res, "X-XSS-Protection"), vec!["0"]);
        assert_eq!(
            header(&res, "Strict-Transport-Security"),
            vec!["max-age=31536000; includeSubDomains"]
        );
        assert_eq!(
            header(&res, "Referrer-Policy"),
            vec!["strict-origin-when-cross-origin"]
        );
    }

    #[test]
    fn test_handler_supplied_header_is_not_duplicated() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        res.add_header("X-Frame-Options", "SAMEORIGIN");
        res.add_header("x-content-type-options", "nosniff");

        mw.apply(&mut res);
        mw.apply(&mut res);

        assert_eq!(header(&res, "X-Frame-Options"), vec!["SAMEORIGIN"]);
        assert_eq!(header(&res, "x-content-type-options"), vec!["nosniff"]);
    }

    #[test]
    fn test_optional_headers_are_off_by_default() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        // A guessed CSP or Permissions-Policy breaks applications, so the framework must not
        // emit one unless the owner configured it.
        assert!(header(&res, "Content-Security-Policy").is_empty());
        assert!(header(&res, "Permissions-Policy").is_empty());
        assert!(header(&res, "Cross-Origin-Opener-Policy").is_empty());
        assert!(header(&res, "Cross-Origin-Resource-Policy").is_empty());
    }

    #[test]
    fn test_configured_optional_headers_are_applied() {
        let mw = SecurityHeadersMiddleware {
            content_security_policy: Some("default-src 'self'"),
            permissions_policy: Some("geolocation=()"),
            cross_origin_resource_policy: Some("same-origin"),
            hsts: false,
            ..SecurityHeadersMiddleware::default()
        };
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        assert_eq!(
            header(&res, "Content-Security-Policy"),
            vec!["default-src 'self'"]
        );
        assert_eq!(header(&res, "Permissions-Policy"), vec!["geolocation=()"]);
        assert_eq!(
            header(&res, "Cross-Origin-Resource-Policy"),
            vec!["same-origin"]
        );
        assert!(header(&res, "Strict-Transport-Security").is_empty());
    }

    fn with_policy(policy: &'static str) -> SecurityHeadersMiddleware {
        SecurityHeadersMiddleware {
            content_security_policy: Some(policy),
            ..SecurityHeadersMiddleware::default()
        }
    }

    #[test]
    fn test_only_a_policy_that_names_the_placeholder_wants_a_nonce() {
        assert!(!with_policy("default-src 'self'").needs_nonce());
        assert!(with_policy("script-src 'self' 'nonce-{nonce}'").needs_nonce());
        assert!(
            !SecurityHeadersMiddleware::default().needs_nonce(),
            "no policy at all asks for nothing"
        );
    }

    #[test]
    fn test_a_policy_without_the_placeholder_is_emitted_verbatim() {
        // Two spaces are the author's, not a formatting slip to be repaired: a policy this
        // middleware has no reason to touch must reach the client byte for byte.
        let policy = "default-src 'self';  script-src 'self' 'sha256-abc='";
        let mw = with_policy(policy);
        let mut res = Response::text("hi");
        mw.apply_with_nonce(&mut res, Some("deadbeef"));
        assert_eq!(header(&res, "Content-Security-Policy"), vec![policy]);
    }

    #[test]
    fn test_the_same_nonce_the_handler_was_given_fills_the_policy() {
        let mw = with_policy("default-src 'self'; script-src 'self' 'nonce-{nonce}'");
        let mut res = Response::text("<script nonce=\"abc123\">");
        mw.apply_with_nonce(&mut res, Some("abc123"));
        assert_eq!(
            header(&res, "Content-Security-Policy"),
            vec!["default-src 'self'; script-src 'self' 'nonce-abc123'"]
        );
    }

    #[test]
    fn test_a_missing_nonce_removes_the_expression_and_keeps_the_policy() {
        // Fail-closed: an empty `'nonce-'` would be an invalid expression every browser ignores,
        // and dropping the header would leave the page with no policy at all.
        let mw = with_policy("default-src 'self'; script-src 'self' 'nonce-{nonce}'");
        let mut res = Response::text("hi");
        mw.apply_with_nonce(&mut res, None);
        assert_eq!(
            header(&res, "Content-Security-Policy"),
            vec!["default-src 'self'; script-src 'self'"]
        );
    }

    #[test]
    fn test_a_directive_left_with_only_the_nonce_allows_nothing() {
        let mw = with_policy("default-src 'self'; script-src 'nonce-{nonce}'");
        let mut res = Response::text("hi");
        mw.apply_with_nonce(&mut res, None);
        assert_eq!(
            header(&res, "Content-Security-Policy"),
            vec!["default-src 'self'; script-src"],
            "an empty source list is a directive that permits no source"
        );
        assert!(
            !header(&res, "Content-Security-Policy")[0].contains(CSP_NONCE_PLACEHOLDER),
            "the placeholder must never reach the wire"
        );
    }

    #[test]
    fn test_an_unquoted_placeholder_is_also_removed_without_a_nonce() {
        let mw = with_policy("script-src {nonce} 'self'");
        let mut res = Response::text("hi");
        mw.apply_with_nonce(&mut res, None);
        assert_eq!(
            header(&res, "Content-Security-Policy"),
            vec!["script-src 'self'"]
        );
    }
}
