# Aether Production Deployment Guide

## 1. Compiler Optimizations (`Cargo.toml`)
For maximum throughput and minimal tail latency:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
# Not "abort": a handler runs in a spawned task and `serve.rs` maps its JoinError to a 500 for
# that one request. With panic="abort" the whole process dies instead, and the tested guarantee
# `handler_panics_are_isolated_to_one_response` cannot hold. This is the same value the
# workspace's own `[profile.release]` selects.
panic = "unwind"
strip = true
overflow-checks = false
```

`overflow-checks = false` is safe here only because the arithmetic that decides limits is written
with `checked_*`/`saturating_*` (body-size enforcement, the memory re-pricing, the baseline median);
a handler that relies on a debug-only overflow panic behaves differently in this profile.

Build command:
```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## 2. Linux Kernel Sysctl Tuning
Apply the following kernel parameters for high concurrency:

```sysctl
# Increase socket backlog & max open files
net.core.somaxconn = 65535
fs.file-max = 2097152

# Epoll / Socket buffer tuning
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.tcp_fin_timeout = 15
net.ipv4.tcp_tw_reuse = 1
net.ipv4.ip_local_port_range = 1024 65535

# Fast TCP Keep-Alive
net.ipv4.tcp_keepalive_time = 300
net.ipv4.tcp_keepalive_intvl = 15
net.ipv4.tcp_keepalive_probes = 5
```

## 3. Container Dockerfile
```dockerfile
# Multi-stage build with native binary output
# rust:1.80 cannot build this workspace (edition 2024, and the framework is tested on 1.98.1).
FROM rust:1.98-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY . .
RUN cargo build --release -p aether-examples

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=builder /app/target/release/basic_api /usr/local/bin/aether-server
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/aether-server"]
```

> This Dockerfile was not executed in the environment that produced the readiness report — every
> verification there ran on Windows (MSVC) and Linux parity is tracked separately. Treat it as a
> shape, not as a tested image.

## 4. TLS on the same port

TLS is terminated in-process by `aether::tls` (rustls, `ring` provider — aws-lc-rs is avoided
because it compiles C through cmake and would make the Windows build depend on an external
toolchain). One listener serves `http://` and `https://`: the framing is decided from each
connection's first bytes, not from the port.

```rust
// `get` takes a closure whose *return value* is the response; an `async` block is a future, not
// a response, so a handler that returns one must be registered with `get_async`.
// Every registration and TLS builder consumes the app and returns it, so the whole setup —
// routes and transport — is one expression.
// chain first, then the leaf's key; an encrypted key or a key that does not match the
// chain is refused here, at build time, not on the first client hello
let app = AetherApp::new()
    .get_async("/", |_: RequestContext| async move { "hello" })
    .tls_from_files("/etc/letsencrypt/live/example.com/fullchain.pem",
                    "/etc/letsencrypt/live/example.com/privkey.pem")?
    // opt-in: refuse plaintext instead of answering it on the same port
    .require_tls(true);
```

(Since 0.2 every registration method takes `self` and returns `Self`, so routes and the TLS
builders chain into one expression. In statement form, rebind it: `app = app.get_async(..)`.)

Notes that matter in production:

- **ALPN advertises `h2`, then `http/1.1`.** A client that offers both is served HTTP/2 on this
  same port; a client that offers only HTTP/1.1 (curl built without HTTP/2, for instance) is
  served HTTP/1.1 and sees nothing different. To pin a listener to one framing, build the
  acceptor with `TlsAcceptorBuilder::advertise_h2(false)`, which removes `h2` from the offer
  entirely — the HTTP/2 frontend then never sees that port.
- **Cleartext HTTP/2 is opt-in.** `AetherApp::enable_h2c` accepts RFC 9113 §3.3 *prior-knowledge*
  HTTP/2 on the same listener that serves plaintext HTTP/1.1: a connection that opens with the
  24-byte preface is framed as HTTP/2, anything else stays HTTP/1.1. It is off by default because
  cleartext framing is what an intermediary can rewrite, and because a client that "tries h2 first"
  against a server that never agreed produces exactly the confusion this flag exists to make a
  deliberate choice. When it is on, nothing else changes: one `ServiceCore`, one set of limits,
  host validation, rate limiting, per-stream memory accounting and shutdown. A `require_tls(true)`
  acceptor still refuses every cleartext connection, including one carrying a preface.
- **`Upgrade: h2c` is not implemented, and is not planned.** RFC 9113 §3.1 deprecates the upgrade
  token and the `HTTP2-Settings` header, and RFC 7540 §3.2 requires the upgrade request itself to
  be answered as an implicitly half-closed stream 1 — a state `h2` does not synthesise. A client
  that insists on the exchange is served HTTP/1.1, which is the protocol it is speaking until the
  switch; a client that wants cleartext HTTP/2 today sends the preface.
- **A preface against a listener without `enable_h2c` is closed, not answered.** The client gets a
  graceful FIN, because an HTTP/1 status line where a preface was sent is something that client
  cannot parse.
- **Certificate renewal needs no restart.** `TlsAcceptor::replace_certificates(vec)` validates the
  whole new table before swapping one `Arc`; a rejected reload leaves the live certificate
  untouched. Get the acceptor with `AetherApp::tls_acceptor()` (the clone shares the certificate
  table). Fetch the new material with your ACME client of choice — Aether serves certificates, it
  does not order them.
- **SNI:** `TlsAcceptorBuilder::insert(TlsIdentity::for_name("api.example.com", set)?)` for extra
  hostnames; exactly one catch-all identity is required, and a hello with an unknown name gets the
  catch-all rather than a failed handshake.
- **Revocation is enforced from material you supply; nothing is fetched.** `TlsCertSet::with_crl`
  parses a PEM/DER CRL, verifies the signature with the chain's issuer, and refuses a set whose
  leaf serial it lists, a stale or not-yet-valid list, or a list signed outside the chain.
  `TlsCertSet::with_ocsp` staples a *basic* OCSP response only after checking that it answered
  `successful`, names this leaf (serial and both issuer hashes recomputed from the chain), reports
  `good` with `thisUpdate` not in the future and `nextUpdate` still ahead, and is signed by the
  chain's issuer or by an issuer-signed responder certificate carrying `id-kp-OCSPSigning`.
  Invalid material is refused at load or reload time and never sent. Refresh remains yours: hand a
  fresh list or staple to `replace_certificates`. A staple reaches a client only when that client
  sent `status_request`, so this protects what the server agrees to serve rather than every
  handshake.
- **Timeouts:** the first byte is bounded by `read_timeout` (5 s default) and the handshake by
  `ServerLimits::tls_handshake_timeout` (10 s default) — a client that completes the TCP handshake
  and then stalls costs a task and a socket buffer for that long, no longer. The HTTP/2 handshake
  that follows is bounded by `read_timeout` on both paths (ALPN-`h2` and h2c), so a connection that
  sends the preface and stops mid-settings is closed rather than held open.
- Behind a TLS-terminating proxy this is still the proxy you see in `peer_addr()`. A caller that
  needs the client's own address calls `AetherApp::enable_trusted_proxies` with the CIDR blocks of
  every proxy in front of it; the chain is then walked right to left and stops at the first hop
  that is not itself listed, `RequestContext::client_addr()` returns that address, and the rate
  limiter keys on it (`peer_addr()` keeps naming the socket). The headers are unsigned, so the
  list is what makes them believable: without one every forwarded claim is ignored, and a chain
  that crosses a proxy you did not list is not transparent.

Verified by 25 unit tests in `framework/aether/src/tls.rs` (15 of them revocation) and 11 wire tests in
`framework/tests/tests/tls_e2e.rs`, which mint a throwaway CA and leaf per run with `rcgen`
(no committed key material, no expiring fixtures) and drive real handshakes against a live
`AetherServer`, including WSS, SNI selection by trust anchor, refused plaintext, and reload.

Checked against clients that are not rustls, because both ends of the suite use the same library:
Windows `curl` (Schannel) and a Python/OpenSSL client each negotiated **ALPN `http/1.1` over
TLS 1.3** against a live server and received `HTTP/1.1 200 OK` through the TLS record; without
trusting the minted CA, curl exits 60 (the correct refusal), and an h2c preface gets an immediate
EOF rather than an HTTP/1 answer. Node.js `http2` negotiated `h2` and completed the whole HTTP/2
exchange over TLS 1.3 and TLS 1.2; on the cleartext side, python-h2 plus a hand-framed HEADERS
probe (neither of them Rust clients) drove the same routes with `enable_h2c` on — see the
independent-run records in [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).
