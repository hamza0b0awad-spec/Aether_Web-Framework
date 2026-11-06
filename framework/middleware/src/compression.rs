//! # Compression Middleware (Gzip & Brotli)
//!
//! Automatically compresses response bodies based on Accept-Encoding header.

use aether_http::response::Response;
use bytes::Bytes;

#[derive(Clone, Debug)]
pub struct CompressionConfig {
    pub enable_gzip: bool,
    pub enable_brotli: bool,
    pub min_compression_size: usize,
    pub compression_level: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enable_gzip: true,
            enable_brotli: false,       // Brotli is slower, disable by default
            min_compression_size: 1024, // Only compress responses >= 1KB
            compression_level: 6,       // Default balance between speed and compression
        }
    }
}

pub struct CompressionMiddleware {
    config: CompressionConfig,
}

impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self::new(CompressionConfig::default())
    }
}

impl CompressionMiddleware {
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }

    /// Apply compression to response based on Accept-Encoding header
    pub fn apply(&self, response: &mut Response, accept_encoding: Option<&str>) {
        if aether_http::response::status_forbids_body(response.status) {
            return;
        }
        let accept = accept_encoding.unwrap_or("");
        // RFC 9110 §12.5.1: identity is the representation as stored, and a field that
        // excludes it (`identity;q=0`, or `*;q=0` with no more specific entry) means an
        // untouched body is not an acceptable answer. This middleware negotiated codings,
        // so it must honour that on every path that would otherwise emit one.
        let identity_ok = accepts_identity(accept);

        // A coding the handler already chose is that handler's answer to `Accept-Encoding`.
        // This middleware has nothing to add: it will neither overwrite the field nor refuse
        // a representation it has not compared against the request.
        if response
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
        {
            return;
        }

        // An empty payload is the same bytes under any coding, so there is no representation
        // to negotiate: a redirect or a framework error must keep its status even for a client
        // that excluded identity. (A streamed body also keeps `body` empty, but it has bytes
        // on the way and is handled as untransformable below.)
        if response.body.is_empty() && !response.is_streaming() {
            return;
        }

        // Every remaining path can answer 200 with identity, 200 with a coding, or 406, so
        // the representation varies on Accept-Encoding however this request resolves: without
        // Vary a shared cache can hand a gzip body (or a 406) to a client it was never meant
        // for (RFC 9111 §4.1).
        add_vary(response);

        // The bodies this middleware does not transform, each for its own reason. The
        // transformation is what makes a coding available, so a client that refused
        // identity leaves nothing this path can serve.
        let untransformable = response.is_streaming()
            // A 206 carries a slice whose byte offsets are named in `Content-Range`. gzip is
            // not byte-preserving, so compressing it makes the declared range false: the
            // client's resumed offsets then point at the wrong place in a body it cannot
            // align. Ranges are served as stored (RFC 9110 §14.3, tower-http does the same).
            || response.status == 206
            || response
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-range"))
            // RFC 9111 §5.6.5: a representation carrying `Cache-Control: no-transform` must
            // not have its body altered, and compression is a transform. Handlers use the
            // directive deliberately (byte-exact payloads, streaming ranges, signatures).
            || has_no_transform(response);
        if untransformable {
            if !identity_ok {
                refuse_not_acceptable(response);
            }
            return;
        }

        // Below the floor, and for media types that are already entropy-coded, another pass
        // buys nothing — unless identity is refused, in which case the coding the client
        // named is the only body left to send.
        if identity_ok
            && (response.body.len() < self.config.min_compression_size
                || is_precompressed_content_type(response))
        {
            return;
        }

        let Some(encoding) =
            select_encoding(accept, self.config.enable_gzip, self.config.enable_brotli)
        else {
            if identity_ok {
                return;
            }
            // No coding this server speaks is acceptable and identity was refused: there is
            // no representation left to send.
            refuse_not_acceptable(response);
            return;
        };

        let original_len = response.body.len();
        let compressed = match encoding {
            "br" => self.compress_brotli(&response.body),
            _ => self.compress_gzip(&response.body),
        };

        match compressed {
            // Incompressible payloads can come out larger than they went in; keep the original
            // rather than paying bytes for nothing — but only when the client accepts it.
            Ok(body) if body.len() < original_len || !identity_ok => {
                response.body = body;
                response.add_header("Content-Encoding", encoding);
                self.update_content_length(response);
                // The body is now a different representation: anything that asserted what the
                // old bytes were, or promised them as a range, no longer describes this one.
                strip_stale_representation_metadata(response);
            }
            Ok(_) => {}
            Err(_) if !identity_ok => refuse_not_acceptable(response),
            Err(_) => {}
        }
    }

    fn compress_gzip(&self, data: &[u8]) -> Result<Bytes, std::io::Error> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut encoder =
            GzEncoder::new(Vec::new(), Compression::new(self.config.compression_level));
        encoder.write_all(data)?;
        Ok(Bytes::from(encoder.finish()?))
    }

    fn compress_brotli(&self, data: &[u8]) -> Result<Bytes, std::io::Error> {
        use brotli::enc::writer::CompressorWriter;
        use std::io::Write;

        let mut writer = CompressorWriter::new(
            Vec::new(),
            4096, // buffer size
            self.config.compression_level,
            22, // lg_window_size
        );
        writer.write_all(data)?;
        Ok(Bytes::from(writer.into_inner()))
    }

    fn update_content_length(&self, response: &mut Response) {
        // Remove old Content-Length and add new one
        response
            .headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
        response.add_header("Content-Length", response.body.len().to_string());
    }
}

/// Replaces a response with `406 Not Acceptable` and drops the fields that described the body
/// it no longer has.
///
/// `Vary` survives: it is what tells a cache that the refusal itself depends on the request
/// field. Framing fields, the representation metadata and the validators do not: they describe
/// a body this response no longer carries, and an `ETag` on a 406 would name the very
/// representation the client just refused.
fn refuse_not_acceptable(response: &mut Response) {
    response.status = 406;
    response.status_text = "Not Acceptable";
    response.body = Bytes::new();
    response.stream = None;
    // The refusal carries no representation at all, so it cannot keep the validators or the
    // range promise that described the one it withheld.
    strip_stale_representation_metadata(response);
    response.headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("content-length")
            && !k.eq_ignore_ascii_case("content-encoding")
            && !k.eq_ignore_ascii_case("content-range")
            && !k.eq_ignore_ascii_case("content-type")
            && !k.eq_ignore_ascii_case("content-disposition")
            && !k.eq_ignore_ascii_case("content-language")
            && !k.eq_ignore_ascii_case("last-modified")
            && !k.eq_ignore_ascii_case("transfer-encoding")
    });
}

/// Drops the fields that described the representation before its body was replaced.
///
/// A strong `ETag` and the integrity fields — `Content-MD5`, the legacy `Digest` of RFC 3230,
/// and `Content-Digest`/`Repr-Digest` of RFC 9530 — are assertions about these exact bytes, and
/// `Accept-Ranges: bytes` promises these bytes can be range-served. None of that is true of a
/// coded body, so keeping it hands a client a validator it will present on a later conditional
/// or `Range` request, which the handler then answers from the *stored* representation —
/// identity bytes spliced into a compressed cache entry. The middleware cannot mint a
/// replacement validator either: the handler is the only party that can compare one, and it
/// never sees the coded bytes. Nginx clears the same field when it gzips. `Last-Modified` stays:
/// it dates the resource, not the coding.
fn strip_stale_representation_metadata(response: &mut Response) {
    response.headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("etag")
            && !k.eq_ignore_ascii_case("content-md5")
            && !k.eq_ignore_ascii_case("digest")
            && !k.eq_ignore_ascii_case("content-digest")
            && !k.eq_ignore_ascii_case("repr-digest")
            && !k.eq_ignore_ascii_case("accept-ranges")
    });
}

/// Whether any `Cache-Control` field carries the `no-transform` directive (RFC 9111 §5.6.5).
///
/// Directives are case-insensitive and may be comma-separated within one field, and a response
/// may carry the field more than once.
fn has_no_transform(response: &Response) -> bool {
    response
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
        .flat_map(|(_, v)| v.split(','))
        .any(|directive| {
            directive
                .split_once('=')
                .map_or(directive, |(name, _)| name)
                .trim()
                .eq_ignore_ascii_case("no-transform")
        })
}

/// Media types whose payload is already entropy-coded. Recompressing them burns CPU and
/// usually produces a *larger* body.
const PRECOMPRESSED_MIME: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "font/woff",
    "font/woff2",
    "application/vnd.ms-fontobject",
    "application/zip",
    "application/gzip",
    "application/x-gzip",
    "application/x-bzip2",
    "application/x-7z-compressed",
    "application/x-rar-compressed",
    "application/x-xz",
];

/// Media-type prefixes treated as already compressed.
const PRECOMPRESSED_PREFIX: &[&str] = &["video/", "audio/"];

fn is_precompressed_content_type(response: &Response) -> bool {
    let Some((_, ct)) = response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    else {
        return false;
    };
    let mime = ct.split(';').next().unwrap_or("").trim();
    PRECOMPRESSED_MIME
        .iter()
        .any(|p| mime.eq_ignore_ascii_case(p))
        || PRECOMPRESSED_PREFIX.iter().any(|p| {
            // `get(..n)` yields None when `n` lands mid-char: a Content-Type carrying a
            // non-ASCII byte (app-controlled, but the panic escapes handler isolation
            // because this runs in the connection task) must not kill the socket.
            mime.get(..p.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(p))
        })
}

/// Append `Accept-Encoding` to a response's `Vary` field, without duplicating it.
///
/// A response whose body depends on the request's `Accept-Encoding` must say so, or a shared
/// cache can hand a gzip body to a client that never asked for one (RFC 9111 §4.1).
pub fn add_vary(response: &mut Response) {
    if let Some((_, existing)) = response
        .headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
    {
        let already = existing
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("accept-encoding"));
        if !already {
            existing.push_str(", Accept-Encoding");
        }
    } else {
        response.add_header("Vary", "Accept-Encoding");
    }
}

/// Chooses a response encoding from an `Accept-Encoding` header, honouring q-values.
///
/// A substring test is not sufficient here: `Accept-Encoding: gzbrip` contains `br`, and
/// `gzip;q=0` explicitly forbids gzip even though the token `gzip` is present. Per RFC 9110
/// §12.5.1 a q-value of 0 means "not acceptable", and an unlisted coding is only acceptable
/// when the wildcard `*` is present. Ties prefer Brotli, which has the better ratio.
pub fn select_encoding(
    accept: &str,
    gzip_enabled: bool,
    brotli_enabled: bool,
) -> Option<&'static str> {
    let preferences = AcceptEncoding::parse(accept);
    let gzip_score = if gzip_enabled {
        preferences.q(preferences.gzip)
    } else {
        0.0
    };
    let br_score = if brotli_enabled {
        preferences.q(preferences.br)
    } else {
        0.0
    };

    if br_score > 0.0 && br_score >= gzip_score {
        Some("br")
    } else if gzip_score > 0.0 {
        Some("gzip")
    } else {
        None
    }
}

/// Whether the identity coding (the representation as stored) is acceptable.
///
/// RFC 9110 §12.5.1: identity is acceptable unless the field excludes it — `identity;q=0`, or a
/// wildcard `*;q=0` with no more specific entry for `identity`. A field that names only `gzip`
/// still permits identity, which is what makes an unconfigured server correct by default.
pub fn accepts_identity(accept: &str) -> bool {
    let preferences = AcceptEncoding::parse(accept);
    match preferences.identity {
        Some(q) => q > 0.0,
        None => preferences.wildcard != Some(0.0),
    }
}

/// The q-values an `Accept-Encoding` field assigns, one slot per coding this server speaks.
struct AcceptEncoding {
    gzip: Option<f32>,
    br: Option<f32>,
    identity: Option<f32>,
    wildcard: Option<f32>,
}

impl AcceptEncoding {
    fn parse(accept: &str) -> Self {
        let mut parsed = Self {
            gzip: None,
            br: None,
            identity: None,
            wildcard: None,
        };
        for item in accept.split(',') {
            let mut parts = item.split(';');
            let Some(raw) = parts.next() else { continue };
            let coding = raw.trim();
            if coding.is_empty() {
                continue;
            }

            let mut q: f32 = 1.0;
            for param in parts {
                // Parameter names are case-insensitive (RFC 9110 §5.6.6), so `;Q=0` weighs
                // the same as `;q=0`.
                let Some((name, value)) = param.trim().split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("q") {
                    // An unparsable q-value is treated as "not acceptable" rather than trusted.
                    q = value.trim().parse::<f32>().unwrap_or(0.0).clamp(0.0, 1.0);
                }
            }

            // A repeated coding names the same coding twice; the client's strongest statement wins.
            let slot = if coding.eq_ignore_ascii_case("*") {
                &mut parsed.wildcard
            } else if coding.eq_ignore_ascii_case("gzip") {
                &mut parsed.gzip
            } else if coding.eq_ignore_ascii_case("br") {
                &mut parsed.br
            } else if coding.eq_ignore_ascii_case("identity") {
                &mut parsed.identity
            } else {
                continue;
            };
            *slot = Some(slot.map_or(q, |old| old.max(q)));
        }
        parsed
    }

    /// An explicit q (including q=0) overrides the wildcard.
    fn q(&self, explicit: Option<f32>) -> f32 {
        explicit.unwrap_or_else(|| self.wildcard.unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert!(config.enable_gzip);
        assert!(!config.enable_brotli);
        assert_eq!(config.min_compression_size, 1024);
        assert_eq!(config.compression_level, 6);
    }

    #[test]
    fn test_skip_small_responses() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text("small");
        assert_eq!(response.body.len(), 5);
        middleware.apply(&mut response, Some("gzip"));
        // Should not be compressed (too small)
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding"))
        );
    }

    #[test]
    fn test_gzip_compression() {
        let middleware = CompressionMiddleware::default();
        let large_body = "a".repeat(2000);
        let mut response = Response::text_dynamic(large_body.clone());
        let original_len = response.body.len();

        middleware.apply(&mut response, Some("gzip"));

        // Should be compressed
        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "gzip")
        );
        // Compressed data should be smaller
        assert!(response.body.len() < original_len);
    }

    /// The already-compressed prefix check sliced the media type by **byte** index. A
    /// Content-Type carrying a multi-byte character whose boundary did not fall on that
    /// index (`text/é` slices at byte 6, inside `é`) panicked — and this runs in the
    /// connection task, outside handler panic isolation, so the socket died with no
    /// response at all instead of being served.
    #[test]
    fn test_non_ascii_content_type_does_not_panic_the_prefix_check() {
        let middleware = CompressionMiddleware::default();
        for ct in [
            "text/é",
            "video/é",
            "audio/x‑m4a",
            "é/video/mp4",
            "application/json;charset=útf-8",
        ] {
            let mut response = Response::text_dynamic("a".repeat(2000));
            response
                .headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            response.add_header("Content-Type", ct);
            middleware.apply(&mut response, Some("gzip"));
            let already_compressed = ct.starts_with("video/") || ct.starts_with("audio/");
            let encoded = response
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding"));
            if already_compressed {
                assert!(
                    !encoded,
                    "{ct} is already-compressed media and must be left alone"
                );
            } else {
                assert!(encoded, "{ct} is ordinary text and must still compress");
            }
        }

        // The real prefix rule still works for well-formed types.
        let mut response = Response::text_dynamic("a".repeat(2000));
        response
            .headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
        response.add_header("Content-Type", "video/mp4");
        middleware.apply(&mut response, Some("gzip"));
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")),
            "video/* must never be recompressed"
        );
    }

    #[test]
    fn test_no_compression_without_accept() {
        let middleware = CompressionMiddleware::default();
        let large_body = "a".repeat(2000);
        let mut response = Response::text_dynamic(large_body.clone());

        middleware.apply(&mut response, None);

        // Should not be compressed
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding"))
        );
    }

    /// A cache must be told the body depends on Accept-Encoding, or it can serve a gzip body
    /// to a client that never advertised gzip support.
    #[test]
    fn test_vary_accept_encoding_is_added() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("gzip"));

        let vary = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .map(|(_, v)| v.as_str());
        assert_eq!(vary, Some("Accept-Encoding"));
    }

    #[test]
    fn test_vary_is_added_even_when_client_declines_compression() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("identity"));

        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("vary") && v.contains("Accept-Encoding"))
        );
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding"))
        );
    }

    #[test]
    fn test_vary_is_not_duplicated() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        response.add_header("Vary", "Accept-Language");

        middleware.apply(&mut response, Some("gzip"));
        middleware.apply(&mut response, Some("gzip"));

        let count = response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .count();
        assert_eq!(count, 1);
        let v = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .unwrap()
            .1
            .clone();
        assert!(v.contains("Accept-Language"), "existing Vary lost: {}", v);
        assert_eq!(
            v.to_ascii_lowercase().matches("accept-encoding").count(),
            1,
            "Accept-Encoding duplicated: {}",
            v
        );
    }

    /// A 206 body is a byte slice named by `Content-Range`; gzip is not byte-preserving,
    /// so compressing one makes the declared offsets lie for any client that resumes.
    #[test]
    fn test_partial_content_is_never_compressed() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::status(206, "Partial Content");
        response.body = Bytes::from("a".repeat(4000));
        response.add_header("Content-Range", "bytes 0-3999/9999");
        response.add_header("Content-Type", "text/plain");
        middleware.apply(&mut response, Some("gzip"));
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")),
            "a range body was rewritten"
        );
        assert_eq!(response.body.len(), 4000, "range body bytes changed");
    }

    /// The guard keys on the header, not only the status, so an app that tags a 200 with
    /// `Content-Range` (a partial payload delivered under a status it chose itself) is
    /// also left alone.
    #[test]
    fn test_content_range_on_a_200_is_left_alone() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::status(200, "OK");
        response.body = Bytes::from("a".repeat(4000));
        response.add_header("Content-Range", "bytes 0-3999/4000");
        response.add_header("Content-Type", "text/plain");
        middleware.apply(&mut response, Some("gzip"));
        assert_eq!(response.body.len(), 4000);
    }

    #[test]
    fn test_no_vary_on_bodyless_status_codes() {
        let middleware = CompressionMiddleware::default();
        for status in [204u16, 304] {
            let mut response = Response::status(status, "x");
            response.body = Bytes::from("a".repeat(2000));
            middleware.apply(&mut response, Some("gzip"));
            assert!(
                response
                    .headers
                    .iter()
                    .all(|(k, _)| !k.eq_ignore_ascii_case("vary")),
                "{} must not gain Vary",
                status
            );
        }
    }

    /// Regression: `accept.contains("br")` matched any header containing those two bytes in
    /// sequence, and ignored q-values entirely.
    #[test]
    fn test_select_encoding_honours_q_values() {
        // q=0 means "not acceptable" (RFC 9110 §12.5.1).
        assert_eq!(select_encoding("gzip;q=0", true, false), None);
        assert_eq!(select_encoding("gzip;q=0.0", true, false), None);
        assert_eq!(
            select_encoding("br;q=0, gzip;q=0.5", true, true),
            Some("gzip")
        );

        // An explicit q=0 beats a wildcard.
        assert_eq!(select_encoding("*;q=1, gzip;q=0", true, false), None);

        // Highest q wins, regardless of header order.
        assert_eq!(
            select_encoding("gzip;q=0.5, br;q=0.9", true, true),
            Some("br")
        );
        assert_eq!(
            select_encoding("br;q=0.3, gzip;q=0.8", true, true),
            Some("gzip")
        );

        // Ties prefer Brotli (better ratio).
        assert_eq!(select_encoding("gzip, br", true, true), Some("br"));
        assert_eq!(select_encoding("br, gzip", true, true), Some("br"));
    }

    #[test]
    fn test_select_encoding_rejects_substring_false_positives() {
        // "gzbrip" contains the bytes "br" — the old substring test served Brotli here.
        assert_eq!(select_encoding("gzbrip", true, true), None);
        assert_eq!(select_encoding("x-gzip", true, false), None);
        assert_eq!(select_encoding("compress", true, true), None);
        assert_eq!(select_encoding("", true, true), None);
        assert_eq!(select_encoding("identity", true, true), None);
    }

    #[test]
    fn test_select_encoding_wildcard_and_case() {
        assert_eq!(select_encoding("*", true, false), Some("gzip"));
        assert_eq!(select_encoding("*", false, true), Some("br"));
        assert_eq!(select_encoding("*", false, false), None);
        assert_eq!(select_encoding("GZIP", true, false), Some("gzip"));
        assert_eq!(select_encoding("  gzip  ", true, false), Some("gzip"));
        assert_eq!(select_encoding("gzip;q=0.000", true, false), None);
        // Whitespace and empty entries must not confuse the parser.
        assert_eq!(select_encoding(", ,gzip,, ", true, false), Some("gzip"));
    }

    #[test]
    fn test_select_encoding_respects_disabled_codecs() {
        assert_eq!(select_encoding("gzip, br", false, false), None);
        assert_eq!(select_encoding("gzip, br", false, true), Some("br"));
        assert_eq!(select_encoding("gzip, br", true, false), Some("gzip"));
    }

    #[test]
    fn test_select_encoding_malformed_q_does_not_panic() {
        for accept in [
            "gzip;q=",
            "gzip;q",
            "gzip;q=abc",
            "gzip;q=-1",
            "gzip;q=1e999",
            "gzip;q=NaN",
            "gzip;",
            ";",
            "gzip;q=0.5;level=1",
        ] {
            let _ = select_encoding(accept, true, true);
        }
        // A malformed q must not be treated as full-quality acceptance.
        assert_eq!(select_encoding("gzip;q=abc", true, false), None);
        assert_eq!(select_encoding("gzip;q=-1", true, false), None);
    }

    /// `identity` is acceptable unless it is explicitly refused: an absent header, a header
    /// that names only other codings, and `*` all leave the plain representation usable.
    /// Only `identity;q=0` and a bare `*;q=0` (which covers identity) make it unacceptable —
    /// RFC 9110 §12.5.1, and the case where answering 406 is the honest answer.
    #[test]
    fn test_accepts_identity_only_refuses_explicit_zero() {
        assert!(accepts_identity(""));
        assert!(accepts_identity("gzip"));
        assert!(accepts_identity("gzip;q=0"));
        assert!(accepts_identity("*"));
        assert!(accepts_identity("br, gzip;q=0.5"));
        assert!(!accepts_identity("identity;q=0"));
        assert!(!accepts_identity("identity;q=0.0"));
        assert!(!accepts_identity("*;q=0"));
        assert!(!accepts_identity("gzip;q=1, *;q=0"));
        // An explicit identity q overrides the wildcard in both directions.
        assert!(accepts_identity("*;q=0, identity;q=1"));
        assert!(!accepts_identity("identity;q=0, *;q=1"));
        // Case-insensitive, like every other coding token.
        assert!(!accepts_identity("IDENTITY;Q=0"));
    }

    /// Already-compressed media must not be recompressed.
    #[test]
    fn test_precompressed_content_types_are_skipped() {
        let middleware = CompressionMiddleware::default();
        for mime in [
            "image/jpeg",
            "image/png",
            "image/webp",
            "video/mp4",
            "audio/mpeg",
            "font/woff2",
            "application/zip",
            "IMAGE/JPEG",
            "image/jpeg; charset=binary",
        ] {
            let mut response =
                Response::bytes(Bytes::from("a".repeat(2000)), "application/octet-stream");
            response
                .headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            response.add_header("Content-Type", mime);

            middleware.apply(&mut response, Some("gzip"));
            assert!(
                response
                    .headers
                    .iter()
                    .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")),
                "{} should not be recompressed",
                mime
            );
        }

        // Text-like types still compress.
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("gzip"));
        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "gzip")
        );
    }

    /// An incompressible body must not be replaced by a larger "compressed" one.
    ///
    /// The fixture is a full-state LCG: the previous one (`i * const >> 13`) re-derived each
    /// byte from its index alone and deflated to a quarter of its size, so this test only ever
    /// exercised the shrinking path and its "left alone" assertion never ran.
    #[test]
    fn test_incompressible_body_is_left_alone() {
        let middleware = CompressionMiddleware::default();

        let mut state = 0x1234_5678u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let mut response = Response::bytes(Bytes::from(noise.clone()), "application/octet-stream");

        middleware.apply(&mut response, Some("gzip"));
        assert_eq!(
            response.body.len(),
            noise.len(),
            "the fixture must be incompressible for this test to mean anything"
        );
        assert!(
            response
                .headers
                .iter()
                .all(|(k, _)| !k.eq_ignore_ascii_case("content-encoding")),
            "an incompressible body must not be replaced by a larger coded one"
        );
    }

    fn vary_contains(response: &Response, token: &str) -> bool {
        response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("vary"))
            .flat_map(|(_, v)| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    }

    fn has_header(response: &Response, name: &str) -> bool {
        response
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    fn encoded_with(response: &Response) -> Option<&str> {
        response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
            .map(|(_, v)| v.as_str())
    }

    fn gunzip(body: &[u8]) -> Vec<u8> {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut decoded = Vec::new();
        GzDecoder::new(body).read_to_end(&mut decoded).unwrap();
        decoded
    }

    /// RFC 9110 §12.5.1: `identity;q=0` refuses the representation as stored, and with no
    /// other coding offered there is nothing acceptable to send. The middleware used to answer
    /// 200 with the identity body the client had just excluded — `accepts_identity`'s own doc
    /// named the 406 as the honest answer, but nothing called it on this path.
    #[test]
    fn test_identity_refused_without_a_coding_is_not_acceptable() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("identity;q=0"));

        assert_eq!(
            response.status, 406,
            "an excluded identity body was still served"
        );
        assert!(response.body.is_empty(), "a 406 must not carry the body");
        assert!(
            vary_contains(&response, "Accept-Encoding"),
            "the refusal depends on Accept-Encoding and must say so"
        );
    }

    /// `*;q=0` with no more specific entry covers identity exactly as `identity;q=0` does.
    #[test]
    fn test_wildcard_zero_also_refuses_identity() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        middleware.apply(&mut response, Some("*;q=0"));

        assert_eq!(response.status, 406);
        assert!(response.body.is_empty());
    }

    /// When the client accepts gzip but refuses identity, a payload the middleware would
    /// normally leave alone (incompressible, or below the size floor) must still be sent in
    /// the one coding the client accepts — larger or not, identity is not an option.
    #[test]
    fn test_identity_refused_still_sends_the_selected_coding_when_larger() {
        let middleware = CompressionMiddleware::default();
        // gzip carries a 10-byte header and an 8-byte trailer, so a 4-byte payload cannot
        // compress below its own size no matter what the deflate stream does.
        let noise = b"\x00\x00\x00\x00";
        let mut response = Response::bytes(Bytes::from_static(noise), "application/octet-stream");

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 200);
        assert_eq!(encoded_with(&response), Some("gzip"));
        assert!(
            response.body.len() > noise.len(),
            "this arm must exercise the larger-than-original branch"
        );
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut decoded = Vec::new();
        GzDecoder::new(&response.body[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, &noise[..]);
        let cl: Vec<&String> = response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v)
            .collect();
        assert_eq!(cl.len(), 1);
        assert_eq!(cl[0].parse::<usize>().unwrap(), response.body.len());
    }

    #[test]
    fn test_identity_refused_below_the_size_floor_is_still_coded() {
        let middleware = CompressionMiddleware::default();
        let original = "a".repeat(200);
        let mut response = Response::text_dynamic(original.clone());
        assert!(response.body.len() < middleware.config.min_compression_size);

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 200);
        assert_eq!(encoded_with(&response), Some("gzip"));
        assert_eq!(gunzip(&response.body), original.as_bytes());
    }

    /// A body the middleware refuses to transform (`no-transform`) cannot satisfy a client
    /// that excluded identity, so the honest answer is 406, not an identity body.
    #[test]
    fn test_identity_refused_on_an_untransformable_body_is_not_acceptable() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        response.add_header("Cache-Control", "no-transform");

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 406);
        assert!(response.body.is_empty());
    }

    /// A coding the handler already set is the handler's answer; this middleware must not
    /// second-guess it, overwrite it, or refuse the response on its behalf.
    #[test]
    fn test_handler_chosen_coding_is_left_alone() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        response.add_header("Content-Encoding", "gzip");

        middleware.apply(&mut response, Some("identity;q=0"));

        assert_eq!(response.status, 200);
        assert_eq!(encoded_with(&response), Some("gzip"));
        assert_eq!(
            response.body.len(),
            2000,
            "the handler's body was rewritten"
        );
        assert!(!vary_contains(&response, "Accept-Encoding"));
    }

    /// The precompressed-media check is an efficiency heuristic, not a constraint: when the
    /// client refuses identity, the coding it named is the only body left to send.
    #[test]
    fn test_precompressed_media_type_is_coded_when_identity_is_refused() {
        let middleware = CompressionMiddleware::default();
        let payload = b"not-really-a-png";
        let mut response = Response::bytes(Bytes::from_static(payload), "image/png");

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 200);
        assert_eq!(encoded_with(&response), Some("gzip"));
        assert_eq!(gunzip(&response.body), payload);
    }

    /// A bodyless status has no representation to negotiate; a client excluding identity
    /// must still get its 204/304, not a 406.
    #[test]
    fn test_bodyless_status_is_never_refused_for_identity() {
        let middleware = CompressionMiddleware::default();
        for status in [204u16, 304] {
            let mut response = Response::status(status, "x");
            middleware.apply(&mut response, Some("identity;q=0"));
            assert_eq!(response.status, status);
        }
    }

    /// A redirect carries no body, so no coding is at stake. Turning its 302 into a 406
    /// would make the redirect unfollowable for that client.
    #[test]
    fn test_empty_body_keeps_its_status_when_identity_is_refused() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::redirect("/login", 302);

        middleware.apply(&mut response, Some("identity;q=0"));

        assert_eq!(response.status, 302);
        assert_eq!(
            response
                .headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("location"))
                .count(),
            1
        );
    }

    /// A 206 is a byte range of a representation this middleware will not transform; with
    /// identity excluded there is no other form of it to send.
    #[test]
    fn test_identity_refused_on_a_range_is_not_acceptable() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::status(206, "Partial Content");
        response.body = Bytes::from("a".repeat(2000));
        response.add_header("Content-Range", "bytes 0-1999/9999");

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 406);
        assert!(response.body.is_empty());
        assert!(
            !response
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-range")),
            "the withheld range must not be advertised on the 406"
        );
        assert!(vary_contains(&response, "Accept-Encoding"));
    }

    /// A streamed body's bytes are not in memory, so it cannot be re-coded either.
    #[test]
    fn test_identity_refused_on_a_streamed_body_is_not_acceptable() {
        use aether_http::response::BodyStream;
        let middleware = CompressionMiddleware::default();
        let source = BodyStream::from_chunks([Bytes::from_static(b"streamed")]);
        let mut response = Response::stream(source, "text/plain");

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 406);
        assert!(
            !response.is_streaming(),
            "the refusal cannot carry the stream"
        );
        assert!(response.body.is_empty());
    }

    /// The 406 describes no representation, so the validators and metadata of the withheld
    /// body must not survive on it.
    #[test]
    fn test_the_refusal_strips_the_withheld_representation_metadata() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(2000));
        response.add_header("Cache-Control", "no-transform");
        response.add_header("ETag", "\"v1\"");
        response.add_header("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT");
        response.add_header("Content-Disposition", "attachment; filename=\"a.bin\"");
        response.add_header("Content-Language", "en");
        response.add_header("Accept-Ranges", "bytes");
        response.add_header("Content-MD5", "sQqNsWTgdUEFt6mb5y4/5Q==");
        response.add_header(
            "Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );
        response.add_header(
            "Content-Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );
        response.add_header(
            "Repr-Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );

        middleware.apply(&mut response, Some("gzip, identity;q=0"));

        assert_eq!(response.status, 406);
        for name in [
            "content-type",
            "content-length",
            "content-encoding",
            "content-range",
            "content-disposition",
            "content-language",
            "etag",
            "last-modified",
            "accept-ranges",
            "content-md5",
            "digest",
            "content-digest",
            "repr-digest",
        ] {
            assert!(
                !response
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case(name)),
                "{name} survived on a 406"
            );
        }
        assert!(vary_contains(&response, "Accept-Encoding"));
    }

    /// A re-coded body is a different representation, so the metadata that described the bytes
    /// it replaced cannot ride along. `ETag` and the integrity fields are assertions about
    /// *these* bytes; `Accept-Ranges` promises they can be range-served. A client that cached
    /// the coded body would otherwise revalidate — or ask for a range — with the identity
    /// validator, and the handler would answer it from the stored representation, splicing
    /// identity bytes into a compressed entry. Nginx clears the same field on gzip for the same
    /// reason. `Last-Modified` stays: it dates the resource, not the coding.
    #[test]
    fn a_transformed_body_loses_the_metadata_that_described_the_original_bytes() {
        let middleware = CompressionMiddleware::default();
        let mut response = Response::text_dynamic("a".repeat(4000));
        response.add_header("ETag", "\"v1\"");
        response.add_header("Accept-Ranges", "bytes");
        response.add_header("Content-MD5", "sQqNsWTgdUEFt6mb5y4/5Q==");
        response.add_header(
            "Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );
        response.add_header(
            "Content-Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );
        response.add_header(
            "Repr-Digest",
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:",
        );
        response.add_header("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT");

        middleware.apply(&mut response, Some("gzip"));

        assert!(
            has_header(&response, "content-encoding"),
            "the fixture must actually be transformed for this test to mean anything"
        );
        for name in [
            "etag",
            "accept-ranges",
            "content-md5",
            "digest",
            "content-digest",
            "repr-digest",
        ] {
            assert!(
                !has_header(&response, name),
                "{name} survived a body the response no longer carries"
            );
        }
        assert!(
            has_header(&response, "last-modified"),
            "Last-Modified dates the resource, not the coding"
        );
    }

    /// The strip is tied to the transform, not to the header: a body the middleware leaves
    /// alone — below the floor, already-compressed media, or incompressible — still carries a
    /// validator that describes it exactly.
    #[test]
    fn an_untransformed_body_keeps_its_validator() {
        let middleware = CompressionMiddleware::default();
        let mut small = Response::text("tiny");
        small.add_header("ETag", "\"v1\"");
        small.add_header("Accept-Ranges", "bytes");
        middleware.apply(&mut small, Some("gzip"));
        assert!(!has_header(&small, "content-encoding"));
        assert!(has_header(&small, "etag") && has_header(&small, "accept-ranges"));

        // A full-state LCG does not shrink, so the original body is the one sent.
        let mut state = 0x1234_5678u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let mut incompressible =
            Response::bytes(Bytes::from(noise.clone()), "application/octet-stream");
        incompressible.add_header("ETag", "\"v2\"");
        incompressible.add_header("Accept-Ranges", "bytes");
        middleware.apply(&mut incompressible, Some("gzip"));
        // The precondition is asserted, not assumed: a fixture that stopped being
        // incompressible would otherwise skip the check and pass silently.
        assert_eq!(
            incompressible.body.len(),
            noise.len(),
            "the fixture must stay incompressible for this test to mean anything"
        );
        assert_eq!(
            incompressible
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
                .map(|(_, v)| v.as_str()),
            Some("\"v2\""),
            "a body that was not rewritten lost the validator that describes it"
        );
    }

    /// Brotli must round-trip when enabled.
    #[test]
    fn test_brotli_round_trip() {
        let config = CompressionConfig {
            enable_brotli: true,
            ..CompressionConfig::default()
        };
        let middleware = CompressionMiddleware::new(config);
        let body = "hello aether ".repeat(400);
        let mut response = Response::text_dynamic(body.clone());

        middleware.apply(&mut response, Some("br"));

        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("content-encoding") && v == "br")
        );
        assert!(response.body.len() < body.len());

        let mut decoded = Vec::new();
        brotli::BrotliDecompress(&mut &response.body[..], &mut decoded).unwrap();
        assert_eq!(decoded, body.as_bytes());
    }

    /// Gzip must round-trip, and Content-Length must match the compressed body.
    #[test]
    fn test_gzip_round_trip_and_content_length() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let middleware = CompressionMiddleware::default();
        let body = "hello aether ".repeat(400);
        let mut response = Response::text_dynamic(body.clone());
        middleware.apply(&mut response, Some("gzip"));

        let mut decoded = String::new();
        GzDecoder::new(&response.body[..])
            .read_to_string(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body);

        let cl = response
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>();
        assert_eq!(cl.len(), 1, "Content-Length duplicated: {:?}", cl);
        assert_eq!(cl[0].parse::<usize>().unwrap(), response.body.len());
    }
}
