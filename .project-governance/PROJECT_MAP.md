# PROJECT MAP - Current Repository Evidence Map
_Generated: 2026-11-04T16:36:32Z by 4.1-clean-en_
_Root Path: C:\Users\hamza\Desktop\nwfm\aether-web-framework_
_Regenerate with: `python -B .project-governance/tools/project_governance_bootstrap.py map`_

## 🧰 Detected Stack
- Rust
- Python

## ⚡ Source & Config Files
Total Files Scanned: 138 executable/config files (markdown, data files, files over 800 KB and `.project-governance/` excluded)

| Path | Summary | Description |
|---|---|---|
| `gates.sh` | Executable .sh | . | Source code file |
| `framework/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/aether/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/benchmarks/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/core/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/dependency/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/examples/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/examples/basic_api.rs` | Rust Code | framework/examples | Key functions: main |
| `framework/examples/full_async_api.rs` | Rust Code | framework/examples | Key functions: main |
| `framework/examples/scheduling_demo.rs` | Rust Code | framework/examples | Key functions: main |
| `framework/fuzz/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/http/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/middleware/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/openapi/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/router/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/runtime/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/scheduler/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/security/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/tests/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/validation/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/websocket/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/aether/src/app.rs` | Rust Code | framework/aether/src | Key functions: next_request_id |
| `framework/aether/src/auth.rs` | Rust Code | framework/aether/src | Key functions: require_bearer |
| `framework/aether/src/body_pump.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/config.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/cookie.rs` | Rust Code | framework/aether/src | Key functions: issued |
| `framework/aether/src/diagnostics.rs` | Rust Code | framework/aether/src | Key functions: fmt, set_error_reporter |
| `framework/aether/src/extensions.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/extract.rs` | Rust Code | framework/aether/src | Key functions: example, from_parts, from_request |
| `framework/aether/src/handler.rs` | Rust Code | framework/aether/src | Key functions: call, call, call, call |
| `framework/aether/src/http2.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/lib.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/request_body.rs` | Rust Code | framework/aether/src | Key functions: fmt |
| `framework/aether/src/scheduling.rs` | Rust Code | framework/aether/src | Key functions: progressed, finished |
| `framework/aether/src/serve.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/static_dir.rs` | Rust Code | framework/aether/src | Key functions: with_max_age, default |
| `framework/aether/src/test_client.rs` | Rust Code | framework/aether/src | Key functions: example, start, addr, url |
| `framework/aether/src/tls.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/transport.rs` | Rust Code | framework/aether/src | Key functions: peer_addr, negotiated_alpn, poll_read, poll_write |
| `framework/benchmarks/benches/router_bench.rs` | Rust Code | framework/benchmarks/benches | Key functions: bench_radix_router |
| `framework/core/src/arena.rs` | Rust Code | framework/core/src | Key functions: with_capacity, alloc |
| `framework/core/src/bitset.rs` | Rust Code | framework/core/src | Key functions: new, set, clear, contains |
| `framework/core/src/ccb.rs` | Rust Code | framework/core/src | Key functions: new, is_keepalive, is_tls, increment_requests |
| `framework/core/src/diagnostic.rs` | Rust Code | framework/core/src | Key functions: code_str, title |
| `framework/core/src/error.rs` | Rust Code | framework/core/src | Key functions: error_code, to_diagnostic |
| `framework/core/src/lib.rs` | Rust Code | framework/core/src | Source executable |
| `framework/dependency/src/dag.rs` | Rust Code | framework/dependency/src | Key functions: new, add_node, compile_execution_plan |
| `framework/dependency/src/lib.rs` | Rust Code | framework/dependency/src | Source executable |
| `framework/dependency/src/scope.rs` | Rust Code | framework/dependency/src | Source executable |
| `framework/docs/perf-baseline/extensions_pricing-2026-10-26.rs` | Rust Code | framework/docs/perf-baseline | Source executable |
| `framework/docs/perf-baseline/socket_paths-2026-10-19.rs` | Rust Code | framework/docs/perf-baseline | Key functions: done_work, serve_conn, window_complete, starts_with_crlfcrlf |
| `framework/fuzz/fuzz_targets/http_parse_fuzz.rs` | Rust Code | framework/fuzz/fuzz_targets | Key functions: usage, unsigned, seconds, main |
| `framework/fuzz/src/lib.rs` | Rust Code | framework/fuzz/src | Key functions: mix, scale, new, next_u64 |
| `framework/http/src/header.rs` | Rust Code | framework/http/src | Key functions: value_str |
| `framework/http/src/lib.rs` | Rust Code | framework/http/src | Source executable |
| `framework/http/src/limits.rs` | Rust Code | framework/http/src | Source executable |
| `framework/http/src/method.rs` | Rust Code | framework/http/src | Key functions: bit, bitmask, from_bytes, as_str |
| `framework/http/src/multipart.rs` | Rust Code | framework/http/src | Key functions: text, header, parse |
| `framework/http/src/parser.rs` | Rust Code | framework/http/src | Key functions: ows_trim, ows_trim_bytes, parse |
| `framework/http/src/response.rs` | Rust Code | framework/http/src | Key functions: next_chunk, new, from_chunks, next_chunk |
| `framework/http/src/static_files.rs` | Rust Code | framework/http/src | Key functions: mime_from_extension, generate_etag, etag_matches, safe_resolve_path |
| `framework/http/src/url.rs` | Rust Code | framework/http/src | Key functions: hex_val, percent_decode_cow, percent_decode, form_decode |
| `framework/http/tests/parser_properties.rs` | Rust Code | framework/http/tests | Key functions: limits, method_strategy, path_strategy, valid_request_strategy |
| `framework/middleware/src/compression.rs` | Rust Code | framework/middleware/src | Key functions: default, default, new, apply |
| `framework/middleware/src/cors.rs` | Rust Code | framework/middleware/src | Key functions: default, new, apply, apply_for_origin |
| `framework/middleware/src/forwarded.rs` | Rust Code | framework/middleware/src | Key functions: new, trusts |
| `framework/middleware/src/host_validation.rs` | Rust Code | framework/middleware/src | Key functions: new |
| `framework/middleware/src/lib.rs` | Rust Code | framework/middleware/src | Source executable |
| `framework/middleware/src/logger.rs` | Rust Code | framework/middleware/src | Key functions: new, enabled, start, log |
| `framework/middleware/src/rate_limit.rs` | Rust Code | framework/middleware/src | Key functions: default, new |
| `framework/middleware/src/security.rs` | Rust Code | framework/middleware/src | Key functions: default, new |
| `framework/openapi/src/lib.rs` | Rust Code | framework/openapi/src | Source executable |
| `framework/router/src/lib.rs` | Rust Code | framework/router/src | Source executable |
| `framework/router/src/node.rs` | Rust Code | framework/router/src | Key functions: new, supports_method, get_handler, add_handler |
| `framework/router/src/params.rs` | Rust Code | framework/router/src | Source executable |
| `framework/router/src/trie.rs` | Rust Code | framework/router/src | Key functions: new, allow_header_for_target, bitmask_to_allow_header |
| `framework/router/tests/router_properties.rs` | Rust Code | framework/router/tests | Key functions: token, a_registered_static_route_matches_its_own_path, parametrised_route |
| `framework/runtime/src/lib.rs` | Rust Code | framework/runtime/src | Source executable |
| `framework/runtime/src/worker.rs` | Rust Code | framework/runtime/src | Key functions: handle_connection |
| `framework/runtime/tests/connection.rs` | Rust Code | framework/runtime/tests | Key functions: demo_router, serve_once, exchange, a_request_split_across_two_reads_is_answered_once |
| `framework/scheduler/examples/active_trace.rs` | Rust Code | framework/scheduler/examples | Key functions: ns, dump |
| `framework/scheduler/examples/pump_cost.rs` | Rust Code | framework/scheduler/examples | Key functions: request_cycle, bench, main |
| `framework/scheduler/src/baseline.rs` | Rust Code | framework/scheduler/src | Key functions: from_cfg, observe_completed, sample_count, value |
| `framework/scheduler/src/config.rs` | Rust Code | framework/scheduler/src | Key functions: default |
| `framework/scheduler/src/engine.rs` | Rust Code | framework/scheduler/src | Source executable |
| `framework/scheduler/src/job.rs` | Rust Code | framework/scheduler/src | Key functions: is_divisible, is_terminal, index, connection |
| `framework/scheduler/src/lib.rs` | Rust Code | framework/scheduler/src | Source executable |
| `framework/scheduler/src/policy.rs` | Rust Code | framework/scheduler/src | Key functions: name, admission_order, pick_execution, quota |
| `framework/scheduler/src/quota.rs` | Rust Code | framework/scheduler/src | Key functions: grant, observe_slice |
| `framework/scheduler/src/resources.rs` | Rust Code | framework/scheduler/src | Key functions: new, over_ceiling |
| `framework/scheduler/src/sim.rs` | Rust Code | framework/scheduler/src | Key functions: name, new, next_u64, f64 |
| `framework/scheduler/src/tree.rs` | Rust Code | framework/scheduler/src | Key functions: new, queue, is_queued, unqueue |
| `framework/scheduler/tests/engine.rs` | Rust Code | framework/scheduler/tests | Key functions: ms, cfg_with, res_with, engine |
| `framework/scheduler/tests/memory_reprice.rs` | Rust Code | framework/scheduler/tests | Key functions: ms, engine, activate, growth_beyond_the_budget_is_refused_and_changes_nothing |
| `framework/scheduler/tests/policy_extension.rs` | Rust Code | framework/scheduler/tests | Key functions: ms, slots, new, bump |
| `framework/scheduler/tests/pump_head_of_line.rs` | Rust Code | framework/scheduler/tests | Key functions: ms, engine, tree, state |
| `framework/scheduler/tests/sim.rs` | Rust Code | framework/scheduler/tests | Key functions: workload_is_reproducible_byte_for_byte, every_job_is_accounted_for_exactly_once, all_policies_do_identical_total_work_when_nothing_is_rejected |
| `framework/scheduler/tests/stress.rs` | Rust Code | framework/scheduler/tests | Key functions: next, below, engine, stress_mixed_kinds_5000_jobs_no_leaks |
| `framework/security/src/auth.rs` | Rust Code | framework/security/src | Key functions: owned_heap, value_heap |
| `framework/security/src/constant_time.rs` | Rust Code | framework/security/src | Key functions: constant_time_eq, test_constant_time |
| `framework/security/src/cookie.rs` | Rust Code | framework/security/src | Key functions: fmt |
| `framework/security/src/csp.rs` | Rust Code | framework/security/src | Key functions: fmt |
| `framework/security/src/csrf.rs` | Rust Code | framework/security/src | Key functions: as_str |
| `framework/security/src/jwt.rs` | Rust Code | framework/security/src | Key functions: default, new, verify_signature |
| `framework/security/src/lib.rs` | Rust Code | framework/security/src | Source executable |
| `framework/security/src/smuggling.rs` | Rust Code | framework/security/src | Key functions: audit_request_desync |
| `framework/security/tests/secret_rendering.rs` | Rust Code | framework/security/tests | Key functions: the_csrf_token_renders_a_placeholder, a_refusal_carries_no_credential |
| `framework/tests/examples/h2_handframed_cap.py` | Python Script | framework/tests/examples | Python script or module |
| `framework/tests/examples/h2_interop_server.rs` | Rust Code | framework/tests/examples | Key functions: pki |
| `framework/tests/examples/openloop_loadgen.rs` | Rust Code | framework/tests/examples | Source executable |
| `framework/tests/examples/pgo_train.rs` | Rust Code | framework/tests/examples | Key functions: main |
| `framework/tests/src/lib.rs` | Rust Code | framework/tests/src | Key functions: test_error_codes_and_diagnostics, test_server_banner |
| `framework/tests/tests/bearer_auth.rs` | Rust Code | framework/tests/tests | Key functions: mint, bearer_token, every_invalid_credential_gets_the_same_401_and_the_route_never_runs |
| `framework/tests/tests/csrf.rs` | Rust Code | framework/tests/tests | Key functions: tokens, cookies, issued, planted |
| `framework/tests/tests/error_reporting.rs` | Rust Code | framework/tests/tests | Key functions: seen, error_status, redacted_detail, error_status |
| `framework/tests/tests/extensions_e2e.rs` | Rust Code | framework/tests/tests | Key functions: marked_app, an_extension_set_by_middleware_reaches_the_handler, two_middlewares_share_one_map_and_both_values_survive |
| `framework/tests/tests/extractors.rs` | Rust Code | framework/tests/tests | Key functions: extractor_app, path_primitive, path_string |
| `framework/tests/tests/facade_wiring.rs` | Rust Code | framework/tests/tests | Key functions: headers, a_jwt_verifies_through_the_facade, constant_time_comparison_is_reachable_through_the_facade, the_desync_audit_rejects_conflicting_framing_through_the_facade |
| `framework/tests/tests/handler_exec_semantics.rs` | Rust Code | framework/tests/tests | Key functions: start, signal_shutdown, finish_shutdown, stop |
| `framework/tests/tests/http2_e2e.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, pki |
| `framework/tests/tests/http_conformance.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, header, body_text |
| `framework/tests/tests/http_e2e.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, header, json |
| `framework/tests/tests/nested_routers.rs` | Rust Code | framework/tests/tests | Key functions: temp_dir, a_mounted_router_serves_under_its_prefix_and_not_without_it, a_mounted_router_state_answers_before_the_app_state |
| `framework/tests/tests/openapi_derivation.rs` | Rust Code | framework/tests/tests | Key functions: operations, parameter_names, status_codes, the_document_covers_every_route_the_app_registered |
| `framework/tests/tests/response_exit.rs` | Rust Code | framework/tests/tests | Key functions: error_status, error_detail, read_head, an_error_status_outside_the_error_range_answers_500 |
| `framework/tests/tests/scheduling.rs` | Rust Code | framework/tests/tests | Key functions: start, stop |
| `framework/tests/tests/signed_cookies.rs` | Rust Code | framework/tests/tests | Key functions: signer, classify, app, issued |
| `framework/tests/tests/streaming.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, header |
| `framework/tests/tests/streaming_request.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, header, json |
| `framework/tests/tests/test_client.rs` | Rust Code | framework/tests/tests | Key functions: echo_app, get_text_roundtrip, url_points_at_bound_port, path_params_and_decoded_query |
| `framework/tests/tests/tls_e2e.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, pki |
| `framework/tests/tests/user_middleware.rs` | Rust Code | framework/tests/tests | Key functions: middleware_modifies_response, middleware_runs_in_registration_order, middleware_can_short_circuit, middleware_applies_only_to_routes_registered_after |
| `framework/validation/src/lib.rs` | Rust Code | framework/validation/src | Source executable |
| `framework/validation/src/simd.rs` | Rust Code | framework/validation/src | Key functions: validate_ascii_fast, validate_email_format |
| `framework/validation/src/validator.rs` | Rust Code | framework/validation/src | Key functions: validate, try_new, as_str, try_new |
| `framework/websocket/src/connection.rs` | Rust Code | framework/websocket/src | Key functions: default |
| `framework/websocket/src/handshake.rs` | Rust Code | framework/websocket/src | Key functions: fmt, validate_handshake |
| `framework/websocket/src/lib.rs` | Rust Code | framework/websocket/src | Key functions: parse_frame |

## 📂 Governance Directory Structure
| File | Role |
|---|---|
| requirements.json | Active tasks - deleted upon completion |
| problems.json | Active bugs or obstacles |
| proposals.json | Proposed additions requiring approval |
| criticisms.json | Architecture or code quality concerns |
| PROJECT_MAP.md | Generated repository file index |
| OWNER_VISION.md | Read-only owner intent and constraints |
| SYSTEM_PROMPT.md | Project-independent AI instructions |