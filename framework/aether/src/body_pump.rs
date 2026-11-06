//! The HTTP/1.1 pump behind a streamed request body.
//!
//! The connection task owns the socket, so while a handler runs it cannot be read by the loop.
//! This type is the loop's stand-in: dispatch polls it alongside its "has the peer gone?" arm, and
//! every poll does at most one chunk of work — hand the lane a chunk if one is buffered, read the
//! socket if none is, and stop the moment the lane is full.
//!
//! Three properties keep the memory bound honest:
//!
//! - The socket is read only when the lane is empty, so a handler that stops reading stops the
//!   read. The client's own window, not a queue here, is the backpressure.
//! - Wire bytes leave `conn_buf` as soon as the decoder has validated them, so the buffer holds a
//!   window, never a body.
//! - `conn_buf`, the lane slot and the chunked decoder's accumulator are re-priced against the
//!   connection's registration after every step, exactly as the buffered path prices `conn_buf`.
//!
//! A body that ends before the framework's limits say it should is a *failure*, not a short read:
//! the response that follows is the last thing this connection may carry, because the bytes that
//! would arrive next cannot be told apart from a new request.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use tokio::io::{AsyncRead, ReadBuf};

use aether_http::limits::ServerLimits;
use aether_http::parser::{BodyFraming, ChunkedBodyDecoder};

use crate::app::{Reprice, conn_buffer_charge, reprice_conn_memory};
use crate::request_body::{BodyError, BodyWriter};
use crate::serve::{PeerWatch, ReadAheadWatch};
use crate::transport::Stream;

/// One read of a streamed body. Larger than the loop's 8 KiB because each read here costs a poll
/// of the dispatch select, and the lane holds the result only until the handler takes it.
const BODY_READ_CHUNK: usize = 64 * 1024;

pub(crate) struct Http1Body<'a> {
    stream: &'a mut Stream,
    conn_buf: &'a mut BytesMut,
    writer: BodyWriter,
    framing: BodyFraming,
    /// Payload bytes still owed by a `Content-Length` body.
    remaining: usize,
    chunked: ChunkedBodyDecoder,
    /// Reused between decodes; `stream_feed` appends into it.
    decoded: Vec<u8>,
    /// Payload bytes currently in `decoded` after the last decode.
    received: u64,
    /// `max_stream_body_bytes`: the ceiling on the *whole* body, not on one chunk.
    cap: u64,
    limits: &'a ServerLimits,
    scratch: Box<[u8]>,
    deadline: Instant,
    finished: bool,
    /// It reached that state on its own terms: the framing says the body is complete. A failed
    /// body is `finished` but not `ended_clean`, and the connection cannot be reused over a tail
    /// the framing never accounted for.
    ended_clean: bool,
    /// The peer's socket failed while the body was arriving: dispatch cancels the handler.
    gone: bool,
    /// Set once the read-ahead arm has seen a final state, so it stops polling the socket.
    peek_settled: bool,
    guard: &'a Option<crate::scheduling::AdmissionGuard>,
    charged: &'a mut u64,
}

impl<'a> Http1Body<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "the pump is the frontend's state: socket, buffer, lane, framing, limits, clock, budget"
    )]
    pub(crate) fn new(
        stream: &'a mut Stream,
        conn_buf: &'a mut BytesMut,
        writer: BodyWriter,
        framing: BodyFraming,
        limits: &'a ServerLimits,
        deadline: Instant,
        guard: &'a Option<crate::scheduling::AdmissionGuard>,
        charged: &'a mut u64,
    ) -> Self {
        let remaining = match framing {
            BodyFraming::ContentLength(len) => len,
            _ => 0,
        };
        Self {
            stream,
            conn_buf,
            writer,
            framing,
            remaining,
            chunked: ChunkedBodyDecoder::new(),
            decoded: Vec::new(),
            received: 0,
            cap: limits.max_stream_body_bytes as u64,
            limits,
            scratch: vec![0u8; BODY_READ_CHUNK].into_boxed_slice(),
            deadline,
            finished: false,
            ended_clean: false,
            gone: false,
            peek_settled: false,
            guard,
            charged,
        }
    }

    /// After the response: finish the body without touching the socket, if what is already
    /// buffered is enough. `true` means the connection is framed and may be reused.
    ///
    /// The pump never reads ahead of the handler, so this only succeeds when the peer's whole
    /// remaining body arrived in the same reads the handler's own chunks came from — common for
    /// a small body and an early answer, impossible for a large one. It is an optimisation of the
    /// close rule, not a second rule: what it cannot finish here closes.
    pub(crate) fn settle(&mut self) -> bool {
        if self.finished {
            // A body that failed never finished its framing: its unread tail is still in the
            // socket, and reading that as the next request is request smuggling. Only a body
            // that ended on its own terms leaves the connection reusable.
            return self.ended_clean && !self.gone;
        }
        loop {
            match self.framing {
                BodyFraming::None => {
                    self.finish_clean();
                    return true;
                }
                BodyFraming::ContentLength(_) => {
                    let take = self.conn_buf.len().min(self.remaining);
                    if take == 0 && self.remaining > 0 {
                        return false;
                    }
                    let _ = self.conn_buf.split_to(take);
                    self.remaining -= take;
                    self.received += take as u64;
                    if self.remaining == 0 {
                        self.finish_clean();
                        return true;
                    }
                }
                BodyFraming::Chunked => {
                    if self.conn_buf.is_empty() {
                        return self.chunked.is_complete() && {
                            self.finish_clean();
                            true
                        };
                    }
                    let before = self.conn_buf.len();
                    self.decoded.clear();
                    match self
                        .chunked
                        .stream_feed(self.conn_buf, self.limits, &mut self.decoded)
                    {
                        Ok(consumed) => {
                            if consumed > 0 {
                                self.conn_buf.advance(consumed);
                            }
                        }
                        Err(_) => {
                            self.fail(BodyError::Malformed);
                            return false;
                        }
                    }
                    if self.chunked.is_complete() {
                        self.finish_clean();
                        return true;
                    }
                    if self.conn_buf.len() == before {
                        // Not enough buffered bytes to finish the framing; the socket is the only
                        // place the rest could come from, and this path does not read it.
                        return false;
                    }
                }
            }
        }
    }

    /// One step of the pump. `Ready` means the body reached its terminal state.
    fn step(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.finished {
            return Poll::Ready(());
        }
        loop {
            if self.writer.poll_room(cx).is_pending() {
                return Poll::Pending;
            }

            let progressed = match self.framing {
                BodyFraming::None => {
                    self.finish_clean();
                    return Poll::Ready(());
                }
                BodyFraming::ContentLength(_) => {
                    if self.conn_buf.is_empty() {
                        false
                    } else {
                        let take = self.conn_buf.len().min(self.remaining);
                        let chunk = self.conn_buf.split_to(take).freeze();
                        self.remaining -= take;
                        self.received += take as u64;
                        self.writer.push(chunk);
                        if !self.check_budget() {
                            self.fail(BodyError::TooLarge);
                            return Poll::Ready(());
                        }
                        if self.remaining == 0 {
                            self.finish_clean();
                            return Poll::Ready(());
                        }
                        true
                    }
                }
                BodyFraming::Chunked => {
                    if self.conn_buf.is_empty() {
                        false
                    } else {
                        let before = self.conn_buf.len();
                        self.decoded.clear();
                        match self.chunked.stream_feed(
                            self.conn_buf,
                            self.limits,
                            &mut self.decoded,
                        ) {
                            Ok(consumed) => {
                                if consumed > 0 {
                                    self.conn_buf.advance(consumed);
                                }
                            }
                            Err(_) => {
                                self.fail(BodyError::Malformed);
                                return Poll::Ready(());
                            }
                        }
                        let delivered = !self.decoded.is_empty();
                        if delivered {
                            let len = self.decoded.len() as u64;
                            if self.received.saturating_add(len) > self.cap {
                                self.fail(BodyError::TooLarge);
                                return Poll::Ready(());
                            }
                            self.received += len;
                            let chunk = Bytes::copy_from_slice(&self.decoded);
                            self.writer.push(chunk);
                            if !self.check_budget() {
                                self.fail(BodyError::TooLarge);
                                return Poll::Ready(());
                            }
                        }
                        if self.chunked.is_complete() {
                            self.finish_clean();
                            return Poll::Ready(());
                        }
                        delivered || self.conn_buf.len() < before
                    }
                }
            };

            if progressed {
                continue;
            }

            // Nothing buffered: the next byte can only come from the socket.
            if Instant::now() >= self.deadline {
                self.fail(BodyError::Timeout);
                return Poll::Ready(());
            }
            let mut read = ReadBuf::new(&mut self.scratch[..]);
            match Pin::new(&mut *self.stream).poll_read(cx, &mut read) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => {
                    // The socket is dead; the handler has nobody left to answer.
                    self.gone = true;
                    self.fail(BodyError::Aborted);
                    return Poll::Ready(());
                }
                Poll::Ready(Ok(())) => {
                    let filled = read.filled().len();
                    if filled == 0 {
                        // EOF before the body finished framing it: the peer stopped mid-body.
                        self.fail(BodyError::Malformed);
                        return Poll::Ready(());
                    }
                    self.conn_buf.extend_from_slice(read.filled());
                }
            }
            if !self.check_budget() {
                self.fail(BodyError::TooLarge);
                return Poll::Ready(());
            }
        }
    }

    /// Keep the connection's registration equal to what this stream now holds.
    fn check_budget(&mut self) -> bool {
        let buffered = conn_buffer_charge(self.conn_buf)
            .saturating_add(self.writer.held() as u64)
            .saturating_add(match self.framing {
                BodyFraming::Chunked => self.chunked.body_capacity_bytes() as u64,
                _ => 0,
            });
        match reprice_conn_memory(self.guard, *self.charged, buffered, 0) {
            Reprice::Updated(value) => {
                *self.charged = value;
                true
            }
            Reprice::Unchanged => true,
            Reprice::OverBudget => false,
        }
    }

    fn finish_clean(&mut self) {
        if !self.finished {
            self.finished = true;
            self.ended_clean = true;
            self.writer.finish();
        }
    }

    fn fail(&mut self, error: BodyError) {
        if !self.finished {
            self.finished = true;
            self.writer.fail(error);
        }
    }
}

impl PeerWatch for Http1Body<'_> {
    fn poll_gone(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.gone {
            return Poll::Ready(());
        }
        if !self.finished {
            // The pump is the reader while the body arrives; polling the socket from here as well
            // would race it for the body's own bytes. A failed read inside the pump sets `gone`.
            return Poll::Pending;
        }
        if self.peek_settled {
            return Poll::Pending;
        }
        // The body is over, so the socket is back to what the buffered path does between requests:
        // a read-ahead peek that separates "the peer left" from "the peer will send again".
        let mut peek = ReadAheadWatch::on(&mut *self.stream, &mut *self.conn_buf);
        let polled = peek.poll_gone(cx);
        if peek.is_settled() {
            self.peek_settled = true;
            if polled.is_ready() {
                self.gone = true;
                return Poll::Ready(());
            }
        }
        Poll::Pending
    }

    fn poll_body(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.step(cx)
    }

    fn body_pending(&self) -> bool {
        !self.finished
    }

    fn body_deadline(&self) -> Option<Instant> {
        (!self.finished).then_some(self.deadline)
    }

    fn expire_body(&mut self) {
        self.fail(BodyError::Timeout);
    }
}
