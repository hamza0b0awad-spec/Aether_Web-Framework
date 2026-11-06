# Track 100 design note — a request body the handler reads while it arrives

Written before the code. Roadmap §19 row 100 names the constraint this note has to satisfy: *the
wire budget is the design constraint*. Every decision below is an answer to one question — how does
a body larger than the server's held memory stay inside the server's memory budget while the
handler is still reading it?

## What is being changed

Until this track the HTTP/1.1 loop did not dispatch a request until its whole body sat in
`conn_buf` (or in the chunked decoder's copy). That is correct and cheap for ordinary forms, and it
is the reason an upload costs its full length in resident memory, twice while a chunked body is
being decoded. Response streaming (track 99) already proved the pull-based shape on the other
direction; this track applies it to the request.

## The four decisions

**1. Opt-in per route, not automatic.** `ctx.body()` is synchronous and every existing handler
reads it. Switching every request to "dispatch at head, body arrives later" would leave those
handlers reading an empty `Bytes` with no compile error — the worst kind of change. So a route is
registered as streaming (`route_stream` / group equivalent) or it is not, and the frontend knows
before the body moves. A route that is not streaming keeps today's path byte for byte.

**2. Pull, one slot, no queue.** The handler holds a `RequestBody`; the frontend holds the other
half of a one-chunk lane. The pump reads the socket only when the lane is empty, so a handler that
stops reading stops the socket read: the client's own TCP window becomes the backpressure, not a
queue in the server. This is the same "the consumer decides" rule track 99 used for response
bodies, and it is why the lane is a single slot rather than an `mpsc` channel with a buffer.

**3. The memory bound is the existing connection registration, updated per chunk.** The live set
during a streamed upload is `conn_buf` (one read), the lane slot (one chunk), and the chunked
decoder's accumulator since its last drain — none of them proportional to the body. They are
re-priced through `reprice_conn_memory` as bytes move, exactly as the buffered path prices
`conn_buf`. A body is therefore bounded by *time* and by a declared cap (`max_stream_body_bytes`),
not by memory.

**4. A body the handler did not finish reading closes the connection.** Mid-request bytes on a
keep-alive socket are indistinguishable from the next request, and continuing to parse them is
request smuggling by construction. So: pump finished cleanly → the socket may be reused; pump
failed (cap, framing, timeout, peer gone) or was abandoned → the response is written, then FIN,
then a bounded discard of what the peer already queued. The discard is not optional on Windows: a
close over unread received bytes resets the connection and destroys the response the client is
still reading (measured 2026-10-26 for the buffer-budget refusal, recorded on
`refuse_over_budget`).

## Framing and limits

- `Content-Length` is a promise the pump enforces in both directions: it delivers exactly that many
  bytes and leaves anything after them in `conn_buf` for the next request. A peer that stops early
  is a failed body, not a short one.
- `chunked` uses the **same** decoder as the buffered path (`ChunkedBodyDecoder`), in a draining
  mode. A second chunk parser would be a second place for a smuggling disagreement to hide; the
  state machine stays single-sourced.
- Declared length over `max_stream_body_bytes` is refused with `413` before dispatch — the one
  refusal that can be made without reading anything.
- A chunked body is bounded by the same cap on cumulative payload, checked per chunk.
- The body must arrive inside `receive_timeout` counted from the head, not from the last byte: the
  handler deadline (`request_timeout`) bounds execution, and an upload is not execution.

## What is deliberately out of scope

- **HTTP/2 DATA frames.** The h2 frontend still collects the body before dispatch. It is tracked as
  row 100b rather than claimed here; the matrix row says `PARTIALLY_IMPLEMENTED` until it lands.
- **Extractors on a streaming route.** `Json<T>`/`Form<T>` read `ctx.body`, which is empty on a
  streamed request by design. A streaming route reads the body itself (`ctx.read_body(limit)`).
  The documentation states this; making an extractor stream would be a second body API.
- **`multipart/form-data` parsing.** Unchanged; out of this track's scope entirely.
- **TLS-specific behaviour.** The pump reads through `crate::transport::Stream`, so it is the same
  code path for plaintext and TLS; nothing here can tell them apart.
