//! # Dynamic static-directory serving
//!
//! Maps a catch-all route to files under a developer-supplied root. Every request path is
//! resolved through [`safe_resolve_path`], so traversal, alternate data streams, and
//! Windows device names cannot escape the root. Files are read with `tokio::fs` to keep
//! blocking I/O off the reactor.
//!
//! Two behaviours are opt-in through [`StaticDirOptions`] and off by default: a directory
//! listing for a folder with no `index.html`, and selection of a pre-compressed `.br`/`.gz`
//! sibling when the client's `Accept-Encoding` allows it.

use aether_http::response::{BodySource, BodyStream, Chunk, Response, status_forbids_body};
use aether_http::static_files::{plan_file, safe_resolve_path};
use aether_middleware::compression::{accepts_identity, add_vary, select_encoding};
use bytes::Bytes;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// How much of a file one read hands the writer.
///
/// A streamed answer holds one chunk plus whatever the wire has not drained, so this is the
/// per-request memory a static file costs the budget instead of the file's size.
const STATIC_CHUNK: usize = 64 * 1024;

/// How a `static_dir` route answers.
///
/// The default is the conservative one this route has always had: no directory listing and only
/// the files as they sit on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticDirOptions {
    /// `max-age` for `Cache-Control` on a file answer.
    pub max_age_secs: u32,
    /// Answer a request for a directory with its entries, unless the directory has an
    /// `index.html`. Off by default: a listing publishes every file name under the root.
    pub directory_listing: bool,
    /// Serve `name.br` / `name.gz` when the client's `Accept-Encoding` prefers it and the sibling
    /// exists. Off by default, because the siblings are only harmless if the operator put them
    /// there deliberately.
    pub precompressed: bool,
}

impl StaticDirOptions {
    /// The defaults with an explicit cache lifetime.
    pub const fn with_max_age(max_age_secs: u32) -> Self {
        Self {
            max_age_secs,
            directory_listing: false,
            precompressed: false,
        }
    }
}

impl Default for StaticDirOptions {
    fn default() -> Self {
        Self::with_max_age(3600)
    }
}

/// The request fields a static answer depends on, borrowed from the request for its lifetime.
pub(crate) struct StaticRequest<'a> {
    pub relative: &'a str,
    /// The request target's path, undecoded. The captured `filepath` cannot answer "did this
    /// request end in a slash": a router that matches `/dir/` and `/dir` to the same route
    /// reports the same capture for both, and only the raw path keeps the distinction.
    pub path: &'a str,
    pub if_none_match: Option<&'a str>,
    pub if_range: Option<&'a str>,
    pub range: Option<&'a str>,
    pub accept_encoding: Option<&'a str>,
}

/// Serve one request from a static root.
///
/// The answer is decided from metadata and the payload is streamed in [`STATIC_CHUNK`] reads, so
/// a multi-gigabyte file costs the memory budget one chunk rather than its own size, and a `HEAD`
/// opens the file without reading a byte of it. The validator is therefore the weak metadata one
/// ([`aether_http::static_files::file_etag`]) with `Last-Modified` beside it: a content hash would
/// have to be computed before the head, which is the buffering this route exists to avoid.
pub(crate) async fn serve_from_dir(
    root: &Path,
    prefix: &str,
    request: &StaticRequest<'_>,
    options: StaticDirOptions,
) -> Response {
    let Ok(path) = safe_resolve_path(root, request.relative) else {
        return Response::status(404, "Not Found");
    };

    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return Response::status(404, "Not Found");
    };
    if metadata.is_dir() {
        // A directory that has a page is a page, listing on or off: the flag opts into
        // publishing the names a directory holds, not into serving the page it already holds.
        // Checking here rather than inside `serve_directory` is what makes the page reachable
        // without the listing.
        if let Some(index) = contained_child(root, &path.join("index.html")).await {
            return serve_file(root, request, &index, options).await;
        }
        return if options.directory_listing {
            serve_directory(prefix, request, &path, options).await
        } else {
            // A listing leaks filenames and is a common misconfiguration; it is opt-in.
            Response::status(404, "Not Found")
        };
    }
    if !metadata.is_file() {
        return Response::status(404, "Not Found");
    }

    serve_file(root, request, &path, options).await
}

/// The listing for a directory that holds no page of its own.
async fn serve_directory(
    prefix: &str,
    request: &StaticRequest<'_>,
    directory: &Path,
    options: StaticDirOptions,
) -> Response {
    let relative = request.relative.trim_end_matches('/');
    let at_root = relative.is_empty();
    // Relative links in a listing resolve against the final segment, so a directory reached
    // without its trailing slash answers a redirect instead of a page whose links all miss.
    if !request.path.ends_with('/') {
        return redirect_to_directory(prefix, relative);
    }

    let listing = match read_listing(directory, options).await {
        Ok(listing) => listing,
        Err(status) => return status,
    };
    let display = format!("{prefix}/{relative}");
    let mut response = Response::html_dynamic(render_listing(&display, &listing, at_root));
    // A listing is a directory read, not a representation of a file: it must be revalidated on
    // every use, and it has no entity-tag of its own to be conditional on.
    response.add_header("Cache-Control", "no-cache");
    response
}

async fn serve_file(
    root: &Path,
    request: &StaticRequest<'_>,
    path: &Path,
    options: StaticDirOptions,
) -> Response {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(request.relative);

    let accept = request.accept_encoding.unwrap_or("");
    let variant = if options.precompressed {
        select_variant(root, path, request.accept_encoding).await
    } else {
        None
    };

    let (serve_path, coding) = match variant {
        Some((variant_path, coding)) => (variant_path, Some(coding)),
        None => {
            // Refusing identity is a statement about this request, and only a route that reads
            // `Accept-Encoding` can honour it. `Vary` belongs on this refusal too: a cache that
            // stored it must not replay it to a client that never excluded identity.
            if options.precompressed && !accepts_identity(accept) {
                let mut refusal = Response::status(406, "Not Acceptable");
                add_vary(&mut refusal);
                return refusal;
            }
            (path.to_path_buf(), None)
        }
    };

    // The metadata read is the only time the file is touched before the head goes out: the plan
    // answers a conditional request, a `HEAD`, and a range without ever opening the payload.
    let Ok(metadata) = tokio::fs::metadata(&serve_path).await else {
        return Response::status(500, "Internal Server Error");
    };
    let plan = plan_file(
        // The base name, not the variant's: `app.js.br` is still JavaScript.
        filename,
        metadata.len(),
        metadata.modified().ok(),
        request.if_none_match,
        request.if_range,
        request.range,
        options.max_age_secs,
    );

    let mut response = Response::status(plan.status, plan.status_text);
    response.headers = plan.headers;
    if let Some((offset, len)) = plan.body {
        match FileBody::open(&serve_path, offset, len).await {
            Ok(body) => response.stream = Some(BodyStream::new(body)),
            Err(_) => return Response::status(500, "Internal Server Error"),
        }
    }
    if let Some(coding) = coding
        && !status_forbids_body(response.status)
    {
        response.add_header("Content-Encoding", coding);
    }

    if options.precompressed {
        // On every answer, the plain file included: a cache that stored the uncompressed body
        // must not hand it to a client that asked for Brotli, and the other way round.
        add_vary(&mut response);
    }
    response
}

/// A whole regular file as a stream, plus the length to declare — the shape [`App::static_file`]
/// answers with, so a registered file costs the memory budget one [`STATIC_CHUNK`] like a
/// directory's does.
pub(crate) async fn open_file(path: &Path) -> std::io::Result<(FileBody, u64)> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let len = metadata.len();
    Ok((FileBody::open(path, 0, len).await?, len))
}

/// The payload of a static file, read from disk as the writer asks for it.
///
/// The answer was decided from metadata, so a `HEAD`, a `304` or a `416` never opens the payload
/// and the file is touched only after its head is already representable. `remaining` is the
/// plan's length, which is what makes a file that shrank mid-stream an error the writer can act
/// on: ending quietly would send fewer bytes than the `Content-Length` that already went out.
pub(crate) struct FileBody {
    file: tokio::fs::File,
    remaining: u64,
}

impl FileBody {
    async fn open(path: &Path, offset: u64, len: u64) -> std::io::Result<Self> {
        let mut file = tokio::fs::File::open(path).await?;
        if offset != 0 {
            file.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        Ok(Self {
            file,
            remaining: len,
        })
    }
}

impl BodySource for FileBody {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Chunk>> + Send + '_>> {
        Box::pin(async move {
            if self.remaining == 0 {
                return None;
            }
            let want = self.remaining.min(STATIC_CHUNK as u64) as usize;
            let mut buf = vec![0u8; want];
            match self.file.read(&mut buf).await {
                // The plan is unreachable on this file, so the failure is reported once and the
                // source is spent: a writer that keeps polling must see the end, not the error
                // again, or a drain loop never terminates.
                Ok(0) => {
                    self.remaining = 0;
                    Some(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "static file shrank while it was being streamed",
                    )))
                }
                Ok(n) => {
                    buf.truncate(n);
                    self.remaining -= n as u64;
                    Some(Ok(Bytes::from(buf)))
                }
                Err(err) => {
                    self.remaining = 0;
                    Some(Err(err))
                }
            }
        })
    }
}

/// The pre-compressed sibling to serve, if the client accepts one and it is inside the root.
async fn select_variant(
    root: &Path,
    path: &Path,
    accept_encoding: Option<&str>,
) -> Option<(PathBuf, &'static str)> {
    let brotli = contained_sibling(root, path, ".br").await;
    let gzip = contained_sibling(root, path, ".gz").await;
    let coding = select_encoding(
        accept_encoding.unwrap_or(""),
        gzip.is_some(),
        brotli.is_some(),
    )?;
    match coding {
        "br" => brotli.map(|path| (path, "br")),
        _ => gzip.map(|path| (path, "gzip")),
    }
}

/// `path` with `suffix` appended, canonicalised, and required to still be inside `root`.
///
/// The suffix is appended to the *file name*, so `a.js` looks for `a.js.br`. The containment
/// re-check is not redundant: the base file was resolved and canonicalised, but its sibling is a
/// different inode, and a symlink named `a.js.br` can point anywhere on the machine.
async fn contained_sibling(root: &Path, path: &Path, suffix: &str) -> Option<PathBuf> {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    contained_child(root, Path::new(&name)).await
}

/// A child path, canonicalised and required to be inside the canonical root.
async fn contained_child(root: &Path, child: &Path) -> Option<PathBuf> {
    if !tokio::fs::metadata(child).await.ok()?.is_file() {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let canonical = child.canonicalize().ok()?;
    canonical.starts_with(&canonical_root).then_some(canonical)
}

/// One entry of a listing, already filtered and classified.
struct ListingEntry {
    name: String,
    is_dir: bool,
}

/// Entries that may appear in a listing, in the order a reader expects to scan them.
async fn read_listing(
    directory: &Path,
    options: StaticDirOptions,
) -> Result<Vec<ListingEntry>, Response> {
    let mut reader = tokio::fs::read_dir(directory)
        .await
        .map_err(|_| Response::status(500, "Internal Server Error"))?;
    let mut entries = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|_| Response::status(500, "Internal Server Error"))?
    {
        if entries.len() >= MAX_LISTING_ENTRIES {
            break;
        }
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        // A symlink can point out of the root, where the resolver will refuse it: listing it
        // would advertise a link that cannot work.
        if file_type.is_symlink() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            // A name that is not UTF-8 has no faithful href; skip rather than invent one.
            continue;
        };
        if !listable_name(&name) || (options.precompressed && is_variant_name(&name)) {
            continue;
        }
        entries.push(ListingEntry {
            name,
            is_dir: file_type.is_dir(),
        });
    }
    entries.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_ascii_lowercase()));
    Ok(entries)
}

/// How many entries a listing will render. A directory with more than this is not a page; the
/// response says it was truncated rather than buffering an unbounded amount of HTML.
const MAX_LISTING_ENTRIES: usize = 10_000;

/// Whether a name may be shown. The Windows rules apply on every platform so a listing does not
/// differ between them: the resolver refuses these names everywhere, and a link the server will
/// answer `404` is worse than no link.
fn listable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.ends_with('.')
        && !name.ends_with(' ')
}

/// A pre-compressed sibling is an encoding of another entry, not an entry of its own.
fn is_variant_name(name: &str) -> bool {
    name.ends_with(".br") || name.ends_with(".gz")
}

/// The listing page.
fn render_listing(display: &str, entries: &[ListingEntry], at_root: bool) -> String {
    let title = html_escape(display);
    let mut html = String::with_capacity(256 + entries.len() * 64);
    html.push_str("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">");
    html.push_str(&format!("<title>Index of {title}</title></head>\n"));
    html.push_str(&format!("<body><h1>Index of {title}</h1>\n<ul>\n"));
    if !at_root {
        html.push_str("<li><a href=\"../\">../</a></li>\n");
    }
    for entry in entries {
        let href = encode_segment(&entry.name);
        let label = html_escape(&entry.name);
        if entry.is_dir {
            html.push_str(&format!("<li><a href=\"{href}/\">{label}/</a></li>\n"));
        } else {
            html.push_str(&format!("<li><a href=\"{href}\">{label}</a></li>\n"));
        }
    }
    if entries.len() >= MAX_LISTING_ENTRIES {
        html.push_str(&format!(
            "<li>listing truncated at {MAX_LISTING_ENTRIES} entries</li>\n"
        ));
    }
    html.push_str("</ul></body></html>\n");
    html
}

/// The trailing-slash redirect for a directory.
///
/// `Location` is rebuilt from the route prefix and the request's own segments, so the only bytes
/// that reach the header from the request are percent-encoded segments — never the raw URI, and
/// never a CR or LF that would end the field early.
fn redirect_to_directory(prefix: &str, relative: &str) -> Response {
    let encoded: Vec<String> = relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(encode_segment)
        .collect();
    let mut location = String::with_capacity(prefix.len() + relative.len() + 2);
    location.push_str(prefix);
    location.push('/');
    location.push_str(&encoded.join("/"));
    if !encoded.is_empty() {
        location.push('/');
    }
    let mut response = Response::status(301, "Moved Permanently");
    response.add_header("Location", location);
    response
}

/// Percent-encode one path segment, leaving RFC 3986 §2.3 unreserved characters alone.
///
/// Every other byte is escaped, which is what keeps a file name containing `?`, `#` or a space
/// from becoming a query string or two different links.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Escape the five characters that can end an element or an attribute in HTML.
///
/// A file name is attacker-influenced content whenever an upload lands in the root, so it is
/// rendered as text, never as markup.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[(&str, bool)]) -> Vec<ListingEntry> {
        names
            .iter()
            .map(|(name, is_dir)| ListingEntry {
                name: (*name).to_string(),
                is_dir: *is_dir,
            })
            .collect()
    }

    #[test]
    fn a_name_that_cannot_be_served_is_never_listed() {
        for hidden in [
            "",
            ".git",
            ".env",
            "a\\b",
            "app.js:stream",
            "name.",
            "name ",
        ] {
            assert!(!listable_name(hidden), "{hidden:?} must not be listed");
        }
        for shown in ["index.html", "a b.txt", "naïve.txt", "-"] {
            assert!(listable_name(shown), "{shown:?} must be listed");
        }
    }

    #[test]
    fn the_variant_siblings_are_not_entries_of_their_own() {
        assert!(is_variant_name("app.js.br"));
        assert!(is_variant_name("app.js.gz"));
        assert!(!is_variant_name("app.js"));
        assert!(!is_variant_name("brochure.txt"));
    }

    #[test]
    fn a_segment_escapes_every_byte_that_is_not_unreserved() {
        assert_eq!(encode_segment("app.js"), "app.js");
        assert_eq!(encode_segment("a b.txt"), "a%20b.txt");
        assert_eq!(encode_segment("q?x#y"), "q%3Fx%23y");
        assert_eq!(encode_segment("naïve"), "na%C3%AFve");
        assert_eq!(encode_segment("%2e%2e"), "%252e%252e");
    }

    #[test]
    fn a_hostile_name_is_rendered_as_text() {
        let listing = render_listing(
            "/assets",
            &entries(&[
                ("<script>alert(1)</script>", false),
                ("\"onmouseover=x", false),
            ]),
            true,
        );
        assert!(!listing.contains("<script>"), "{listing}");
        assert!(listing.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(listing.contains("&quot;onmouseover=x"));
    }

    #[test]
    fn directories_are_listed_with_a_trailing_slash_and_a_parent_link() {
        let listing = render_listing(
            "/assets/sub",
            &entries(&[("docs", true), ("a.txt", false)]),
            false,
        );
        assert!(listing.contains("<a href=\"../\">../</a>"));
        assert!(listing.contains("<a href=\"docs/\">docs/</a>"));
        assert!(listing.contains("<a href=\"a.txt\">a.txt</a>"));
        assert!(listing.contains("<title>Index of /assets/sub</title>"));
    }

    #[test]
    fn a_redirect_names_the_directory_once() {
        let redirect = |prefix: &str, relative: &str| {
            redirect_to_directory(prefix, relative)
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(redirect("/assets", "guide"), "/assets/guide/");
        assert_eq!(redirect("/assets", "a b/c"), "/assets/a%20b/c/");
        assert_eq!(redirect("/assets", ""), "/assets/");
    }

    #[test]
    fn the_root_listing_has_no_parent_link() {
        let listing = render_listing("/assets", &entries(&[("a.txt", false)]), true);
        assert!(!listing.contains("href=\"../\""));
    }

    /// A streamed file is read in fixed-size pieces, so its memory cost is the chunk and not
    /// the file, and an offset skips to the first byte of a range without reading past it.
    #[tokio::test]
    async fn a_file_body_reads_in_bounded_chunks_and_starts_at_its_offset() {
        let base = std::env::temp_dir().join(format!("aether_static_body_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("payload.bin");
        let content: Vec<u8> = (0..200 * 1024u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();

        let (mut body, len) = open_file(&path).await.unwrap();
        assert_eq!(len, 200 * 1024);
        let mut seen = Vec::new();
        let mut sizes = Vec::new();
        while let Some(chunk) = body.next_chunk().await {
            let chunk = chunk.expect("a readable file yields chunks");
            sizes.push(chunk.len());
            seen.extend_from_slice(&chunk);
        }
        assert_eq!(seen, content);
        assert!(
            sizes.iter().all(|&size| size <= STATIC_CHUNK),
            "a chunk larger than the read size: {sizes:?}"
        );
        assert_eq!(
            sizes,
            vec![STATIC_CHUNK, STATIC_CHUNK, STATIC_CHUNK, 8 * 1024]
        );

        let mut ranged = FileBody::open(&path, 65_530, 21).await.unwrap();
        let mut sliced = Vec::new();
        while let Some(chunk) = ranged.next_chunk().await {
            sliced.extend_from_slice(&chunk.expect("a range reads cleanly"));
        }
        assert_eq!(sliced, content[65_530..65_551]);

        // A plan longer than the file is an error, not a short body: the writer has already
        // promised a length it can no longer keep.
        let mut short = FileBody::open(&path, 0, len + 1).await.unwrap();
        let mut failed = false;
        while let Some(chunk) = short.next_chunk().await {
            if chunk.is_err() {
                failed = true;
            }
        }
        assert!(
            failed,
            "a file shorter than its plan must report the truncation"
        );

        assert!(
            open_file(&base).await.is_err(),
            "a directory is not a file answer"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
