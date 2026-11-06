//! `Host` header allow-listing.
//!
//! A server that answers for any `Host` it is addressed as will happily serve the wrong
//! application behind a misconfigured vhost, mint password-reset links pointing at an
//! attacker's domain, and answer absolute-form targets (`GET http://evil.com/ HTTP/1.1`)
//! that a proxy forwarded to it. The frameworks this competes with leave it to the
//! deployment layer; that is a choice, not a safe default.
//!
//! An absolute-form target never reaches this check as the `Host` field it arrived with: the
//! HTTP/1 parser replaces that field with the target's authority (RFC 9112 §3.2.2), and the h2
//! path folds `:authority` in the same way. So the value compared here is always the host the
//! request actually names, whichever form it used, and the check below sees the same string a
//! route or handler would.
//!
//! Matching rules, in this order:
//!
//! * The authority is lowercased and its port stripped, because `Host: Example.COM:8080`
//!   names the same resource as `Example.com`.
//! * A single trailing `.` is the FQDN root label, not a different name, so it is stripped
//!   from both the configured entries and the incoming value.
//! * Any authority containing `@` is rejected outright. `http://evil.com@good.com` and its
//!   reverse are the classic split between what a proxy parses as the host and what the
//!   origin reads from the header.
//! * An entry beginning with `*.` matches subdomains at any depth (`a.example.com`,
//!   `a.b.example.com`) and never the bare domain, so `*.example.com` cannot be satisfied
//!   by `notexample.com` either — the dot is part of the pattern.

use aether_http::response::Response;

#[derive(Debug, Clone)]
struct Entry {
    /// Lowercased, port- and root-dot-stripped label to compare against, e.g.
    /// `example.com` or `.example.com` for a wildcard.
    label: String,
    wildcard: bool,
}

/// A configured `Host` allow-list.
#[derive(Debug, Clone)]
pub struct HostValidation {
    entries: Vec<Entry>,
}

impl HostValidation {
    /// Build a validator from host names. Invalid entries are reported rather than
    /// silently dropped — a typo in an allow-list fails closed and breaks production.
    pub fn new<I, S>(allowed: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut entries = Vec::new();
        for raw in allowed {
            let raw = raw.as_ref();
            let normalized = normalize(raw).ok_or_else(|| {
                format!("invalid allowed host {raw:?}: a host must be a name or *.name, with no scheme, path, port or credentials")
            })?;
            if normalized.is_empty() {
                return Err(format!("empty allowed host entry {raw:?}"));
            }
            let wildcard = normalized.starts_with('.');
            entries.push(Entry {
                label: normalized,
                wildcard,
            });
        }
        if entries.is_empty() {
            return Err("host validation needs at least one allowed host".to_string());
        }
        Ok(Self { entries })
    }

    /// True when `host_header` names one of the allowed hosts. A missing value is not
    /// allowed: the HTTP/1.1 parser already rejects a request without `Host`, so reaching
    /// here with `None` means an empty field, which is equally unusable.
    pub fn accepts(&self, host_header: Option<&str>) -> bool {
        let Some(raw) = host_header else {
            return false;
        };
        let Some(host) = normalize(raw) else {
            return false;
        };
        // `normalize` keeps a wildcard as `.domain` because that is how the *config* is
        // stored; a client that sends `Host: *.example.com` is presenting a pattern, not a
        // name, and never matches.
        if host.is_empty() || host.starts_with('.') {
            return false;
        }
        self.entries.iter().any(|e| {
            if e.wildcard {
                // `host.ends_with(&e.label)` alone would also accept the bare domain when
                // the label were `example.com`; the stored label always keeps its leading
                // dot, so a suffix match here is a subdomain match.
                host.len() > e.label.len() && host.ends_with(&e.label)
            } else {
                host == e.label
            }
        })
    }

    /// A rejection response, or `None` when the request may proceed. The body is fixed
    /// text: echoing the offending host would hand an attacker a reflection point.
    pub fn reject(&self, host_header: Option<&str>) -> Option<Response> {
        if self.accepts(host_header) {
            return None;
        }
        let mut res = Response::status(400, "Bad Request");
        res.body = bytes::Bytes::from_static(b"The Host header is not allowed for this service");
        Some(res)
    }
}

/// Lowercase, strip a port and a trailing root dot, and reject anything that is not a bare
/// host: scheme, path, query, credentials or an empty label.
fn normalize(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // A `Host` field carries an authority, never a URI: a scheme, credentials, a query or a
    // fragment means either a broken client or someone testing how this server splits the
    // authority from the rest.
    if raw.is_empty()
        || raw.contains('@')
        || raw.contains('?')
        || raw.contains('#')
        || raw.contains("://")
    {
        return None;
    }
    let authority = raw;
    // A remaining `/` is a path segment.
    if authority.contains('/') {
        return None;
    }

    let host = if let Some(rest) = authority.strip_prefix('[') {
        // Bracketed IPv6, with the optional port after the closing bracket.
        let end = rest.find(']')?;
        match rest.get(end + 1..) {
            None | Some("") => {}
            Some(tail) => {
                let port = tail.strip_prefix(':')?;
                if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
            }
        }
        &rest[..end]
    } else {
        // A bracketless colon is either a port or a bare IPv6, and only one of them is a
        // host this matcher can compare: `foo:bar` and `::1` are refused, `foo:8080` is not.
        if authority.matches(':').count() > 1 {
            return None;
        }
        match authority.rsplit_once(':') {
            Some((name, port))
                if !name.is_empty()
                    && !port.is_empty()
                    && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                name
            }
            Some(_) => return None,
            None => authority,
        }
    };

    let mut lowered = host.to_ascii_lowercase();
    // The FQDN root label is not part of the name.
    if lowered.ends_with('.') {
        lowered.pop();
    }
    if lowered.is_empty() {
        return None;
    }
    // A wildcard is stored as the dot plus the domain so a suffix match is unambiguous, and
    // must name at least one label below a TLD (`*.com` would list every registration).
    if let Some(rest) = lowered.strip_prefix("*.") {
        if !rest.contains('.') {
            return None;
        }
        lowered = format!(".{rest}");
    } else if lowered.contains('*') {
        // `*.` after the root-dot strip, or a `*` anywhere else, is not a host name.
        return None;
    }
    if lowered
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b == b'\r' || b == b'\n' || b == 0)
    {
        return None;
    }
    Some(lowered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(allowed: &[&str]) -> HostValidation {
        HostValidation::new(allowed).expect("valid allow-list")
    }

    #[test]
    fn exact_host_matches_independent_of_case_and_port() {
        let h = v(&["example.com"]);
        for candidate in [
            "example.com",
            "Example.COM",
            "example.com:8080",
            "EXAMPLE.com:443",
            "example.com.",
        ] {
            assert!(h.accepts(Some(candidate)), "{candidate} must match");
        }
    }

    #[test]
    fn rejects_other_hosts_and_nothing_is_an_empty_match() {
        let h = v(&["example.com"]);
        for candidate in [
            "notexample.com",
            "example.org",
            "sub.example.com",
            "evil.com/#@example.com",
            "e.com",
            "",
        ] {
            assert!(!h.accepts(Some(candidate)), "{candidate} must not match");
        }
        assert!(!h.accepts(None));
    }

    #[test]
    fn userinfo_cannot_smuggle_a_matching_host() {
        let h = v(&["good.com"]);
        for candidate in [
            "good.com@evil.com",
            "evil.com@good.com",
            "@good.com",
            "http://evil.com@good.com",
        ] {
            assert!(!h.accepts(Some(candidate)), "{candidate} must be refused");
        }
    }

    #[test]
    fn scheme_and_path_are_not_allowed_in_the_config_or_the_header() {
        assert!(HostValidation::new(["http://good.com"]).is_err());
        assert!(HostValidation::new(["good.com/path"]).is_err());
        assert!(HostValidation::new(["good.com:notaport"]).is_err());
        assert!(HostValidation::new(Vec::<String>::new()).is_err());
        let h = v(&["good.com"]);
        assert!(!h.accepts(Some("http://good.com")));
        assert!(!h.accepts(Some("good.com/admin")));
    }

    #[test]
    fn wildcard_matches_subdomains_at_any_depth_but_not_the_apex() {
        let h = v(&["*.example.com"]);
        assert!(h.accepts(Some("a.example.com")));
        assert!(h.accepts(Some("A.B.EXAMPLE.com:443")));
        assert!(h.accepts(Some("deep.sub.example.com")));
        assert!(
            !h.accepts(Some("example.com")),
            "the apex is a different name"
        );
        assert!(!h.accepts(Some("notexample.com")));
        assert!(!h.accepts(Some("example.com.evil.net")));
        assert!(HostValidation::new(["*."]).is_err());
        assert!(
            HostValidation::new(["*.com"]).is_err(),
            "a TLD wildcard would list the whole zone"
        );
    }

    #[test]
    fn mixes_exact_and_wildcard_entries() {
        let h = v(&["example.com", "*.example.com", "localhost", "[::1]"]);
        assert!(h.accepts(Some("example.com")));
        assert!(h.accepts(Some("api.example.com")));
        assert!(h.accepts(Some("localhost")));
        assert!(h.accepts(Some("localhost:3000")));
        assert!(h.accepts(Some("[::1]")));
        assert!(h.accepts(Some("[::1]:8080")));
        assert!(!h.accepts(Some("127.0.0.2")));
    }

    #[test]
    fn rejection_is_400_without_reflecting_the_host() {
        let h = v(&["example.com"]);
        let res = h
            .reject(Some("evil.com<script>"))
            .expect("a non-allowed host is rejected");
        assert_eq!(res.status, 400);
        assert!(
            !res.body.windows(5).any(|w| w == b"evil."),
            "the body reflects the host: {:?}",
            res.body
        );
        // A framing-legal empty body must not become a second Content-Length source.
        assert!(
            !res.headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
        );
    }
}
