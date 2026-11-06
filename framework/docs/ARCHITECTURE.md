# Aether Architecture & Design Specification

> **DESIGN DOCUMENT — 2026-10-18.** Parts of this document describe intended or aspirational
> architecture, not the verified implementation. In particular, the server runs on Tokio's
> multi-threaded work-stealing runtime (not thread-per-core; and the diagram's io_uring and epoll
> interfaces are not in this tree either — no `.rs` file names either one, the reactor is Tokio's);
> request bodies and headers are
> copied into `RequestContext` before handlers run; and `RequestArena` is not used by the
> server path. HTTP/3 is not implemented. HTTP/1.1 runs over plaintext *and* over TLS on the
> same port (`transport.rs` + `tls.rs`, verified by `tests/tests/tls_e2e.rs`); HTTP/2 runs over
> TLS when ALPN negotiates `h2`, and over cleartext only on a listener that opted into
> `AetherApp::enable_h2c` (`http2.rs`, verified by `tests/tests/http2_e2e.rs`, by an independent
> Node.js client over TLS and by python-h2 over cleartext); an h2c preface on a listener that has
> not opted in is closed, not answered. See
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md) for verified status.

## 1. Architectural Philosophy
Aether is an ultra-high-throughput, zero-copy native web framework engineered to combine the developer ergonomics of FastAPI with the zero-allocation performance of Drogon (C++) and the fearless concurrency and memory safety of Rust.

```
                      ┌────────────────────────────┐
                      │    FastAPI-Style DX API    │
                      └─────────────┬──────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │    Precompiled Request     │
                      │       Execution Plan       │
                      └─────────────┬──────────────┘
                                    │
          ┌─────────────────────────┼────────────────────────┐
          ▼                         ▼                        ▼
┌──────────────────┐      ┌──────────────────┐     ┌──────────────────┐
│ Radix Trie       │      │ Compile-Time     │     │ Constant-Time    │
│ Router (Bitsets) │      │ Monomorphic DTO  │     │ Security & RFC   │
└─────────┬────────┘      └─────────┬────────┘     └─────────┬────────┘
          │                         │                        │
          └─────────────────────────┼────────────────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │   Zero-Copy HTTP Engine    │
                      │  (SmallVec + httparse)     │
                      └─────────────┬──────────────┘
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │ Thread-per-Core Async Core │
                      │  (io_uring / epoll / tokio)│
                      └────────────────────────────┘
```

## 2. Zero-Copy Borrowing Model
Standard frameworks allocate strings and heap vectors for request methods, path tokens, query maps, and headers.
Aether borrows directly from the socket read ring buffer:
- `RequestView<'req>`: Borrows `&'req str` for method, path, headers, and body.
- `SmallVec<[HeaderView<'req>; 16]>`: 16 headers allocated on the stack.
- `RequestArena`: Bump-pointer 4KB arena per worker connection. Resetting on Keep-Alive requests is an $O(1)$ offset zeroing operation.

## 3. Bitset-Accelerated Radix Router
- Routes are organized into a Radix compressed trie.
- Allowed methods are packed into a 64-bit scalar (`u64 bitmask`).
- Checking whether an incoming method is permitted is a single machine instruction:
  `if (node.allowed_methods & (1 << method.bit())) != 0`
- Extracted URL path parameters are placed into a `SmallVec<[Param<'req>; 4]>` directly borrowing slices from the URI string.

## 4. Precompiled Dependency Injection Graph
- Unlike Python FastAPI's dynamic runtime reflection on every request, Aether analyzes all handlers, singletons, database pools, and request-scoped services at application boot.
- Performs topological cycle validation (Kahn's algorithm).
- Generates an inlined DAG execution vector, resolving dependencies in deterministic $O(1)$ index accesses.

## 5. One Serving Core, Several Protocol Frontends
`framework/aether/src/serve.rs` holds the protocol-independent half of request handling —
routing, security rejections, CORS preflight, handler dispatch under the scheduling layer, and
response post-processing. `ServiceCore` is built once at `serve` and cloned per connection.

- A frontend only parses framing and writes bytes. It hands the core a `RequestParts`, and
  `RequestContext::from_parts` is the only way to reach a handler, so HTTP/1.1 and HTTP/2 cannot
  disagree about header dedup, parameter decoding or guard ordering.
- Per-request state lives on that one context, not on the connection: `RequestParts::extensions` is
  an empty `Extensions` type-map that middleware fills in as the request moves
  (`framework/aether/src/extensions.rs`). Because `RequestContext` moves — it is never cloned —
  through the chain into the handler, the map has a single owner at a time and needs no lock, and
  because it is created per request rather than per stream or per socket, an HTTP/2 connection with
  ten concurrent streams holds ten independent maps. Typed extraction
  (`framework/aether/src/extract.rs`) happens on the way into the handler, i.e. inside the core, so
  `Extension<T>`/`State<T>`/`Json<T>` behave identically on every frontend.
- Version-specific wire semantics stay in the frontend: the HTTP/1 `Connection` header, HEAD body
  suppression, status-line version echo, keep-alive draining and the WebSocket raw-stream handoff.
- A WebSocket upgrade request goes through the *same* dispatch path as an HTTP route
  (`ServiceCore::ws_middleware` folds the route's chain around a terminal that takes the request out
  of the response path and returns a `101` sentinel instead of answering it), so middleware can stamp
  extensions, refuse the upgrade with its own status, and is covered by admission, the request timeout
  and panic mapping like any handler. The upgrade proceeds only when the chain returns that `101`
  untouched, whether the guard answers before `next` or after awaiting it, and whether it ran the
  chain inline, inside `tokio::spawn`, or dropped its future — what the client gets is the guard's
  own response; the handshake line the
  client receives is still synthesized by the frontend, which is why a guard cannot attach a header to
  a successful handshake and why the post-processing pass does not touch it.
- `Connection`-based close decisions are HTTP/1.x only. An HTTP/2 client cannot close a
  multiplexed connection it does not own (RFC 7540 §8.1 forbids the field anyway).

### 5.1 Transport: one port, framing decided per connection
`framework/aether/src/transport.rs` is the layer under the frontends. A listener hands each
accepted socket to `classify`, which `peek`s — non-consuming, so the framing that gets chosen still
sees every byte — and decides from the first bytes: `0x16 0x03` → TLS record, the 24-byte
`PRI * HTTP/2.0…` preface → h2c, anything else → HTTP/1.1. A `peek` always reports from the
socket's *first unread byte* and never advances it, so what a later segment shows **replaces** the
prefix already in hand instead of extending it, and a discriminator that stalls mid-way is answered
from the bytes that did arrive after `CLASSIFY_RETRY_LIMIT` (4) waits of `CLASSIFY_RETRY_SLACK`
(25 ms) — ≈100 ms, bounded because a non-consuming peek is always ready. Nothing is decided by port
number, so `http://` and `https://` share a listener, and `establish` returns the type-erased
`Stream` that both the HTTP/1.1 loop and `WebSocket<S>` are written against
(`a_preface_arriving_in_two_segments_is_still_recognised_as_http2` pins the split case,
`classifying_leaves_the_bytes_for_the_protocol_that_uses_them` the non-consuming half).

Three consequences worth knowing before editing this file:
- **A refusal is a close, not an HTTP status.** Before any framing exists there is nothing to
  answer in, so a TLS client on a server with no certificate, a plaintext client on a
  `require_tls(true)` server, and an h2c preface on a listener that has not opted into
  `enable_h2c` all get a graceful FIN — `shutdown()`, because
  on Windows a bare drop turns into an RST that discards bytes already written.
- **ALPN advertises only what this build serves.** `h2` then `http/1.1` now that the HTTP/2
  frontend exists (§5.2); `TlsAcceptorBuilder::advertise_h2(false)` pins a port to HTTP/1.1, which
  is what a deployment that wants exactly one framing on the wire asks for. Advertising `h2`
  without the frontend would have made a client start HTTP/2 framing against a server that can
  only answer HTTP/1 — silent corruption, not an error.
- **Certificate state is one `Arc`.** `tls.rs` resolves SNI through a `ResolvesServerCert` that
  reads a `Mutex<Arc<SharedCerts>>`; `replace_certificates` validates a whole new table before
  swapping the pointer, so a failed reload leaves the live certificate untouched and an in-flight
  handshake keeps the snapshot it already took. Unknown SNI falls back to the catch-all rather
  than aborting, and a builder with no catch-all is refused at build time.

The breaking change this introduced: handler signatures that name the transport explicitly moved
from `WebSocket<tokio::net::TcpStream>` to `WebSocket<aether::transport::Stream>`.

### 5.2 HTTP/2 frontend: `h2` drives, `ServiceCore` answers
`framework/aether/src/http2.rs` is a frontend, not another server. `transport::establish` decides
the framing once and returns `Frontend::{Http1, Http2}` — from ALPN on a TLS connection, from the
preface plus `enable_h2c` on a plaintext one — and the listener dispatches on that value. Deriving
it a second time from the stream would not be a fallback but a wrong answer: a plaintext connection
has no ALPN to report.

Two properties of `h2::server::Connection` shape the loop. It is neither `Clone` nor a `Future`:
`accept()` *is* the driver, so a handler is never awaited inline — every stream goes to its own
task and the loop returns to `accept()` immediately, which is also what keeps the accept path off
the handler's timeout. And the connection must keep being polled until it reports closed, so
shutdown calls `graceful_shutdown()` (a GOAWAY), keeps accepting and polling while in-flight
streams finish inside the same drain budget the listener gives a draining socket, then releases it.

What is ours versus `h2`'s:

- Ours: the translation. The pseudo-header view becomes the same `RequestParts` HTTP/1 produces
  (`:authority` folded into `host` — and replacing a `Host` the client sent, the way an
  absolute-form target replaces it on HTTP/1 — lowercase dedup, the per-line head budget applied
  line by line), so both frontends answer one request identically because they call one
  implementation.
- Ours: the money. Each accepted stream re-prices the connection registration with what it
  actually holds — `STREAM_BASE_MEMORY` plus body bytes as they arrive — the way the HTTP/1 loop
  re-prices its read buffer, and a stream the memory budget cannot cover is shed with 503 while
  the streams already running on that socket finish untouched.
- Ours: the response head is filtered against `FORBIDDEN_FIELDS` before it goes on the wire,
  because `h2` answers a connection-specific field with a user error that resets the stream — a
  handler that legitimately set `Connection: close` for HTTP/1 must not destroy its own response
  over h2. The request direction needs no filter: `h2` refuses those fields before the handler is
  reached, verified with HEADERS blocks framed by hand.
- Ours: the handshake is bounded by `read_timeout`, on both the TLS/ALPN path and h2c. Without it
  a client that completes the TCP handshake and then stops mid-preface holds a task, a connection
  permit and a socket buffer — the slowloris class the header path already defends against, one
  layer earlier.
- Ours: the credit. `h2` accounts for flow control, but it only credits the peer for bytes the
  application says it is done with, and `RecvStream::drop` clears the buffer without crediting
  anything. `read_body` therefore calls `release_capacity(n)` for every chunk it consumes, and
  releases nothing when it refuses a body — abandoning the stream is what ends it. A frontend that
  skips this reads exactly one receive window and then waits for bytes the client is forbidden to
  send, so an upload larger than 64 KiB is indistinguishable from a stalled one and dies at the
  request deadline; that was the real defect in this module, found only by a client that enforces
  the window instead of assuming it. The same rule applies in the other direction: a test harness
  that reads a response body without releasing capacity stalls *the server*.
- `h2`'s: the stream state machine, HPACK, framing, flow-control accounting, PING and SETTINGS.
  The two advertised settings are the HTTP/1 limits they correspond to — `max_concurrent_streams`
  and the header byte budget, which earns a 431 from `h2` itself before this module sees the head.
  Window sizes are not advertised, so `h2`'s 64 KiB defaults stand: `builder` is deliberately left
  at the defaults until there is a measurement of what a larger window buys against the memory
  each stream may then hold.

A WebSocket upgrade cannot arrive on either HTTP/2 path — RFC 9113 §8.1 has no upgrade mechanism —
so a WebSocket route reached over h2 answers 400 rather than pretending to switch protocols.

## 6. Algorithm & data-structure strategy
See [`adr/ADR-001-algorithm-strategy.md`](adr/ADR-001-algorithm-strategy.md): what is already
implemented, what is evidence-gated (and therefore parked until a measurement justifies it), and
what is rejected outright.
