//! # Bounded RFC 6455 connection driver
//!
//! Reads masked client frames, assembles fragmented messages, answers pings, and performs
//! the closing handshake. Every read is bounded before the payload is buffered, so a frame
//! header declaring a terabyte cannot make the server allocate it. Every write is bounded by
//! the peer's consumption, so a peer that stops reading cannot park a session in a socket that
//! will not take bytes.

use aether_core::error::AetherError;
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

use crate::{WsOpcode, parse_client_frame};

/// The reason carried by the close frame a server sends when it is stopping. RFC 6455 §7.4.1.1
/// defines status 1001 for exactly this: the endpoint is going away.
const GOING_AWAY_REASON: &str = "server shutting down";

/// A complete application message. Control frames are handled by the driver and never
/// surfaced here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

/// Resource bounds for a connection.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketLimits {
    /// Maximum size of a reassembled message.
    pub max_message_bytes: usize,
    /// Maximum size of a single frame payload. A frame larger than this is rejected as soon
    /// as its header arrives, before the payload is read.
    pub max_frame_bytes: usize,
    /// Inactivity policy for `recv`: the maximum gap between raw bytes arriving from the
    /// peer before the connection is failed with [`WsError::IdleTimeout`]. Any frame counts
    /// (including pings), so a heartbeat satisfies it. `None` disables the timeout.
    pub read_timeout: Option<std::time::Duration>,
    /// Inactivity policy for sending: the maximum time the peer may leave one outgoing frame
    /// with no bytes consumed before the write fails with [`WsError::WriteTimeout`]. Like
    /// [`Self::read_timeout`] this bounds an interval, not a frame: a peer that keeps consuming
    /// may take any total time for a large frame. `None` disables the timeout.
    pub write_timeout: Option<std::time::Duration>,
}

impl Default for WebSocketLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 16 * 1024 * 1024,
            max_frame_bytes: 16 * 1024 * 1024,
            // A silent peer for two minutes is treated as gone: idle sockets otherwise
            // pin a task, a connection slot, and their read buffer indefinitely.
            read_timeout: Some(std::time::Duration::from_secs(120)),
            // Ten seconds of zero consumption is the same peer-dead signal on the way out, and
            // matches `ServerLimits::write_timeout`. It cannot be folded into `read_timeout`: a
            // peer that pings while its receive window stays shut satisfies the read deadline and
            // starves the write, and the read deadline only ever bounds a wait inside `recv`.
            write_timeout: Some(std::time::Duration::from_secs(10)),
        }
    }
}

#[derive(Debug)]
pub enum WsError {
    Io(std::io::Error),
    Protocol(&'static str),
    MessageTooLarge,
    InvalidUtf8,
    /// A send was attempted after a close frame went out.
    Closed,
    /// The peer closed the TCP stream without a WebSocket close frame.
    UnexpectedEof,
    /// No bytes arrived from the peer within `WebSocketLimits::read_timeout`.
    IdleTimeout,
    /// The peer consumed no bytes of an outgoing frame for `WebSocketLimits::write_timeout`.
    /// The frame this error interrupted is only partly on the wire, so the connection's framing
    /// is no longer trustworthy and the session must end; the transport is not closed here
    /// because a close frame would be written into the same stalled socket.
    WriteTimeout,
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "WebSocket I/O error: {err}"),
            Self::Protocol(msg) => write!(f, "WebSocket protocol error: {msg}"),
            Self::MessageTooLarge => f.write_str("WebSocket message exceeds the configured limit"),
            Self::InvalidUtf8 => f.write_str("WebSocket text message is not valid UTF-8"),
            Self::Closed => f.write_str("WebSocket close frame already sent"),
            Self::UnexpectedEof => f.write_str("WebSocket stream ended without a close frame"),
            Self::IdleTimeout => {
                f.write_str("WebSocket peer was idle past the configured read timeout")
            }
            Self::WriteTimeout => f.write_str(
                "WebSocket peer consumed no frame bytes past the configured write timeout",
            ),
        }
    }
}

impl std::error::Error for WsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WsError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Server side of an established WebSocket connection.
///
/// The generic parameter lets the driver run over a `TcpStream`, a TLS stream, or an
/// in-memory transport in tests. Outgoing frames are never masked, as required of a server
/// by RFC 6455 §5.1.
pub struct WebSocket<S> {
    io: S,
    buffer: Vec<u8>,
    limits: WebSocketLimits,
    fragments: Vec<u8>,
    fragment_opcode: Option<WsOpcode>,
    close_sent: bool,
    shutdown: Option<watch::Receiver<bool>>,
}

impl<S> WebSocket<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wraps an already-upgraded stream. `initial` is any bytes that were read past the
    /// opening HTTP request (the first WebSocket frames can arrive in the same TCP segment).
    pub fn from_parts(io: S, initial: Vec<u8>, limits: WebSocketLimits) -> Self {
        Self {
            io,
            buffer: initial,
            limits,
            fragments: Vec::new(),
            fragment_opcode: None,
            close_sent: false,
            shutdown: None,
        }
    }

    /// Wraps an upgraded stream whose server is listening on `shutdown`.
    ///
    /// When the flag flips, [`Self::recv`] stops waiting for the peer and ends the session on
    /// the server's initiative: it sends one close frame with status 1001 ("going away",
    /// RFC 6455 §7.4.1.1) and shuts the write half down, so a peer learns the server is
    /// stopping instead of reading a bare TCP FIN as an abnormal closure. A socket built with
    /// [`Self::from_parts`] has no such signal and closes only on the peer's terms.
    pub fn from_parts_with_shutdown(
        io: S,
        initial: Vec<u8>,
        limits: WebSocketLimits,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            shutdown: Some(shutdown),
            ..Self::from_parts(io, initial, limits)
        }
    }

    pub fn limits(&self) -> WebSocketLimits {
        self.limits
    }

    /// Receives the next application message. Returns `Ok(None)` after the closing
    /// handshake; the caller should then drop the connection.
    ///
    /// Pings are answered and pongs are consumed internally, so a caller blocked in `recv`
    /// is still responsive to control traffic.
    ///
    /// A socket built with [`Self::from_parts_with_shutdown`] also returns `Ok(None)` when its
    /// server asks the session to stop, after sending the 1001 close frame itself.
    pub async fn recv(&mut self) -> Result<Option<Message>, WsError> {
        enum Wait {
            Read(Result<(), WsError>),
            GoingAway,
        }

        loop {
            let parsed = match parse_client_frame(&self.buffer) {
                Ok(result) => result,
                Err(err) => {
                    let mapped = map_parse_error(err);
                    return Err(self.fail(mapped).await);
                }
            };

            let Some((frame, consumed)) = parsed else {
                if let Err(err) = self.check_buffered_header() {
                    return Err(self.fail(err).await);
                }
                // The inactivity policy bounds each network wait; buffered frame bytes
                // already in `self.buffer` make progress without touching the socket.
                let wait = {
                    let Self {
                        io,
                        buffer,
                        limits,
                        shutdown,
                        ..
                    } = self;
                    let read_timeout = limits.read_timeout;
                    let read = async {
                        match read_timeout {
                            Some(limit) => tokio::time::timeout(limit, read_more(io, buffer))
                                .await
                                .map_err(|_| WsError::IdleTimeout)?,
                            None => read_more(io, buffer).await,
                        }
                    };
                    match shutdown.as_mut() {
                        Some(signal) => tokio::select! {
                            biased;
                            _ = shutdown_requested(signal) => Wait::GoingAway,
                            result = read => Wait::Read(result),
                        },
                        None => Wait::Read(read.await),
                    }
                };
                match wait {
                    Wait::Read(Ok(())) => continue,
                    Wait::Read(Err(err)) => return Err(err),
                    Wait::GoingAway => return self.going_away().await,
                }
            };

            self.buffer.drain(..consumed);

            // `check_buffered_header` enforces `max_frame_bytes` before each socket read,
            // but a complete frame that was already sitting in the bytes read past the HTTP
            // upgrade never passes through that gate. Re-check the decoded frame so the
            // per-frame limit cannot be skipped by pipelining the first frame with the
            // handshake.
            if frame.payload.len() > self.limits.max_frame_bytes {
                return Err(self.fail(WsError::MessageTooLarge).await);
            }

            match frame.opcode {
                WsOpcode::Ping => {
                    self.write_frame(WsOpcode::Pong, &frame.payload, true)
                        .await?;
                }
                WsOpcode::Pong => {}
                WsOpcode::Close => {
                    if let Err(err) = validate_close_payload(&frame.payload) {
                        return Err(self.fail(err).await);
                    }
                    if !self.close_sent {
                        self.write_frame(WsOpcode::Close, &frame.payload, true)
                            .await?;
                        self.close_sent = true;
                    }
                    return Ok(None);
                }
                WsOpcode::Text | WsOpcode::Binary | WsOpcode::Continuation => {
                    match self.accept_data_frame(frame) {
                        Ok(Some(message)) => return Ok(Some(message)),
                        Ok(None) => {}
                        Err(err) => return Err(self.fail(err).await),
                    }
                }
            }
        }
    }

    pub async fn send(&mut self, message: Message) -> Result<(), WsError> {
        match message {
            Message::Text(text) => self.send_text(&text).await,
            Message::Binary(bytes) => self.send_binary(&bytes).await,
        }
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        self.ensure_open(text.len())?;
        self.write_frame(WsOpcode::Text, text.as_bytes(), true)
            .await
    }

    pub async fn send_binary(&mut self, payload: &[u8]) -> Result<(), WsError> {
        self.ensure_open(payload.len())?;
        self.write_frame(WsOpcode::Binary, payload, true).await
    }

    /// Sends an unsolicited ping. The peer's pong is consumed by [`Self::recv`].
    ///
    /// Refused once the closing handshake has started: RFC 6455 §7.1.1 stops data and
    /// control transmission after we send Close, and a ping issued then is either
    /// discarded by the peer or answered by a frame arriving after the session ended.
    pub async fn ping(&mut self, payload: &[u8]) -> Result<(), WsError> {
        if self.close_sent {
            return Err(WsError::Closed);
        }
        if payload.len() > 125 {
            return Err(WsError::Protocol("control frame payload exceeds 125 bytes"));
        }
        self.write_frame(WsOpcode::Ping, payload, true).await
    }

    /// Starts the closing handshake. Idempotent: a second call does nothing.
    ///
    /// The caller still needs to call [`Self::recv`] (or simply drop the connection) to
    /// receive the peer's close frame; servers may close the TCP stream after a short wait.
    pub async fn close(&mut self, code: u16, reason: &str) -> Result<(), WsError> {
        if self.close_sent {
            return Ok(());
        }
        if !valid_close_code(code) {
            return Err(WsError::Protocol("invalid close status code"));
        }
        if reason.len() > 123 {
            return Err(WsError::Protocol("close reason exceeds 123 bytes"));
        }
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        self.write_frame(WsOpcode::Close, &payload, true).await?;
        self.close_sent = true;
        Ok(())
    }

    /// Returns the underlying transport after the session ends.
    pub fn into_inner(self) -> S {
        self.io
    }

    /// Ends the session because the server is going away.
    ///
    /// Both steps are best-effort and both happen before returning: the close frame is the
    /// peer's notice, and shutting the write half down afterwards keeps that frame from being
    /// discarded by an RST that a bare close can earn while the receive buffer still holds
    /// unread bytes — the same reason every HTTP path in the server shuts the transport down
    /// before returning. `Ok(None)` is the same "session over" the closing handshake reports.
    async fn going_away(&mut self) -> Result<Option<Message>, WsError> {
        let _ = self.close(1001, GOING_AWAY_REASON).await;
        let _ = self.io.shutdown().await;
        Ok(None)
    }

    fn ensure_open(&self, payload_len: usize) -> Result<(), WsError> {
        if self.close_sent {
            return Err(WsError::Closed);
        }
        if payload_len > self.limits.max_message_bytes {
            return Err(WsError::MessageTooLarge);
        }
        Ok(())
    }

    fn accept_data_frame(&mut self, frame: crate::ClientFrame) -> Result<Option<Message>, WsError> {
        match frame.opcode {
            WsOpcode::Text | WsOpcode::Binary => {
                if self.fragment_opcode.is_some() {
                    return Err(WsError::Protocol(
                        "new data frame while a fragmented message is in progress",
                    ));
                }
                if frame.fin {
                    return self.complete_message(frame.opcode, frame.payload);
                }
                if frame.payload.len() > self.limits.max_message_bytes {
                    return Err(WsError::MessageTooLarge);
                }
                self.fragment_opcode = Some(frame.opcode);
                self.fragments = frame.payload;
                Ok(None)
            }
            WsOpcode::Continuation => {
                let Some(opcode) = self.fragment_opcode else {
                    return Err(WsError::Protocol(
                        "continuation frame without a start frame",
                    ));
                };
                if self.fragments.len() + frame.payload.len() > self.limits.max_message_bytes {
                    return Err(WsError::MessageTooLarge);
                }
                self.fragments.extend_from_slice(&frame.payload);
                if frame.fin {
                    self.fragment_opcode = None;
                    let payload = std::mem::take(&mut self.fragments);
                    self.complete_message(opcode, payload)
                } else {
                    Ok(None)
                }
            }
            _ => Err(WsError::Protocol("unexpected frame opcode")),
        }
    }

    fn complete_message(
        &mut self,
        opcode: WsOpcode,
        payload: Vec<u8>,
    ) -> Result<Option<Message>, WsError> {
        if payload.len() > self.limits.max_message_bytes {
            return Err(WsError::MessageTooLarge);
        }
        match opcode {
            WsOpcode::Text => match String::from_utf8(payload) {
                Ok(text) => Ok(Some(Message::Text(text))),
                Err(_) => Err(WsError::InvalidUtf8),
            },
            WsOpcode::Binary => Ok(Some(Message::Binary(payload))),
            _ => Err(WsError::Protocol("unexpected message opcode")),
        }
    }

    /// Rejects a frame whose declared size is over the limit before its payload is read,
    /// and refuses unmasked client frames as soon as the second byte arrives.
    fn check_buffered_header(&self) -> Result<(), WsError> {
        if self.buffer.len() >= 2 && self.buffer[1] & 0x80 == 0 {
            return Err(WsError::Protocol("client frames must be masked"));
        }
        let Some(payload_len) = declared_payload_len(&self.buffer) else {
            return Ok(());
        };
        if payload_len > self.limits.max_frame_bytes {
            return Err(WsError::MessageTooLarge);
        }
        Ok(())
    }

    /// Writes one server frame, unmasked as RFC 6455 §5.1 requires of a server.
    ///
    /// With `WebSocketLimits::write_timeout` set, the frame reaches the transport in pieces and
    /// every wait for socket space is bounded. A peer that stops reading then costs the session one
    /// interval instead of its lifetime: before this bound the wait belonged to the socket, and the
    /// parked task held its connection slot and its buffer registration while `read_timeout` —
    /// which only bounds a wait inside [`Self::recv`] — could not reach it.
    ///
    /// An interrupted write leaves a partial frame on the wire, so a [`WsError::WriteTimeout`]
    /// invalidates the connection: the caller ends the session instead of retrying.
    async fn write_frame(
        &mut self,
        opcode: WsOpcode,
        payload: &[u8],
        fin: bool,
    ) -> Result<(), WsError> {
        let is_control = matches!(opcode, WsOpcode::Close | WsOpcode::Ping | WsOpcode::Pong);
        if is_control && payload.len() > 125 {
            return Err(WsError::Protocol("control frame payload exceeds 125 bytes"));
        }

        let mut header = [0u8; 10];
        header[0] = (if fin { 0x80 } else { 0 }) | opcode as u8;
        let header_len = if payload.len() <= 125 {
            header[1] = payload.len() as u8;
            2
        } else if payload.len() <= u16::MAX as usize {
            header[1] = 126;
            header[2..4].copy_from_slice(&(payload.len() as u16).to_be_bytes());
            4
        } else {
            header[1] = 127;
            header[2..10].copy_from_slice(&(payload.len() as u64).to_be_bytes());
            10
        };

        match self.limits.write_timeout {
            None => {
                self.io.write_all(&header[..header_len]).await?;
                self.io.write_all(payload).await?;
                self.io.flush().await?;
            }
            Some(limit) => {
                // Header, payload and flush draw on one budget: the peer may take any total time
                // over a frame, as long as every interval it is given pays for the next one.
                let mut budget = WriteBudget::new(limit);
                let Self { io, .. } = self;
                write_bounded(io, &header[..header_len], &mut budget).await?;
                write_bounded(io, payload, &mut budget).await?;
                flush_bounded(io, &mut budget).await?;
            }
        }
        Ok(())
    }

    /// Best-effort protocol failure notification. The connection is unusable afterwards
    /// regardless, so a failed close write does not replace the original error.
    async fn fail(&mut self, err: WsError) -> WsError {
        if !self.close_sent {
            let (code, reason) = match &err {
                WsError::MessageTooLarge => (1009u16, "message too large"),
                WsError::InvalidUtf8 => (1007, "invalid UTF-8"),
                WsError::Protocol(_) => (1002, "protocol error"),
                _ => (1011, "internal error"),
            };
            let _ = self.close(code, reason).await;
        }
        err
    }
}

/// One bounded read into `buffer`. Zero bytes is the peer ending the stream without a close
/// frame; the inactivity deadline is applied by the caller so this stays a plain read.
async fn read_more<S>(io: &mut S, buffer: &mut Vec<u8>) -> Result<(), WsError>
where
    S: AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    let read = io.read(&mut chunk).await?;
    if read == 0 {
        return Err(WsError::UnexpectedEof);
    }
    buffer.extend_from_slice(&chunk[..read]);
    Ok(())
}

/// Resolves once the server has asked the session to stop.
///
/// `changed()` alone is not enough: a receiver cloned after the flag flipped inherits the seen
/// version and would wait for a change that already happened, which is exactly the shutdown
/// that races an upgrade. Checking the current value first covers that. A dropped sender means
/// no one can ever ask again — the server that owned the session is gone — so it resolves too
/// instead of parking the session forever.
async fn shutdown_requested(signal: &mut watch::Receiver<bool>) {
    loop {
        if *signal.borrow_and_update() {
            return;
        }
        if signal.changed().await.is_err() {
            return;
        }
    }
}

/// The least payload one write interval must move to renew its deadline.
///
/// Renewal on *any* progress makes a session's lifetime unbounded for a peer that grants one
/// window update per interval — the shape PROB-0022 settled for HTTP/2 — so the same timer doubles
/// as a floor on how slowly a peer may consume: `MIN_PROGRESS_FOR_RENEWAL / write_timeout` of
/// payload. Requiring it is what bounds a stalled peer; deliberately *not* putting an absolute
/// per-frame cap alongside it is what keeps a large frame deliverable to a slow but reading one.
/// The figure matches the HTTP/2 frontend so one timeout means one rate floor across both emitters.
const MIN_PROGRESS_FOR_RENEWAL: usize = 1024;

/// One frame's share of the peer's write budget: the interval bound and the progress tally that
/// decides whether a wait earns the next one.
struct WriteBudget {
    limit: std::time::Duration,
    deadline: tokio::time::Instant,
    earned: usize,
}

impl WriteBudget {
    fn new(limit: std::time::Duration) -> Self {
        Self {
            limit,
            deadline: tokio::time::Instant::now() + limit,
            earned: 0,
        }
    }

    /// Charges a completed write. Moving bytes is necessary for a renewal but not sufficient:
    /// credit accumulated across waits counts toward the minimum, which then restarts the tally
    /// and the clock with it.
    fn record(&mut self, written: usize) {
        self.earned += written;
        if self.earned >= MIN_PROGRESS_FOR_RENEWAL {
            self.earned = 0;
            self.deadline = tokio::time::Instant::now() + self.limit;
        }
    }
}

/// Hand `buf` to the transport, failing the frame if the budget runs out before it does.
async fn write_bounded<S>(io: &mut S, buf: &[u8], budget: &mut WriteBudget) -> Result<(), WsError>
where
    S: AsyncWrite + Unpin,
{
    let mut offset = 0usize;
    while offset < buf.len() {
        let written = tokio::time::timeout_at(budget.deadline, io.write(&buf[offset..]))
            .await
            .map_err(|_| WsError::WriteTimeout)??;
        if written == 0 {
            return Err(WsError::Io(std::io::Error::from(
                std::io::ErrorKind::WriteZero,
            )));
        }
        offset += written;
        budget.record(written);
    }
    Ok(())
}

/// The same bound on the trailing flush, which a buffering transport (TLS) can make a real wait.
async fn flush_bounded<S>(io: &mut S, budget: &mut WriteBudget) -> Result<(), WsError>
where
    S: AsyncWrite + Unpin,
{
    tokio::time::timeout_at(budget.deadline, io.flush())
        .await
        .map_err(|_| WsError::WriteTimeout)?
        .map_err(WsError::Io)
}

fn map_parse_error(err: AetherError) -> WsError {
    match err {
        AetherError::BodyLimitExceeded => WsError::MessageTooLarge,
        _ => WsError::Protocol("malformed WebSocket frame"),
    }
}

fn declared_payload_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 2 {
        return None;
    }
    match buf[1] & 0x7f {
        len @ 0..=125 => Some(len as usize),
        126 => {
            if buf.len() < 4 {
                None
            } else {
                Some(u16::from_be_bytes([buf[2], buf[3]]) as usize)
            }
        }
        _ => {
            if buf.len() < 10 {
                return None;
            }
            let len = u64::from_be_bytes(buf[2..10].try_into().expect("10 bytes"));
            Some(usize::try_from(len).unwrap_or(usize::MAX))
        }
    }
}

fn validate_close_payload(payload: &[u8]) -> Result<(), WsError> {
    match payload.len() {
        0 => Ok(()),
        1 => Err(WsError::Protocol("close frame with a 1-byte payload")),
        _ => {
            let code = u16::from_be_bytes([payload[0], payload[1]]);
            if !valid_close_code(code) {
                return Err(WsError::Protocol("invalid close status code"));
            }
            std::str::from_utf8(&payload[2..])
                .map(|_| ())
                .map_err(|_| WsError::InvalidUtf8)
        }
    }
}

fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn client_frame(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let len = payload.len();
        if len <= 125 {
            out.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ mask[i & 3]),
        );
        out
    }

    async fn read_frame(peer: &mut tokio::io::DuplexStream) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        peer.read_exact(&mut head).await.unwrap();
        let opcode = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        assert!(!masked, "server frames must not be masked");
        let len = match head[1] & 0x7f {
            126 => {
                let mut ext = [0u8; 2];
                peer.read_exact(&mut ext).await.unwrap();
                u16::from_be_bytes(ext) as usize
            }
            127 => {
                let mut ext = [0u8; 8];
                peer.read_exact(&mut ext).await.unwrap();
                u64::from_be_bytes(ext) as usize
            }
            n => n as usize,
        };
        let mut payload = vec![0u8; len];
        peer.read_exact(&mut payload).await.unwrap();
        (opcode, payload)
    }

    #[tokio::test]
    async fn reassembles_fragments_and_answers_ping_in_between() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(false, 0x1, b"hel", [1, 2, 3, 4]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x9, b"p", [5, 6, 7, 8]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x0, b"lo", [9, 1, 2, 3]))
            .await
            .unwrap();

        let message = ws.recv().await.unwrap();
        assert_eq!(message, Some(Message::Text("hello".to_string())));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0xA, "ping must be answered with pong");
        assert_eq!(payload, b"p");
    }

    #[tokio::test]
    async fn rejects_unmasked_client_frame_with_protocol_close() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&[0x81, 0x05]).await.unwrap();
        peer.write_all(b"hello").await.unwrap();

        let err = ws.recv().await.unwrap_err();
        assert!(matches!(err, WsError::Protocol(_)));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1002);
    }

    #[tokio::test]
    async fn close_is_echoed_and_ends_the_stream() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        let mut payload = 1000u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"bye");
        peer.write_all(&client_frame(true, 0x8, &payload, [4, 3, 2, 1]))
            .await
            .unwrap();

        assert_eq!(ws.recv().await.unwrap(), None);
        let (opcode, echoed) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(echoed, payload);
    }

    #[tokio::test]
    async fn rejects_text_with_invalid_utf8() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(true, 0x1, &[0xff, 0xfe], [7, 7, 7, 7]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::InvalidUtf8)));
        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1007);
    }

    #[tokio::test]
    async fn continuation_without_start_is_rejected() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        peer.write_all(&client_frame(true, 0x0, b"orphan", [1, 1, 1, 1]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::Protocol(_))));
    }

    #[tokio::test]
    async fn declared_oversized_frame_is_rejected_before_payload_arrives() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let limits = WebSocketLimits {
            max_message_bytes: 64,
            max_frame_bytes: 64,
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        // Header alone declares a 70-byte masked payload; the payload is never sent.
        let mut header = vec![0x82, 0x80 | 70];
        header.extend_from_slice(&[1, 2, 3, 4]);
        peer.write_all(&header).await.unwrap();

        let err = tokio::time::timeout(std::time::Duration::from_secs(1), ws.recv())
            .await
            .expect("must not wait for the payload");
        assert!(matches!(err, Err(WsError::MessageTooLarge)));
    }

    #[tokio::test]
    async fn silent_peer_is_failed_by_the_idle_deadline() {
        let (server, _peer) = tokio::io::duplex(4096);
        let limits = WebSocketLimits {
            read_timeout: Some(std::time::Duration::from_millis(40)),
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        // `_peer` is kept alive so the read stays pending rather than reaching EOF.
        assert!(matches!(ws.recv().await, Err(WsError::IdleTimeout)));
    }

    #[tokio::test]
    async fn a_ping_resets_the_idle_deadline() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let window = std::time::Duration::from_millis(40);
        let limits = WebSocketLimits {
            read_timeout: Some(window),
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        peer.write_all(&client_frame(true, 0x9, b"h", [1, 2, 3, 4]))
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        assert!(matches!(ws.recv().await, Err(WsError::IdleTimeout)));
        assert!(
            started.elapsed() >= window,
            "a heartbeat must not end the session; only the following silent window may"
        );

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0xA, "ping must be answered with pong");
        assert_eq!(payload, b"h");
    }

    #[tokio::test]
    async fn reassembled_message_limit_is_enforced() {
        let (server, mut peer) = tokio::io::duplex(8192);
        let limits = WebSocketLimits {
            max_message_bytes: 8,
            max_frame_bytes: 64,
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        peer.write_all(&client_frame(false, 0x2, b"12345", [1, 2, 3, 4]))
            .await
            .unwrap();
        peer.write_all(&client_frame(true, 0x0, b"6789", [5, 6, 7, 8]))
            .await
            .unwrap();

        assert!(matches!(ws.recv().await, Err(WsError::MessageTooLarge)));
    }

    #[tokio::test]
    async fn server_output_is_unmasked_and_round_trips() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        ws.send_text("hello").await.unwrap();
        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x1);
        assert_eq!(payload, b"hello");

        peer.write_all(&client_frame(true, 0x1, b"world", [9, 9, 9, 9]))
            .await
            .unwrap();
        assert_eq!(
            ws.recv().await.unwrap(),
            Some(Message::Text("world".to_string()))
        );
    }

    #[tokio::test]
    async fn initial_bytes_from_the_upgrade_request_are_not_lost() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let initial = client_frame(true, 0x1, b"early", [4, 4, 4, 4]);
        let mut ws = WebSocket::from_parts(server, initial, WebSocketLimits::default());

        assert_eq!(
            ws.recv().await.unwrap(),
            Some(Message::Text("early".to_string()))
        );
        let _ = &mut peer;
    }

    #[tokio::test]
    async fn close_code_and_payload_rules() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        assert!(matches!(
            ws.close(1005, "").await,
            Err(WsError::Protocol(_))
        ));
        ws.close(1000, "done").await.unwrap();
        assert!(matches!(ws.send_text("late").await, Err(WsError::Closed)));

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1000);
        assert_eq!(&payload[2..], b"done");
    }

    /// RFC 6455 §7.1.1: once the closing handshake has started the session stops
    /// transmitting. `send_text` already refused, but `ping` skipped that check and would
    /// emit a frame after our Close, which the peer either discards or answers late.
    #[tokio::test]
    async fn ping_after_close_is_refused_and_nothing_reaches_the_wire() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let mut ws = WebSocket::from_parts(server, Vec::new(), WebSocketLimits::default());

        ws.close(1000, "done").await.unwrap();
        let (opcode, _) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);

        assert!(matches!(
            ws.ping(b"are you there").await,
            Err(WsError::Closed)
        ));

        // The refusal must be total: not one byte after the close frame.
        let mut stray = [0u8; 16];
        let quiet =
            tokio::time::timeout(std::time::Duration::from_millis(50), peer.read(&mut stray)).await;
        assert!(
            quiet.is_err(),
            "a frame was written after Close: {:?}",
            stray
        );
    }

    /// A server that is stopping must not leave a live peer with a bare FIN. RFC 6455 §7.4.1.1
    /// defines 1001 for exactly this, and the session ends on the close so the handler's loop
    /// returns instead of being killed mid-await.
    #[tokio::test]
    async fn a_server_shutdown_closes_a_waiting_session_with_going_away() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut ws = WebSocket::from_parts_with_shutdown(
            server,
            Vec::new(),
            WebSocketLimits::default(),
            shutdown_rx,
        );

        shutdown_tx.send(true).unwrap();
        assert_eq!(ws.recv().await.unwrap(), None);

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1001);
        assert_eq!(&payload[2..], b"server shutting down");

        // Nothing after the close frame: the write half is already shut down, so the peer
        // reads EOF rather than a stray frame. The bound is what makes a missing shutdown a
        // failure instead of a hang.
        let mut tail = [0u8; 8];
        let tail = tokio::time::timeout(std::time::Duration::from_secs(1), peer.read(&mut tail))
            .await
            .expect("the write half was never shut down: the peer's read is still pending");
        assert_eq!(tail.unwrap(), 0);
    }

    /// A shutdown that starts before the session does is still a shutdown: the receiver given
    /// to the socket was cloned after the flag had flipped, and `changed()` alone would wait
    /// for a change that already happened.
    #[tokio::test]
    async fn a_shutdown_that_started_before_the_upgrade_still_closes_the_session() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let mut ws = WebSocket::from_parts_with_shutdown(
            server,
            Vec::new(),
            WebSocketLimits::default(),
            shutdown_rx,
        );

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), ws.recv())
            .await
            .expect("the already-set flag must be seen, not waited on");
        assert_eq!(outcome.unwrap(), None);

        let (opcode, payload) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);
        assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1001);
    }

    #[tokio::test]
    async fn a_session_closed_by_shutdown_refuses_further_sends() {
        let (server, mut peer) = tokio::io::duplex(4096);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut ws = WebSocket::from_parts_with_shutdown(
            server,
            Vec::new(),
            WebSocketLimits::default(),
            shutdown_rx,
        );

        shutdown_tx.send(true).unwrap();
        assert_eq!(ws.recv().await.unwrap(), None);
        let (opcode, _) = read_frame(&mut peer).await;
        assert_eq!(opcode, 0x8);

        assert!(matches!(ws.send_text("late").await, Err(WsError::Closed)));
    }

    /// PROB-0030: `recv` bounded its reads and nothing bounded a write, so a peer that stopped
    /// taking bytes parked the session for as long as it liked — and `read_timeout` could not reach
    /// it, because a task sitting in a write never returns to the wait that deadline bounds. The
    /// outer `timeout` is the reproduction: on the tree before this bound the send does not come
    /// back at all, so the test reports the park rather than hanging the suite.
    #[tokio::test]
    async fn a_peer_that_stops_consuming_costs_the_session_one_write_interval() {
        let (server, _peer) = tokio::io::duplex(8);
        let window = std::time::Duration::from_millis(40);
        let limits = WebSocketLimits {
            read_timeout: Some(std::time::Duration::from_secs(30)),
            write_timeout: Some(window),
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        let started = tokio::time::Instant::now();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ws.send_text(&"x".repeat(4096)),
        )
        .await;
        let elapsed = started.elapsed();

        let err = match outcome {
            Err(_) => panic!("the send never returned: this write still has no deadline"),
            Ok(Ok(())) => panic!("a 4 KiB frame left the socket a peer never read from"),
            Ok(Err(err)) => err,
        };
        assert!(matches!(err, WsError::WriteTimeout), "got {err:?}");
        assert!(
            elapsed >= window,
            "the peer was not even given its interval: {elapsed:?}"
        );
        assert!(
            elapsed < window * 20,
            "the stalled write outlived its interval: {elapsed:?}"
        );
    }

    /// The bound is an interval, not a frame. A peer that consumes well above the renewal floor may
    /// take any total time: this frame needs some fifteen drains and about twice the deadline to
    /// deliver, and none of those waits may cut it off. An absolute per-frame cap fails here —
    /// which is the other half of the trade `MIN_PROGRESS_FOR_RENEWAL` makes, and the reason the
    /// bound is a rate floor rather than a size limit.
    #[tokio::test]
    async fn a_peer_that_reads_slowly_gets_the_whole_frame() {
        let (server, mut peer) = tokio::io::duplex(16 * 1024);
        let window = std::time::Duration::from_millis(100);
        let limits = WebSocketLimits {
            read_timeout: None,
            write_timeout: Some(window),
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        let payload = "x".repeat(60 * 1024);
        let body_len = payload.len();
        let frame_len = body_len + 4;
        let started = tokio::time::Instant::now();
        let sender = tokio::spawn(async move { ws.send_text(&payload).await });

        // Drain a 4 KiB slice every seventh of an interval: every gap between renewals is far
        // inside the deadline, while the frame as a whole takes twice the deadline to deliver.
        let mut received: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        while received.len() < frame_len {
            tokio::time::sleep(window / 7).await;
            let n = peer.read(&mut chunk).await.expect("peer read");
            if n == 0 {
                // The writer gave up and dropped the session. Break rather than spin: a detector
                // that hangs is worth less than one that fails, and the assertion below says why.
                break;
            }
            received.extend_from_slice(&chunk[..n]);
        }
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), sender)
            .await
            .expect("the send task never finished")
            .expect("send task panicked");
        let elapsed = started.elapsed();

        assert!(
            matches!(outcome, Ok(())),
            "a reading peer was cut off: {outcome:?}"
        );
        assert!(
            elapsed > window,
            "the frame finished inside one interval, so this never tested a total time: {elapsed:?}"
        );
        assert_eq!(received[0], 0x81, "fin + text opcode");
        assert_eq!(received[1], 126, "16-bit extended length");
        assert_eq!(&received[2..4], &(body_len as u16).to_be_bytes());
        assert_eq!(received.len(), frame_len);
    }

    /// The floor has to be paid, not merely touched. A peer that hands back one byte per interval
    /// renews a deadline that renews on any progress, and buys itself a session that never ends —
    /// the shape PROB-0022 settled for HTTP/2. So the trickle must still be running when the writer
    /// is judged: four intervals of one byte every fifth of an interval, raced against the send, and
    /// the send has to win by failing. A test that drained and then waited would let an
    /// any-progress renewal look correct, because the writer is only stalled after the peer stops.
    #[tokio::test]
    async fn a_peer_that_trickles_one_byte_per_interval_is_shed() {
        let (server, mut peer) = tokio::io::duplex(8);
        let window = std::time::Duration::from_millis(100);
        let limits = WebSocketLimits {
            read_timeout: None,
            write_timeout: Some(window),
            ..WebSocketLimits::default()
        };
        let mut ws = WebSocket::from_parts(server, Vec::new(), limits);

        let payload = "x".repeat(60 * 1024);
        let mut sender = tokio::spawn(async move { ws.send_text(&payload).await });

        let trickle = async {
            let mut byte = [0u8; 1];
            for _ in 0..20 {
                tokio::time::sleep(window / 5).await;
                if peer.read(&mut byte).await.unwrap_or(0) == 0 {
                    break;
                }
            }
        };

        let sent = tokio::select! {
            joined = &mut sender => joined.expect("send task panicked"),
            _ = trickle => panic!(
                "a peer moving one byte per interval kept the write alive through four intervals; \
                 renewal is being bought with any progress instead of the floor"
            ),
        };

        assert!(
            matches!(sent, Err(WsError::WriteTimeout)),
            "the trickle peer was not shed on the write floor: {sent:?}"
        );
    }
}
