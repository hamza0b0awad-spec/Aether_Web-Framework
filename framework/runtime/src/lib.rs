//! # Aether Runtime Crate
//!
//! Legacy prototype connection loop, kept only as a workspace member. Nothing in this
//! repository uses it: the production serve path lives in `aether::app` (`AetherApp::serve`),
//! which adds chunked-body decoding, Host enforcement, keep-alive/timeout handling,
//! backpressure-bounded writes, scheduler admission, and middleware. `handle_connection`
//! here is a byte-stream loop — it reassembles a request split across reads, serves every
//! pipelined request in one read, and answers a framing error `400`/`413`/`431` with
//! `Connection: close` — but it routes through a demo router of hardcoded handler ids, never
//! enforces `Host`, and applies no I/O timeouts. Do not build on it; prefer the
//! `aether` crate's `AetherApp`. Do not treat it as supported API.

pub mod worker;

pub use worker::handle_connection;
