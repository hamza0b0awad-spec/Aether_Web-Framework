# AETHER CORE CAPABILITIES SPECIFICATION & ARCHITECTURAL PROTECTION MATRIX

> **AUDIT NOTE (2026-10-18):** `RequestArena` exists and is tested, but it is **not** used by
> the server request path (the allocator is the system allocator). "Zero-copy" applies to the
> parser's borrowed slices; headers, parameters, and the body are copied into owned values
> before handlers run. HTTP/2 is implemented over TLS (ALPN `h2`) through `h2` + the shared
> `ServiceCore`, and over plaintext on a listener that opts into `AetherApp::enable_h2c`
> (RFC 9113 §3.3 prior knowledge); the `Upgrade: h2c` exchange and HTTP/3 are not. See
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).


This document specifies the core engineering assets, non-negotiable architectural invariants, and performance protections that define **Aether Web Framework**.

---

## 1. Core Architectural Pillars

### 1.1 Single-Pass Zero-Copy HTTP/1.1 Parser (`RequestView`)
- **Location:** `framework/http/src/parser.rs`
- **Entry point the serving loop calls:** `parse_request_incremental` (`parser.rs:748`), which returns
  the borrowed `RequestView` plus the decoded body and the consumed-byte count. `RequestView::parse`
  (`parser.rs:59`) is the `Content-Length`-only variant. The app's serving loop does not call it, but
  "unused" would be wrong: `framework/runtime/src/worker.rs:32` is a worker loop the runtime crate
  still exports and its own `connection.rs` test target drives, `framework/fuzz/src/lib.rs:86` is the
  fuzz oracle, and the `parser_properties` suite plus the `aether-tests` harness helpers all call it
  directly; the property suite exists to hold the two entry
  points to the same answers. Track 110 tested that claim by mutation and found the one shape nothing
  pinned in the buffered variant — a body with bytes behind its declared length. It now has
  `test_content_length_surplus_is_left_for_the_next_request`, and deleting the stop-at-promise line
  turns it red.
- **Invariant:** Direct slice borrowing (`&'req [u8]`, `&'req str`) from incoming TCP bytes.
- **Why It Matters:** Eliminates intermediate string and header heap allocations during the critical parsing phase.
- **RFC 9112 Protections:**
  - Dual `Content-Length` detection and rejection with `400 Bad Request` — any second line counts,
    even one that agrees with the first (`a_repeated_content_length_is_refused_on_http_1_even_when_it_agrees`),
    which is stricter than RFC 9110 §6.3.2.2 and is what stops a front-ending proxy from ever holding a
    different reading than this server.
  - `Transfer-Encoding` + `Content-Length` conflict detection (CL.TE / TE.CL desync defense).
  - Strict line-ending parsing enforcing CRLF conformance.
  - A body read to its declared length and no further, so the surplus stays the next message on the
    connection (`a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request`) and a
    surplus that does not parse is a `400` then a close rather than a longer body
    (`bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close`).
  - Header whitespace trimmed as SP and HTAB only, so a non-ASCII space before a length is a `400`
    (`a_non_ascii_padded_content_length_is_refused_on_http_1`) while a real OWS padding is served
    (`an_ows_padded_content_length_is_served_on_http_1`).

### 1.2 Deterministic $O(K)$ Radix Trie with Bitmask Dispatch (`RadixRouter`)
- **Location:** `framework/router/src/trie.rs`
- **Invariant:** 
  - Segment sorting (`Static > Param > CatchAll`) ensuring deterministic path matching.
  - Stack allocation of path traversal using `SmallVec<[&str; 8]>`.
  - Single-instruction method bitmask validation (`allowed_methods & method.bitmask()`, `router/src/node.rs:43`).
- **Why It Matters:** Zero heap allocation during route resolution regardless of routing table size.

### 1.3 Memory Alignment & Cache-Conscious CCB (`ConnectionControlBlock`)
- **Location:** `framework/core/src/ccb.rs`
- **Invariant:** 64-byte cacheline-aligned descriptor with packed bitsets (`Bitset64`).
- **Why It Matters:** Enables 100,000 idle sockets in <45MB RAM overhead with zero false sharing across CPU cores.

### 1.4 Zero-Leak Request Context (`RequestContext`)
- **Location:** `framework/aether/src/app.rs`
- **Invariant:** Bounded lifetimes without `Box::leak`, with fast-path extractors (`json()`, `param()`, `query()`, `cookie()`, `state()`).
- **Also carries:** `Extensions` (`framework/aether/src/extensions.rs`) — a `TypeId`-keyed map of
  values that belong to *this* request, moved with the context through the middleware chain into the
  handler. It is boxed only on the first insert, so a request that never uses it allocates nothing.
  A WebSocket handler receives the same map: the route's middleware runs around its upgrade request
  before the handshake is validated (see the protection matrix below).

### 1.5 Precompiled Topological DAG Dependency Graph
- **Location:** `framework/dependency/src/dag.rs`
- **Invariant:** Kahn's topological sort executed at server startup.
- **Why It Matters:** Discovers circular dependency deadlocks before opening the TCP port on `0.0.0.0:3000`.

---

## 2. Core Feature Protection Matrix

| Aether Core Feature | What Could Break It | Protection Strategy | Verification Result |
| :--- | :--- | :--- | :--- |
| **RequestArena / Buffer Reuse** | Blindly replacing with complex object pools | Keep arena for request-scoped bump allocation; use connection buffers | **PROTECTED** |
| **Zero-Copy Parser** | Adding excessive string conversion layers | Retain `RequestView` slice borrowing | **PROTECTED** |
| **RadixRouter & Bitmasks** | Replacing with dynamic regex or boxing | Retain stack `SmallVec` and bitmask matching | **PROTECTED** |
| **Dependency DAG** | Runtime dynamic reflection | Keep precompiled Kahn's algorithm at boot | **PROTECTED** |
| **JWT Constant-Time** | Naive `==` string checks | Retain `ring::hmac` verification and the hand-written `constant_time_eq` | **PROTECTED — reachable from the facade since track 91 (2026-10-28) as `aether::JwtValidator` and `aether::constant_time_eq`** |
| **RFC 9112 Hardening** | Permissive header parsing | Retain strict desync & smuggling checks | **PROTECTED** |
| **Zero Global Contention** | Global logger / route locks | Monomorphic inline execution & atomic counters | **PROTECTED** |
| **Request-scoped `Extensions`** | Attaching the map to the connection, or cloning `RequestContext` per middleware | Keep exactly one owner per request (the context moves, it is never cloned) and hand values over by `remove`; a non-`Clone` value is the tripwire | **PROTECTED — `extensions_are_per_stream_never_per_connection` holds four concurrent HTTP/2 streams in the chain at once and requires each to read only its own value, and `extensions_do_not_leak_between_requests_on_one_connection` does the same for two keep-alive HTTP/1.1 requests on one socket** |
| **Middleware covers every route, WebSocket included** | A route kind registered outside the wrapped handler maps, so a guard silently protects HTTP only. Or the chain's answer is discarded once the route is reached, so a guard that checks after `next` has no power | Fold the route's chain around the upgrade request and dispatch it through the same path as an HTTP handler; upgrade only when the chain returns the terminal's `101` untouched, otherwise serve the guard's status — including when the guard forks the chain into `tokio::spawn` or races it with `select!` | **PROTECTED — `a_websocket_upgrade_runs_the_app_middleware_chain`, `a_middleware_can_refuse_a_websocket_upgrade`, `a_middleware_that_answers_after_next_refuses_the_upgrade`, `a_middleware_hanging_after_next_times_the_upgrade_request_out`, `a_panicking_middleware_answers_the_upgrade_request_with_500` (before and after `next`), `a_middleware_that_forks_the_chain_still_answers_the_upgrade_request`, `a_middleware_that_abandons_the_chain_future_refuses_the_upgrade`, `a_middleware_cannot_grant_or_revoke_a_websocket_upgrade`, `group_middleware_wraps_the_upgrade_request_of_a_group_websocket_route`, `middleware_registered_after_a_websocket_route_does_not_wrap_it` (PROB-0017, 2026-10-21), `per_route_middleware_wraps_the_upgrade_request_of_its_websocket_route`, `a_per_route_guard_refuses_a_websocket_upgrade_before_the_handover` (track 94, 2026-10-29). Calling `next` twice is not a case to protect: `Next` consumes a non-`Clone`, crate-internal-constructed `RequestContext`, so it does not compile. The handshake line itself stays frontend-synthesized, so middleware cannot add a header to a successful upgrade, and it cannot revoke one by editing `RequestContext::headers` either — 0.2 removed the public map, so one store is read through read-only accessors and a returned response is the only lever (`a_middleware_cannot_grant_or_revoke_a_websocket_upgrade`). `allows_upgrade` is a transport capability, not a header scan, so only a non-`GET` (405) or an HTTP/2 stream (400) is refused before the chain; a plain `GET` on a WS route runs it and the validator answers 400/426. Since track 95 the `101` can also carry one line the client asked for — `Sec-WebSocket-Protocol` — but only from the route's own declaration (`ws_subprotocols`), never from the client's offer, and an offer a declared route cannot meet is refused `400` before the handover: `an_offered_subprotocol_is_echoed_from_the_route_declaration`, `a_route_without_a_protocol_declaration_ignores_the_offer`, `an_offer_that_matches_no_declared_protocol_is_refused`, `a_client_that_offers_no_subprotocol_still_gets_the_route`, `a_group_websocket_route_negotiates_its_subprotocol` (with `only_a_declared_name_is_ever_echoed` pinning the no-injection half at the unit level)** |
| **A streamed request body cannot smuggle its tail** | Letting a handler answer before the body is read, then reusing the connection anyway, so the unread remainder is parsed as the next request; or letting a body that failed its framing leave the socket reusable | `settle()` reuses the connection only when the framing completed on its own terms (`ended_clean`), never after a failure or a peer that vanished; an unfinished body answers, then closes, then discards under a bound; a declared `Content-Length` over `max_stream_body_bytes` is refused `413` before dispatch | **PROTECTED — `an_unread_body_closes_the_connection_after_the_response`, `a_body_past_the_ceiling_mid_stream_fails_the_read_and_closes`, `a_small_early_answered_body_keeps_the_connection_alive`, `a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch` (track 100, 2026-10-29); the failure-report path was found by the lane's own unit test `a_failure_is_reported_once_and_ends_the_body`. HTTP/2 answers the same route from the same head (track 100b, 2026-10-30): an unread body is abandoned by dropping the `RecvStream`, so the tail costs its own stream and the connection serves the next one — never a drain, never a reusable socket with an unparsed tail — pinned by `an_unread_h2_body_answers_and_the_connection_stays_usable`, `a_declared_length_over_the_streaming_ceiling_is_413_before_dispatch_over_h2` and `an_h2_body_past_the_ceiling_mid_stream_fails_the_read`**. Track 110 (2026-10-31) pinned the promise itself, on both frontends: over HTTP/1 the body stops at the declared length and the surplus is the next message (`a_body_longer_than_its_declared_length_leaves_the_surplus_for_the_next_request`, `bytes_past_the_declared_length_that_are_not_a_request_are_400_and_close`, and the same pair over the lane — `a_streamed_lane_stops_at_its_declared_length_and_the_surplus_is_the_next_request`, `a_streamed_lane_whose_surplus_is_not_a_request_is_400_and_close`); over HTTP/2 the `h2` receive path resets the stream before the head ever reaches this framework, which eight hand-framed tests observe (`a_body_longer_than_its_declared_length_is_reset_over_h2` and its seven siblings), each also asserting no GOAWAY and that the connection's next request is served |
| **The OpenAPI document is a function of the route table** | A spec built by hand, which drifts from what the server answers and keeps passing every test while it does. A `PathItem` with one field per verb, which silently drops every registration whose verb the struct does not name — that is how a `PUT` route vanishes from a document. A document that depends on the order routes were registered in, so a review diff shows moves instead of changes | The app asks its own router for the listing (`RadixRouter::route_listing`) and derives the document from it (`OpenApiSpec::from_routes`): sorted by `(path, method)` before anything reads it, verbs as map keys rather than fields, a `*name` catch-all rendered as the `{name}` slot OpenAPI does have, an `operationId` collision resolved by a position in that same sorted order, and a WebSocket route classified by the handler behind the registration instead of by its `GET` method. No handler is consulted, so the document claims no schema and says so in the descriptions it does emit | **PROTECTED — `route_listing_rebuilds_each_declared_template`, `route_listing_stamps_shared_levels_with_each_routes_own_names`, `route_listing_reports_the_canonical_form_of_each_declaration`, `route_listing_is_independent_of_registration_order` (unit, `aether-router`); `the_document_uses_the_specs_field_names`, `a_catch_all_is_derived_as_a_path_parameter`, `the_derived_document_does_not_depend_on_input_order`, `operation_ids_are_unique_when_two_paths_sanitise_alike`, `a_websocket_registration_answers_the_upgrade_and_its_refusals`, `nothing_in_a_derived_document_claims_a_body_schema`, `a_malformed_slot_declares_no_parameter` (unit, `aether-openapi`); `the_document_covers_every_route_the_app_registered`, `a_shared_dynamic_level_is_named_per_route`, `a_group_and_a_mount_contribute_their_prefixed_routes`, `a_static_dir_is_listed_as_the_catch_all_it_registers`, `a_websocket_route_carries_the_upgrade_instead_of_a_twenty`, `the_document_omits_the_answers_that_have_no_registration`, `the_document_is_the_same_whatever_order_the_routes_were_registered_in`, `the_derived_json_carries_the_specs_keys` (wire, `tests/tests/openapi_derivation.rs`). Track 108 injected eight mutants of this mechanism and each was killed by at least one of these tests (2026-10-31); the honest half is that dropping the router's sort and rendering a catch-all as a parameter are caught only by the router's own tests, because the generator re-sorts and re-templates both shapes itself, so the app-level target cannot see those two defects** |

> **Wiring note (verified 2026-10-20; corrected 2026-10-28).** The JWT / constant-time row protects
> code that lives in `aether-security`. Until track 91 the facade did not depend on it, so the
> capability was not reachable from the framework a user installs. `aether` now depends on
> `aether-security` and `aether-openapi` and re-exports them as
> `aether::{JwtValidator, JwtValidationOptions, VerifiedJwt, constant_time_eq, audit_request_desync}`,
> `aether::security`, `aether::openapi` and `aether::header`, pinned by the `facade_wiring` tests.
> `aether::openapi` is more than a data model since track 108 (2026-10-31): `AetherApp::openapi_spec`
> derives the document from the app's own route table, so what a user publishes is what the server
> answers.
> The last two crates that built inside the workspace with zero dependents - the observability timer
> and the serialization wrapper - were deleted in track 111 (2026-11-02): no member depended on
> either, and neither carried a test, a test target or a doc example. The same track removed from
> `aether/Cargo.toml` the `aether-runtime` and `aether-dependency` declarations that no source line
> in `aether` named; both crates stay workspace members and their tests still run (`connection` 3 and
> 4 unit respectively, re-measured here). The `subtle` crate referenced here previously is not a
> dependency of any member.

---

## 3. Capability Transfer & Non-Regression Rules

1. **Extractors (`Json<T>`, `Query<T>`, `Path<T>`):** Must compile to direct stack conversions without intermediate boxing.
2. **`IntoResponse` Trait:** Must format directly to `BytesMut` with zero temporary string allocation.
3. **Lifecycle Hooks:** Must be zero-cost inline branches when omitted by the application.
