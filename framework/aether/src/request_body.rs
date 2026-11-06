//! The handler half of a streamed request body.
//!
//! A route registered through [`AetherApp::route_stream`](crate::AetherApp::route_stream) is
//! dispatched as soon as its head is complete; the bytes arrive while the handler runs. This
//! module owns the one-slot lane between the frontend pump and the handler: the pump may hold at
//! most one chunk in it, so a handler that stops reading stops the socket read and the client's
//! own window becomes the backpressure.
//!
//! The lane is deliberately not a queue. A queue would let the frontend read ahead of the handler
//! and would make the server's memory a function of the client's send rate, which is the one thing
//! the connection budget exists to prevent.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use bytes::{Bytes, BytesMut};

/// Why a streamed request body ended before its last byte reached the handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// The body passed `max_stream_body_bytes`, or the connection's memory budget could not
    /// cover the next chunk. The handler should answer `413`.
    TooLarge,
    /// The `Transfer-Encoding: chunked` framing was invalid, or the body stopped short of the
    /// length its head declared. The connection is not reusable either way.
    Malformed,
    /// The body did not finish arriving inside `receive_timeout`. The handler should answer
    /// `408`.
    Timeout,
    /// The frontend stopped pumping (the connection task ended). Not observable in normal
    /// operation: the pump outlives the handler that owns this lane.
    Aborted,
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BodyError::TooLarge => "the request body exceeded its limit",
            BodyError::Malformed => "the request body framing was invalid",
            BodyError::Timeout => "the request body did not finish arriving in time",
            BodyError::Aborted => "the request body was abandoned",
        })
    }
}

impl std::error::Error for BodyError {}

/// The shared, one-slot lane between a frontend pump and a handler.
#[derive(Default)]
struct Lane {
    state: Mutex<LaneState>,
}

#[derive(Default)]
struct LaneState {
    /// The one chunk the pump may hold ahead of the handler.
    slot: Option<Bytes>,
    /// Set exactly once, when no further chunk can arrive: clean end or a failure.
    end: Option<Result<(), BodyError>>,
    reader: Option<Waker>,
    writer: Option<Waker>,
    /// Total payload the pump handed over, for diagnostics and tests.
    delivered: u64,
}

/// The handler's end of a streamed request body.
///
/// Not `Clone`: one request has one reader. Pull with [`Self::next_chunk`] or read everything with
/// [`Self::read_to_end`]; the body is the request's, so dropping this without reading it closes
/// the connection after the response (the unread bytes cannot be framed as the next request).
pub struct RequestBody {
    lane: Arc<Lane>,
    /// The terminal state has been reported. A second call answers `None` instead of parking.
    finished: bool,
}

impl fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl RequestBody {
    /// The next chunk, as it arrives. `None` is the end of the body; `Some(Err(..))` is a body
    /// that failed — the error is reported once and the body is over.
    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, BodyError>> {
        if self.finished {
            return None;
        }
        let next = std::future::poll_fn(|cx| self.poll_chunk(cx)).await;
        if next.is_none() || matches!(next, Some(Err(_))) {
            self.finished = true;
        }
        next
    }

    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, BodyError>>> {
        let mut state = self.lane.state.lock().unwrap();
        if let Some(chunk) = state.slot.take() {
            // The pump is waiting for exactly this: the slot is free again.
            if let Some(writer) = state.writer.take() {
                writer.wake();
            }
            return Poll::Ready(Some(Ok(chunk)));
        }
        // Reported, not consumed: the lane's terminal state outlives the report, so a writer
        // that reached it can never push again.
        if let Some(error) = state
            .end
            .as_ref()
            .and_then(|end| end.as_ref().err())
            .copied()
        {
            return Poll::Ready(Some(Err(error)));
        }
        if state.end.is_some() {
            return Poll::Ready(None);
        }
        state.reader = Some(cx.waker().clone());
        Poll::Pending
    }

    /// Read until the end of the body, failing with [`BodyError::TooLarge`] once more than `max`
    /// bytes have arrived.
    ///
    /// `max` is the handler's own cap, independent of the server's `max_stream_body_bytes`: a
    /// route that parses a document knows how large a document it will take, and refusing at that
    /// number stops the upload earlier than the server-wide ceiling would.
    pub async fn read_to_end(&mut self, max: usize) -> Result<Bytes, BodyError> {
        let mut out = BytesMut::new();
        while let Some(chunk) = self.next_chunk().await {
            let chunk = chunk?;
            if out.len() + chunk.len() > max {
                return Err(BodyError::TooLarge);
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out.freeze())
    }
}

/// The frontend's end of the lane. One per streamed request, owned by the pump.
pub(crate) struct BodyWriter {
    lane: Arc<Lane>,
    finished: bool,
}

impl BodyWriter {
    /// Poll until the lane has room for one more chunk.
    pub(crate) fn poll_room(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.lane.state.lock().unwrap();
        if state.slot.is_none() && state.end.is_none() {
            Poll::Ready(())
        } else {
            state.writer = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    /// Hand one chunk over. `false` means the slot is still full — the caller must not decode
    /// another chunk before [`Self::poll_room`] reports room.
    pub(crate) fn push(&mut self, chunk: Bytes) -> bool {
        let mut state = self.lane.state.lock().unwrap();
        if state.slot.is_some() || state.end.is_some() {
            return false;
        }
        state.delivered += chunk.len() as u64;
        state.slot = Some(chunk);
        if let Some(reader) = state.reader.take() {
            reader.wake();
        }
        true
    }

    /// The body ended on its own terms.
    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.end(Ok(()));
    }

    /// The body failed; the handler sees this error once and no more data.
    pub(crate) fn fail(&mut self, error: BodyError) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.end(Err(error));
    }

    fn end(&mut self, end: Result<(), BodyError>) {
        let mut state = self.lane.state.lock().unwrap();
        if state.end.is_none() {
            state.end = Some(end);
        }
        if let Some(reader) = state.reader.take() {
            reader.wake();
        }
        if let Some(writer) = state.writer.take() {
            writer.wake();
        }
    }

    /// Payload handed over so far. Read by the lane's own tests only.
    #[cfg(test)]
    pub(crate) fn delivered(&self) -> u64 {
        self.lane.state.lock().unwrap().delivered
    }

    /// Bytes the lane currently holds for the handler: exactly what the connection's memory
    /// registration must cover on top of its own buffers.
    pub(crate) fn held(&self) -> usize {
        self.lane
            .state
            .lock()
            .unwrap()
            .slot
            .as_ref()
            .map_or(0, |chunk| chunk.len())
    }
}

impl Drop for BodyWriter {
    fn drop(&mut self) {
        // A handler waiting on a lane whose pump is gone would wait for the handler deadline. The
        // pump outlives the handler in every frontend, so this only ever runs after the request is
        // over; the wake is for correctness, not for a case in the request path.
        self.fail(BodyError::Aborted);
    }
}

/// One lane, two ends: the handler's [`RequestBody`] and the pump's writer.
pub(crate) fn lane() -> (RequestBody, BodyWriter) {
    let lane = Arc::new(Lane::default());
    (
        RequestBody {
            lane: Arc::clone(&lane),
            finished: false,
        },
        BodyWriter {
            lane,
            finished: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_chunk_handed_over_before_the_handler_asks_is_delivered_once() {
        let (mut reader, mut writer) = lane();
        assert!(writer.push(Bytes::from_static(b"hello")));
        assert_eq!(writer.held(), 5);

        assert_eq!(
            reader.next_chunk().await,
            Some(Ok(Bytes::from_static(b"hello")))
        );
        assert_eq!(writer.held(), 0);

        writer.finish();
        assert_eq!(reader.next_chunk().await, None);
        // The terminal state is reported once, not every call.
        assert_eq!(reader.next_chunk().await, None);
    }

    #[tokio::test]
    async fn the_lane_holds_one_chunk_and_reopens_when_it_is_taken() {
        let (mut reader, mut writer) = lane();
        assert!(writer.push(Bytes::from_static(b"first")));
        // A second push while the slot is occupied is refused, not queued.
        assert!(!writer.push(Bytes::from_static(b"second")));
        assert_eq!(writer.delivered(), 5);

        assert_eq!(
            reader.next_chunk().await,
            Some(Ok(Bytes::from_static(b"first")))
        );
        assert!(writer.push(Bytes::from_static(b"second")));
        assert_eq!(
            reader.next_chunk().await,
            Some(Ok(Bytes::from_static(b"second")))
        );
        assert_eq!(writer.delivered(), 11);
    }

    #[tokio::test]
    async fn a_failure_is_reported_once_and_ends_the_body() {
        let (mut reader, mut writer) = lane();
        writer.fail(BodyError::Malformed);
        assert_eq!(reader.next_chunk().await, Some(Err(BodyError::Malformed)));
        assert_eq!(reader.next_chunk().await, None);
        // A writer that failed cannot start delivering afterwards.
        assert!(!writer.push(Bytes::from_static(b"late")));
    }

    #[tokio::test]
    async fn the_pump_cannot_run_ahead_of_the_reader() {
        let (mut reader, mut writer) = lane();
        writer.push(Bytes::from_static(b"0123456789"));
        writer.push(Bytes::from_static(b"x"));
        writer.finish();
        // The second push was refused while the slot was full, so only ten bytes were ever handed
        // over — which is the point: the pump cannot run ahead of the reader.
        assert_eq!(
            reader.read_to_end(10).await,
            Ok(Bytes::from_static(b"0123456789"))
        );
    }

    #[tokio::test]
    async fn read_to_end_refuses_a_body_past_the_cap() {
        let (mut reader, mut writer) = lane();
        writer.push(Bytes::from_static(b"0123456789A"));
        writer.finish();
        assert_eq!(reader.read_to_end(10).await, Err(BodyError::TooLarge));
    }
}
