# Aether 0.2 — migrating from 0.1

0.2 is a breaking release with two public API changes. Both fail at compile time, and neither changes
what the server sends on the wire.

الخلاصة بالعربية: (١) دوال التسجيل على `AetherApp` صارت تأخذ `self` وتُرجع `Self`، فالسلسلة أو
`app = app.get(...)` بدل الاستدعاء المجرّد؛ (٢) حقل `RequestContext::headers` العام اختفى وحلّ محله
مخزن واحد يُقرأ عبر `header` و`header_values` و`headers`.

The workspace version lives in `framework/Cargo.toml` under `[workspace.package]`, and every member
reads it with `version.workspace = true`, so consuming the path moves all crates together.

## 1. Builder methods take `self` and return `Self`

In 0.1 the registration and configuration methods on `AetherApp` took `&mut self` and returned
`&mut Self`. In 0.2 they take `self` and return `Self`, so they can be chained in one expression. The 38 methods whose signature changed are:

`use_middleware`, `manage`, `enable_cors`, `enable_security_headers`, `enable_security_headers_with`,
`enable_compression`, `enable_host_validation`, `enable_rate_limit`, `enable_csrf`,
`enable_trusted_proxies`, `enable_request_logging`, `get`, `get_async`, `post`, `post_async`, `put`,
`put_async`, `delete`, `delete_async`, `patch`, `patch_async`, `route_sync`, `route_sync_with`,
`route_async`, `route_async_with`, `route_stream`, `ws`, `ws_subprotocols`, `ws_with`, `group`,
`mount`, `static_asset`, `static_file`, `static_dir`, `static_dir_with_cache`,
`static_dir_with_options`, `with_scheduling`, `with_scheduling_resources`.

Methods that already consumed `self` in 0.1 — `tls`, `tls_from_files`, `tls_from_pem`, `require_tls`,
`with_websocket_limits`, `enable_h2c`, `run`, `listen`, `bind`, `serve` — are unchanged.

Both styles compile:

```rust
use aether::prelude::*;

// One expression: the chain builds the whole app.
let app = AetherApp::new()
    .get("/", |_| "hello")
    .enable_compression()
    .tls_from_files("fullchain.pem", "privkey.pem")?
    .require_tls(true);
app.listen("0.0.0.0:3000").await?;

// Or rebind after each call.
let mut app = AetherApp::new();
app = app.get("/", |_| "hello");
app = app.enable_compression();
```

What does **not** compile is the 0.1 statement form:

```rust
let mut app = AetherApp::new();
app.get("/", |_| "hello");   // moves `app`
app.listen("0.0.0.0:3000");  // error[E0382]: use of moved value: `app`
```

That failure mode is the point: a half-converted call site is a compile error, never a route that
silently went missing. The entry points `listen`, `bind`, `run` and `serve` consumed
`self` before 0.2 and still do.
`RouteGroup` and `Router` keep `&mut self`, so a configure closure is unchanged:

```rust
let app = AetherApp::new().group("/api", |g| {
    g.get("/ping", |_| "pong");
});
```

## 2. `RequestContext` has one private header store

In 0.1 `RequestContext` exposed `pub headers: HashMap<String, String>`, keyed by the wire casing of
the first field line, beside a private store that held every repeated line and that `header()` /
`header_values()` actually read. A middleware could mutate the public map, and those writes were
visible to a later handler but not to the framework's own decisions — the two-store defect registered
as PROB-0005.

In 0.2 the field is gone. One store holds every line under a lowercase name, and three read-only
accessors expose it:

| Accessor | Returns |
| :--- | :--- |
| `header(name)` | the first field line for `name`, case-insensitive, as `Option<&str>` |
| `header_values(name)` | every field line for `name`, in wire order, as `Vec<&str>` |
| `headers()` | every field as `(lowercase name, first line)`, in no particular order |

There is no mutable counterpart. A guard that used to rewrite the request refuses by returning a
response instead. Per-request data that is not a header belongs in the extension map:
`insert_extension` to add a value, `extensions_mut` to reach the map.

Porting reads:

| 0.1 | 0.2 |
| :--- | :--- |
| `req.headers.get("X-Id")` | `req.header("x-id")` — returns `Option<&str>`, not `Option<&String>` |
| `req.headers.get("X-Id").cloned()` | `req.header("x-id").map(str::to_string)` |
| `for (k, v) in &req.headers` | `for (k, v) in req.headers()` — keys are lowercase now |
| `req.headers.len()` | `req.headers().count()` |
| `req.headers.insert(k, v)` / `remove(k)` | no equivalent; return a response, or use `insert_extension` |

Wire semantics did not move: `header()` still reports the first line, `header_values()` still keeps
every line, and both frontends still build the same store from the request they parsed.

## 3. Checking a consumer

A consumer that only called `header()` / `header_values()` and never touched the public map needs
nothing beyond the builder rebinding, because the read accessors kept their names and their meaning.
A consumer that enumerated or mutated `headers` needs the mapping above. In this repository the
downstream application `my_cool_site` was ported and compiled against 0.2 in the same pass.

The workspace suite and the gate ladder are the check that the release is what this note describes;
`PRODUCTION_READINESS_REPORT.md` carries the measured reading for the tree that ships with 0.2.
