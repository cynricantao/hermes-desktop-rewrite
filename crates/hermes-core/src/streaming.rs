//! Raw WebSocket streaming clients for the two audio sockets and the sandbox display socket.
//!
//! These routes carry binary frames, so neither [`crate::rpc`] (JSON-RPC text) nor
//! [`crate::http::Rest`] can express them. Everything here is UI-free and owns no background
//! task: callers drive reads and writes directly, and every operation is wrapped in the
//! configured timeout. There is no internal queue to grow without a bound — the only buffers
//! are the bounded WebSocket read/write buffers and the caller's own frames.
//!
//! # Wire protocols (derived from `hermes_cli/web_routers/audio.py` and `display.py`)
//!
//! * **`WS /api/audio/transcribe-stream?token=…&profile=…`** — the client sends
//!   `{"sample_rate": N}` (8000–48000) first, then binary s16le mono PCM chunks, then
//!   `{"eos": true}` exactly once. A disconnect before EOS cancels. The server replies with
//!   `{"type":"partial","text":…}`, `{"type":"final","transcript":…,"provider":…}` or
//!   `{"type":"error","message":…}` and closes. An `error` also means "no live STT", so the
//!   caller may fall back to the upload route.
//! * **`WS /api/audio/speak-stream?token=…&profile=…`** — the client sends `{"text": …}`
//!   deltas, one `{"done": true}` when the reply is complete, and optionally `{"stop": true}`
//!   for barge-in. The server sends `{"type":"start","sample_rate":…,"channels":1}` with the
//!   first PCM, then binary int16 PCM, then `{"type":"end"}`; `{"type":"fallback"}` means no
//!   audio was produced and the caller should use the whole-text `POST /api/audio/speak`.
//! * **`WS /api/display/ws?display_ticket=…`** — opaque raw RFB bytes in both directions.
//!   The ticket comes from `display.observe`, is single-use and expires after 30 s. A text
//!   frame from either side is a protocol error (`1003`). This module transports bytes; it
//!   does not implement RFB framing, decoding or rendering.
//!
//! # Authentication, URLs and redaction
//!
//! Voice URLs are built only from a [`crate::backend::Endpoint`] whose host is loopback and
//! whose token is non-empty; the scheme is always `ws://` over TCP. Display URLs are built
//! from the same endpoint plus the ticket returned by `display.observe`; the path is always
//! [`crate::api::display::DISPLAY_WS_PATH`], never caller-supplied. Profile names are
//! percent-encoded exactly like the REST client. [`StreamError`] never contains a URL, token
//! or ticket, and the clients' `Debug` output prints only `host:port` and state.
//!
//! # Explicit lifecycle
//!
//! Every client exposes [`close`](TranscribeStream::close) (best-effort close handshake) and
//! drops the socket with the value. TTS additionally exposes `stop` (barge-in signal) and
//! `cancel` (stop + close). Display reports close codes as [`DisplayClose`] and deliberately
//! does **not** reconnect or re-observe: after `4000`/`4401`/`4403` the caller decides whether
//! to call `display.observe` again. A dropped display link is never replayed.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify};
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::api::display::{DISPLAY_WS_PATH, DisplayObserve};
use crate::backend::{Endpoint, percent_encode};

/// Path of the live-dictation socket.
pub const TRANSCRIBE_WS_PATH: &str = "/api/audio/transcribe-stream";
/// Path of the streaming-TTS socket.
pub const SPEAK_WS_PATH: &str = "/api/audio/speak-stream";
/// Lowest accepted STT/PCM sample rate.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Highest accepted STT/PCM sample rate.
pub const MAX_SAMPLE_RATE: u32 = 48_000;
/// Lowest TTS output rate accepted in a `start` frame. The backend forwards the provider's
/// rate unclamped (`tts.openai.pcm_sample_rate`, response headers), so this range is wider
/// than the dictation range; resampling for the output device is the caller's job.
pub const MIN_TTS_SAMPLE_RATE: u32 = 1_000;
/// Highest TTS output rate accepted in a `start` frame.
pub const MAX_TTS_SAMPLE_RATE: u32 = 192_000;

const OPEN: u8 = 0;
const EOS_SENT: u8 = 1;
const DONE_SENT: u8 = 1;
const CANCELED: u8 = 2;

/// Timeouts and frame limits for a streaming socket. Sentence-based TTS providers
/// send an entire PCM sentence in one message, so defaults allow up to 16 MiB.
/// Callers may lower these limits for smaller display/audio buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    /// TCP connect plus WebSocket handshake budget.
    pub connect_timeout: Duration,
    /// Budget for one read (including waiting for a quiet screen or provider).
    pub read_timeout: Duration,
    /// Budget for one write/flush.
    pub write_timeout: Duration,
    /// Maximum inbound/outbound WebSocket message. Also set on the tungstenite reader, so an
    /// oversized message is rejected while reading instead of being buffered.
    pub max_message_bytes: usize,
    /// Maximum binary payload accepted from the server or sent by the caller (per frame).
    pub max_binary_frame_bytes: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            // A screen can be static for minutes and a provider can think before its first
            // chunk; callers that need faster failure pass their own budget.
            read_timeout: Duration::from_secs(300),
            write_timeout: Duration::from_secs(10),
            max_message_bytes: 16 * 1024 * 1024,
            max_binary_frame_bytes: 16 * 1024 * 1024,
        }
    }
}

impl StreamConfig {
    fn validate(self) -> Result<Self, StreamError> {
        if self.connect_timeout.is_zero() || self.read_timeout.is_zero() || self.write_timeout.is_zero() {
            return Err(StreamError::InvalidConfig("timeouts must be greater than zero"));
        }
        if self.max_message_bytes < 64 || self.max_message_bytes > 32 * 1024 * 1024 {
            return Err(StreamError::InvalidConfig(
                "max_message_bytes must be between 64 bytes and 32 MiB",
            ));
        }
        if self.max_binary_frame_bytes < 2 || self.max_binary_frame_bytes > self.max_message_bytes {
            return Err(StreamError::InvalidConfig(
                "max_binary_frame_bytes must be at least 2 and no larger than max_message_bytes",
            ));
        }
        Ok(self)
    }
}

/// Why a stream operation failed. Display close codes are values, not errors, when they arrive
/// in a Close frame ([`DisplayEvent::Closed`]); a handshake refusal is [`StreamError::Forbidden`]
/// because the server closes `4403` before accepting.
#[derive(Debug, Error)]
pub enum StreamError {
    #[error("stream endpoint must be a loopback TCP endpoint with a session token")]
    InvalidEndpoint,
    #[error("invalid stream configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid display ticket: {0}")]
    InvalidTicket(&'static str),
    #[error("unsupported sample rate {0}; expected 8000..=48000")]
    InvalidSampleRate(u32),
    #[error("invalid frame: {0}")]
    InvalidFrame(&'static str),
    #[error("invalid PCM: {0}")]
    InvalidPcm(&'static str),
    #[error("frame is {size} bytes, above the configured limit of {limit}")]
    FrameTooLarge { size: usize, limit: usize },
    #[error("websocket connect timed out")]
    ConnectTimeout,
    #[error("websocket read timed out")]
    ReadTimeout,
    #[error("websocket write timed out")]
    WriteTimeout,
    #[error("websocket writer is busy; no frame was submitted")]
    WriteBusy,
    #[error("websocket connect failed: {0}")]
    Connect(String),
    #[error("websocket authentication rejected the session")]
    Unauthorized,
    #[error("websocket policy refused the connection")]
    Forbidden,
    #[error("websocket closed unexpectedly (code {code})")]
    Closed { code: u16 },
    #[error("stream disconnected: {0}")]
    Disconnected(String),
    #[error("stream protocol error: {0}")]
    Protocol(String),
    #[error("stream state error: {0}")]
    State(&'static str),
}

impl StreamError {
    /// True for a non-clean close that means the caller must re-observe before retrying. This
    /// is information only: no client in this module retries by itself.
    pub fn requires_reobserve(&self) -> bool {
        matches!(self, StreamError::Unauthorized | StreamError::Forbidden)
    }
}

/// Typed server messages on the live-dictation socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscribeEvent {
    /// Live text while the user is still speaking.
    Partial { text: String },
    /// The recording was finalized.
    Final { transcript: String, provider: String },
    /// The server could not transcribe; fall back to the upload route if appropriate.
    Error { message: String },
}

/// Typed server messages on the streaming-TTS socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakEvent {
    /// PCM format; always sent before the first PCM chunk.
    Start { sample_rate: u32, channels: u16 },
    /// Raw s16le mono PCM.
    Pcm(Vec<u8>),
    /// Synthesis finished normally.
    End,
    /// Sentence synthesis produced no audio; POST the whole reply instead.
    Fallback,
    /// Server-side synthesis error.
    Error { message: String },
}

/// Display close codes. `4000`/`4001`/`4401`/`4403` are the backend's documented semantic
/// closes; no variant causes an automatic reconnect or observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayClose {
    /// `4000`: another human took over; this viewer was evicted.
    ControlTaken,
    /// `4001`: the profile's screen is not running.
    DesktopGone,
    /// `4401`: ticket missing, expired or already used.
    BadTicket,
    /// `4403`: host/origin/client-IP policy refused the connection (also seen as HTTP 403).
    NotAllowed,
    /// `1003`: a text frame or RFB parse error.
    ProtocolError,
    /// `1000`/`1001`/`1005`: an ordinary close.
    Normal,
    /// Any other close code.
    Other(u16),
}

impl DisplayClose {
    /// Classify a raw close code.
    pub fn from_code(code: u16) -> Self {
        match code {
            4000 => Self::ControlTaken,
            4001 => Self::DesktopGone,
            4401 => Self::BadTicket,
            4403 => Self::NotAllowed,
            1003 => Self::ProtocolError,
            1000 | 1001 | 1005 => Self::Normal,
            other => Self::Other(other),
        }
    }

    /// The canonical close code for this class. `Normal` always reports `1000`, even when the
    /// server sent `1001` or `1005`.
    pub fn code(self) -> u16 {
        match self {
            Self::ControlTaken => 4000,
            Self::DesktopGone => 4001,
            Self::BadTicket => 4401,
            Self::NotAllowed => 4403,
            Self::ProtocolError => 1003,
            Self::Normal => 1000,
            Self::Other(code) => code,
        }
    }

    /// True for an orderly close, including `1005` (no status present).
    pub fn is_clean(self) -> bool {
        matches!(self, Self::Normal)
    }

    /// True when the caller must call `display.observe` again before another viewer socket can
    /// be opened. This is advice only; [`DisplayStream`] never reacquires a ticket itself.
    pub fn requires_reobserve(self) -> bool {
        matches!(
            self,
            Self::ControlTaken | Self::DesktopGone | Self::BadTicket | Self::NotAllowed
        )
    }
}

/// One inbound display item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayEvent {
    /// Raw RFB bytes; this module does not decode them.
    Binary(Vec<u8>),
    /// The socket closed; the caller owns any re-observe/reconnect decision.
    Closed(DisplayClose),
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Shared, task-free connection. Reads and writes hold separate mutexes, so a display caller
/// may `select!` a read with a write (each future borrows `&self`) without deadlocking.
/// Dropping a pending read loses nothing: the message stays in the socket until a later
/// `recv` polls `Stream::next`, which is cancel-safe.
struct WsConnection {
    read: Mutex<SplitStream<Socket>>,
    write: Mutex<SplitSink<Socket, Message>>,
    config: StreamConfig,
    broken: AtomicBool,
    closed: AtomicBool,
    shutdown: Notify,
    secrets: Vec<String>,
}

impl WsConnection {
    async fn connect(url: &str, config: StreamConfig, secrets: Vec<String>) -> Result<Self, StreamError> {
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_message_bytes));
        let handshake = tokio_tungstenite::connect_async_with_config(url, Some(ws_config), true);
        let (socket, _response) = match tokio::time::timeout(config.connect_timeout, handshake).await {
            Err(_) => return Err(StreamError::ConnectTimeout),
            Ok(Err(err)) => return Err(classify_connect_error(err, &secrets)),
            Ok(Ok(pair)) => pair,
        };
        let (sink, stream) = socket.split();
        Ok(Self {
            read: Mutex::new(stream),
            write: Mutex::new(sink),
            config,
            broken: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            shutdown: Notify::new(),
            secrets,
        })
    }

    fn redact(&self, text: &str) -> String {
        redact_text(text, &self.secrets)
    }

    async fn send_text(&self, text: &str) -> Result<(), StreamError> {
        if text.len() > self.config.max_message_bytes {
            return Err(StreamError::FrameTooLarge {
                size: text.len(),
                limit: self.config.max_message_bytes,
            });
        }
        self.send_message(Message::Text(text.to_owned().into())).await
    }

    async fn send_json(&self, value: &Value) -> Result<(), StreamError> {
        self.send_text(&value.to_string()).await
    }

    async fn send_binary(&self, bytes: &[u8]) -> Result<(), StreamError> {
        self.send_message(Message::Binary(bytes.to_vec().into())).await
    }

    async fn send_message(&self, message: Message) -> Result<(), StreamError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(StreamError::State("connection is unusable after a failed write"));
        }
        // Contention is not an uncertain wire write: the caller's frame has not
        // reached the sink, and another operation must not be poisoned by it.
        let mut sink = tokio::time::timeout(self.config.write_timeout, self.write.lock())
            .await
            .map_err(|_| StreamError::WriteBusy)?;
        if self.broken.load(Ordering::SeqCst)
            || self.closed.load(Ordering::SeqCst) && !matches!(&message, Message::Close(_))
        {
            return Err(StreamError::State("connection is closed or unusable"));
        }
        // A dropped SinkExt::send may already have buffered bytes. Its outcome is
        // unknown; poison this connection and wake readers instead of replaying.
        let mut attempt = WriteAttempt {
            broken: &self.broken,
            shutdown: &self.shutdown,
            completed: false,
        };
        match tokio::time::timeout(self.config.write_timeout, sink.send(message)).await {
            Err(_) => Err(StreamError::WriteTimeout),
            Ok(Ok(())) => {
                attempt.completed = true;
                Ok(())
            }
            Ok(Err(err)) => Err(StreamError::Disconnected(self.redact(&err.to_string()))),
        }
    }

    /// `Ok(None)` is an orderly close (`ConnectionClosed` or an absent stream item after the
    /// close handshake). A raw EOF without a close frame is reported as a disconnect.
    async fn recv_message(&self) -> Result<Option<Message>, StreamError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(StreamError::State("connection is unusable after a failed write"));
        }
        let shutdown = self.shutdown.notified();
        tokio::pin!(shutdown);
        shutdown.as_mut().enable();
        // Register first, then recheck: a failure between the initial check and
        // notification registration must not leave this read asleep.
        if self.broken.load(Ordering::SeqCst) {
            return Err(StreamError::State("connection is unusable after a failed write"));
        }
        if self.closed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let read = async {
            let mut stream = self.read.lock().await;
            stream.next().await
        };
        let read = async {
            tokio::select! {
                biased;
                _ = &mut shutdown => Some(Ok(Message::Close(None))),
                value = read => value,
            }
        };
        match tokio::time::timeout(self.config.read_timeout, read).await {
            Err(_) => Err(StreamError::ReadTimeout),
            Ok(Some(Ok(message))) => {
                if self.broken.load(Ordering::SeqCst) {
                    return Err(StreamError::State(
                        "connection is unusable after a cancelled or failed write",
                    ));
                }
                if let Some(size) = message_payload_len(&message)
                    && size > self.config.max_message_bytes
                {
                    return Err(StreamError::FrameTooLarge {
                        size,
                        limit: self.config.max_message_bytes,
                    });
                }
                Ok(Some(message))
            }
            Ok(Some(Err(WsError::ConnectionClosed))) => Ok(None),
            Ok(Some(Err(err))) => Err(self.read_error(err)),
            Ok(None) => Err(StreamError::Disconnected("socket ended without a close frame".into())),
        }
    }

    fn read_error(&self, err: WsError) -> StreamError {
        match err {
            WsError::Capacity(_) => StreamError::Protocol(format!(
                "incoming message exceeds {} bytes",
                self.config.max_message_bytes
            )),
            WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
                StreamError::Disconnected("socket ended without a close frame".into())
            }
            WsError::Protocol(protocol) => StreamError::Protocol(self.redact(&protocol.to_string())),
            other => StreamError::Disconnected(self.redact(&other.to_string())),
        }
    }

    async fn close(&self) -> Result<(), StreamError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.shutdown.notify_waiters();
        // Best effort: a peer that already closed needs no reply, and closing must not turn a
        // completed session into an error.
        let _ = self
            .send_message(Message::Close(Some(CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "".into(),
            })))
            .await;
        Ok(())
    }
}

struct WriteAttempt<'a> {
    broken: &'a AtomicBool,
    shutdown: &'a Notify,
    completed: bool,
}
impl Drop for WriteAttempt<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.broken.store(true, Ordering::SeqCst);
            self.shutdown.notify_waiters();
        }
    }
}

/// Live-dictation client. Construction sends the mandatory sample-rate frame.
pub struct TranscribeStream {
    conn: WsConnection,
    outbound: Mutex<()>,
    state: AtomicU8,
    endpoint_label: String,
}

impl fmt::Debug for TranscribeStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscribeStream")
            .field("endpoint", &self.endpoint_label)
            .field("state", &transcribe_state_label(self.state.load(Ordering::SeqCst)))
            .finish()
    }
}

impl TranscribeStream {
    /// Dial `/api/audio/transcribe-stream` and send `{"sample_rate": N}`.
    ///
    /// `profile` is omitted when empty; otherwise it is percent-encoded into the query string.
    pub async fn connect(
        endpoint: &Endpoint,
        profile: &str,
        sample_rate: u32,
        config: StreamConfig,
    ) -> Result<Self, StreamError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(StreamError::InvalidSampleRate(sample_rate));
        }
        let config = config.validate()?;
        let url = voice_url(endpoint, TRANSCRIBE_WS_PATH, profile)?;
        let conn = WsConnection::connect(&url, config, vec![endpoint.token.clone()]).await?;
        conn.send_json(&json!({ "sample_rate": sample_rate })).await?;
        Ok(Self {
            conn,
            outbound: Mutex::new(()),
            state: AtomicU8::new(OPEN),
            endpoint_label: endpoint_label(endpoint),
        })
    }

    /// Send one binary PCM chunk. The chunk must be non-empty, even-length (s16le samples) and
    /// no larger than [`StreamConfig::max_binary_frame_bytes`].
    pub async fn send_pcm(&self, pcm: &[u8]) -> Result<(), StreamError> {
        let _outbound = self.outbound.lock().await;
        if self.state.load(Ordering::SeqCst) != OPEN {
            return Err(StreamError::State("PCM sent after eos or completion"));
        }
        validate_pcm(pcm, self.conn.config.max_binary_frame_bytes)?;
        self.conn.send_binary(pcm).await
    }

    /// Send the single end-of-stream marker. A second call is an error.
    pub async fn finish(&self) -> Result<(), StreamError> {
        let _outbound = self.outbound.lock().await;
        if self
            .state
            .compare_exchange(OPEN, EOS_SENT, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(StreamError::State("eos already sent"));
        }
        self.conn.send_json(&json!({ "eos": true })).await
    }

    /// Read the next typed server message. `Ok(None)` is an orderly server close.
    pub async fn recv(&self) -> Result<Option<TranscribeEvent>, StreamError> {
        loop {
            let message = match self.conn.recv_message().await? {
                Some(message) => message,
                None => return Ok(None),
            };
            match message {
                Message::Text(text) => {
                    let event = parse_transcribe_frame(&text, &self.conn)?;
                    if matches!(
                        event,
                        Some(TranscribeEvent::Final { .. } | TranscribeEvent::Error { .. })
                    ) {
                        self.state.store(CANCELED, Ordering::SeqCst);
                    }
                    return Ok(event);
                }
                Message::Binary(_) => {
                    return Err(StreamError::Protocol(
                        "binary frame on the transcribe event stream".into(),
                    ));
                }
                Message::Close(frame) => {
                    let code = close_code(frame.as_ref());
                    let _ = self.conn.close().await;
                    return voice_close(code);
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    /// Best-effort close handshake. Dropping the value closes the TCP socket.
    pub async fn close(&self) -> Result<(), StreamError> {
        self.conn.close().await
    }
}

/// Streaming-TTS client. Text deltas may be pushed while server PCM is read concurrently
/// (both futures borrow `&self`).
pub struct SpeakStream {
    conn: WsConnection,
    outbound: Mutex<()>,
    stopped: Notify,
    state: AtomicU8,
    started: AtomicBool,
    terminal: AtomicBool,
    endpoint_label: String,
}

impl fmt::Debug for SpeakStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeakStream")
            .field("endpoint", &self.endpoint_label)
            .field("state", &speak_state_label(self.state.load(Ordering::SeqCst)))
            .field("terminal", &self.terminal.load(Ordering::SeqCst))
            .finish()
    }
}

impl SpeakStream {
    /// Dial `/api/audio/speak-stream` with the same profile encoding as the REST client.
    pub async fn connect(endpoint: &Endpoint, profile: &str, config: StreamConfig) -> Result<Self, StreamError> {
        let config = config.validate()?;
        let url = voice_url(endpoint, SPEAK_WS_PATH, profile)?;
        let conn = WsConnection::connect(&url, config, vec![endpoint.token.clone()]).await?;
        Ok(Self {
            conn,
            outbound: Mutex::new(()),
            stopped: Notify::new(),
            state: AtomicU8::new(OPEN),
            started: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            endpoint_label: endpoint_label(endpoint),
        })
    }

    /// Send an incremental text delta. Empty deltas and deltas after `finish`/`stop` are errors.
    pub async fn push_text(&self, text: &str) -> Result<(), StreamError> {
        let _outbound = self.outbound.lock().await;
        if self.state.load(Ordering::SeqCst) != OPEN || self.terminal.load(Ordering::SeqCst) {
            return Err(StreamError::State("text sent after done or stop"));
        }
        if text.is_empty() {
            return Err(StreamError::InvalidFrame("empty text delta"));
        }
        if text.len() > self.conn.config.max_message_bytes {
            return Err(StreamError::FrameTooLarge {
                size: text.len(),
                limit: self.conn.config.max_message_bytes,
            });
        }
        self.conn.send_json(&json!({ "text": text })).await
    }

    /// Signal end-of-reply exactly once. PCM may still arrive afterwards.
    pub async fn finish(&self) -> Result<(), StreamError> {
        let _outbound = self.outbound.lock().await;
        if self
            .state
            .compare_exchange(OPEN, DONE_SENT, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(StreamError::State("done already sent or stream stopped"));
        }
        self.conn.send_json(&json!({ "done": true })).await
    }

    /// Barge-in: send `{"stop": true}` and suppress further inbound audio. The socket is left
    /// for [`close`](Self::close)/Drop so the caller can still drain the server's close.
    ///
    /// Inbound audio is suppressed before the frame is written, so barge-in never waits on a
    /// slow writer. If this future is dropped before it completes, the server may not have
    /// received the stop and a second `stop` is refused; call [`cancel`](Self::cancel) or drop
    /// the stream so the socket closes and synthesis ends.
    pub async fn stop(&self) -> Result<(), StreamError> {
        if self.state.swap(CANCELED, Ordering::SeqCst) == CANCELED {
            return Err(StreamError::State("stream already stopped"));
        }
        self.stopped.notify_waiters();
        let _outbound = self.outbound.lock().await;
        self.conn.send_json(&json!({ "stop": true })).await
    }

    /// Barge-in and close immediately. The close is attempted even when the stop frame could
    /// not be written; the stop error, if any, is returned afterwards.
    pub async fn cancel(&self) -> Result<(), StreamError> {
        let stopped = match self.stop().await {
            Err(StreamError::State(_)) => Ok(()),
            other => other,
        };
        let closed = self.conn.close().await;
        stopped.and(closed)
    }

    /// Read the next typed server event. `Ok(None)` follows `end`/`fallback`/`error`, a clean
    /// close, or a local `stop` (late PCM is discarded).
    pub async fn recv(&self) -> Result<Option<SpeakEvent>, StreamError> {
        if self.state.load(Ordering::SeqCst) == CANCELED || self.terminal.load(Ordering::SeqCst) {
            return Ok(None);
        }
        loop {
            let stopped = self.stopped.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if self.state.load(Ordering::SeqCst) == CANCELED {
                return Ok(None);
            }
            let message = tokio::select! {
                biased;
                _ = &mut stopped => return Ok(None),
                message = self.conn.recv_message() => message?,
            };
            if self.state.load(Ordering::SeqCst) == CANCELED {
                return Ok(None);
            }
            let Some(message) = message else {
                return Ok(None);
            };
            match message {
                Message::Text(text) => {
                    if let Some(event) = self.parse_frame(&text)? {
                        return Ok(Some(event));
                    }
                }
                Message::Binary(bytes) => return self.pcm(bytes.as_ref()).map(Some),
                Message::Close(frame) => {
                    let code = close_code(frame.as_ref());
                    let _ = self.conn.close().await;
                    return voice_close_speak(code);
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    fn pcm(&self, bytes: &[u8]) -> Result<SpeakEvent, StreamError> {
        if !self.started() {
            return Err(StreamError::Protocol("PCM before start metadata".into()));
        }
        validate_pcm(bytes, self.conn.config.max_binary_frame_bytes)?;
        Ok(SpeakEvent::Pcm(bytes.to_vec()))
    }

    fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    fn parse_frame(&self, text: &str) -> Result<Option<SpeakEvent>, StreamError> {
        #[derive(Deserialize)]
        struct Frame {
            #[serde(rename = "type")]
            kind: String,
            sample_rate: Option<u32>,
            channels: Option<u16>,
            message: Option<String>,
        }
        let frame: Frame = serde_json::from_str(text).map_err(|_| StreamError::Protocol("invalid TTS frame".into()))?;
        match frame.kind.as_str() {
            "start" => {
                let sample_rate = frame
                    .sample_rate
                    .ok_or_else(|| StreamError::Protocol("start frame without sample_rate".into()))?;
                if !(MIN_TTS_SAMPLE_RATE..=MAX_TTS_SAMPLE_RATE).contains(&sample_rate) {
                    return Err(StreamError::Protocol(format!(
                        "start sample_rate {sample_rate} outside {MIN_TTS_SAMPLE_RATE}..={MAX_TTS_SAMPLE_RATE}"
                    )));
                }
                if frame.channels != Some(1) {
                    return Err(StreamError::Protocol(
                        "start frame must declare exactly one channel".into(),
                    ));
                }
                if self.started() {
                    return Err(StreamError::Protocol("duplicate start frame".into()));
                }
                self.started.store(true, Ordering::SeqCst);
                Ok(Some(SpeakEvent::Start {
                    sample_rate,
                    channels: 1,
                }))
            }
            "end" => {
                if !self.started() {
                    return Err(StreamError::Protocol("end before start".into()));
                }
                self.terminal.store(true, Ordering::SeqCst);
                Ok(Some(SpeakEvent::End))
            }
            "fallback" => {
                self.terminal.store(true, Ordering::SeqCst);
                Ok(Some(SpeakEvent::Fallback))
            }
            "error" => {
                self.terminal.store(true, Ordering::SeqCst);
                Ok(Some(SpeakEvent::Error {
                    message: self
                        .conn
                        .redact(frame.message.as_deref().unwrap_or("speech synthesis failed")),
                }))
            }
            _ => Err(StreamError::Protocol("unexpected TTS frame type".into())),
        }
    }

    /// Best-effort close handshake. Dropping the value closes the TCP socket.
    pub async fn close(&self) -> Result<(), StreamError> {
        self.conn.close().await
    }
}

/// Raw-RFB display client. It transports bytes only; no RFB parsing or screen rendering is
/// implemented here.
pub struct DisplayStream {
    conn: WsConnection,
    endpoint_label: String,
    closed: AtomicBool,
}

impl fmt::Debug for DisplayStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DisplayStream")
            .field("endpoint", &self.endpoint_label)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish()
    }
}

impl DisplayStream {
    /// Dial the display socket using the single-use ticket from `display.observe`.
    ///
    /// The ticket's `path` is accepted only when it is empty or exactly
    /// [`DISPLAY_WS_PATH`]; the URL is always `ws://<loopback>:<port>/api/display/ws`.
    pub async fn connect_observed(
        endpoint: &Endpoint,
        observe: &DisplayObserve,
        config: StreamConfig,
    ) -> Result<Self, StreamError> {
        if !observe.path.is_empty() && observe.path != DISPLAY_WS_PATH {
            return Err(StreamError::InvalidTicket("unexpected display socket path"));
        }
        Self::connect_ticket(endpoint, &observe.ticket, config).await
    }

    /// Lower-level entry point for a ticket obtained from `display.observe`.
    pub async fn connect_ticket(endpoint: &Endpoint, ticket: &str, config: StreamConfig) -> Result<Self, StreamError> {
        if ticket.is_empty() {
            return Err(StreamError::InvalidTicket("empty display ticket"));
        }
        let config = config.validate()?;
        let url = display_url(endpoint, ticket)?;
        let conn = WsConnection::connect(&url, config, vec![ticket.to_owned()]).await?;
        Ok(Self {
            conn,
            endpoint_label: endpoint_label(endpoint),
            closed: AtomicBool::new(false),
        })
    }

    /// Send raw RFB bytes (keyboard/pointer/clipboard or handshake input). Text frames are
    /// never sent.
    pub async fn send_binary(&self, bytes: &[u8]) -> Result<(), StreamError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StreamError::State("display stream is closed"));
        }
        if bytes.is_empty() {
            return Err(StreamError::InvalidFrame("empty display frame"));
        }
        if bytes.len() > self.conn.config.max_binary_frame_bytes {
            return Err(StreamError::FrameTooLarge {
                size: bytes.len(),
                limit: self.conn.config.max_binary_frame_bytes,
            });
        }
        self.conn.send_binary(bytes).await
    }

    /// Read one raw RFB chunk or the typed close. There is no automatic reconnect, replay or
    /// ticket reacquisition; `4401`/`4403`/`4000` are returned to the caller.
    pub async fn recv(&self) -> Result<DisplayEvent, StreamError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StreamError::State("display stream is closed"));
        }
        loop {
            let message = match self.conn.recv_message().await? {
                Some(message) => message,
                None => {
                    self.closed.store(true, Ordering::SeqCst);
                    return Ok(DisplayEvent::Closed(DisplayClose::Normal));
                }
            };
            match message {
                Message::Binary(bytes) => {
                    if bytes.len() > self.conn.config.max_binary_frame_bytes {
                        self.closed.store(true, Ordering::SeqCst);
                        return Err(StreamError::FrameTooLarge {
                            size: bytes.len(),
                            limit: self.conn.config.max_binary_frame_bytes,
                        });
                    }
                    return Ok(DisplayEvent::Binary(bytes.to_vec()));
                }
                Message::Text(_) => {
                    self.closed.store(true, Ordering::SeqCst);
                    let _ = self.conn.close().await;
                    return Err(StreamError::Protocol("display stream is binary only".into()));
                }
                Message::Close(frame) => {
                    self.closed.store(true, Ordering::SeqCst);
                    let close = DisplayClose::from_code(close_code(frame.as_ref()));
                    let _ = self.conn.close().await;
                    return Ok(DisplayEvent::Closed(close));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    /// Best-effort close handshake. Dropping the value closes the TCP socket.
    pub async fn close(&self) -> Result<(), StreamError> {
        self.closed.store(true, Ordering::SeqCst);
        self.conn.close().await
    }
}

fn endpoint_label(endpoint: &Endpoint) -> String {
    if endpoint.host.contains(':') && endpoint.host != "::1" {
        endpoint.host.clone()
    } else if endpoint.host == "::1" {
        format!("[::1]:{}", endpoint.port)
    } else {
        format!("{}:{}", endpoint.host, endpoint.port)
    }
}

fn validate_local_endpoint(endpoint: &Endpoint, require_token: bool) -> Result<(), StreamError> {
    let loopback = matches!(endpoint.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if !loopback || endpoint.port == 0 || (require_token && endpoint.token.is_empty()) {
        return Err(StreamError::InvalidEndpoint);
    }
    Ok(())
}

fn ws_authority(endpoint: &Endpoint) -> String {
    if endpoint.host == "::1" {
        format!("[::1]:{}", endpoint.port)
    } else {
        format!("{}:{}", endpoint.host, endpoint.port)
    }
}

fn voice_url(endpoint: &Endpoint, path: &str, profile: &str) -> Result<String, StreamError> {
    validate_local_endpoint(endpoint, true)?;
    let mut url = format!(
        "ws://{}{}?token={}",
        ws_authority(endpoint),
        path,
        percent_encode(&endpoint.token)
    );
    if !profile.is_empty() {
        url.push_str("&profile=");
        url.push_str(&percent_encode(profile));
    }
    Ok(url)
}

fn display_url(endpoint: &Endpoint, ticket: &str) -> Result<String, StreamError> {
    validate_local_endpoint(endpoint, false)?;
    Ok(format!(
        "ws://{}{}?display_ticket={}",
        ws_authority(endpoint),
        DISPLAY_WS_PATH,
        percent_encode(ticket)
    ))
}

fn validate_pcm(bytes: &[u8], limit: usize) -> Result<(), StreamError> {
    if bytes.is_empty() {
        return Err(StreamError::InvalidPcm("empty PCM chunk"));
    }
    if !bytes.len().is_multiple_of(2) {
        return Err(StreamError::InvalidPcm("s16le chunk has an odd byte length"));
    }
    if bytes.len() > limit {
        return Err(StreamError::FrameTooLarge {
            size: bytes.len(),
            limit,
        });
    }
    Ok(())
}

fn parse_transcribe_frame(text: &str, conn: &WsConnection) -> Result<Option<TranscribeEvent>, StreamError> {
    #[derive(Deserialize)]
    struct Frame {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
        transcript: Option<String>,
        provider: Option<String>,
        message: Option<String>,
    }
    let frame: Frame =
        serde_json::from_str(text).map_err(|_| StreamError::Protocol("invalid transcribe frame".into()))?;
    match frame.kind.as_str() {
        "partial" => Ok(Some(TranscribeEvent::Partial {
            text: frame.text.unwrap_or_default(),
        })),
        "final" => Ok(Some(TranscribeEvent::Final {
            transcript: frame.transcript.unwrap_or_default(),
            provider: frame.provider.unwrap_or_default(),
        })),
        "error" => Ok(Some(TranscribeEvent::Error {
            message: conn.redact(frame.message.as_deref().unwrap_or("transcription failed")),
        })),
        _ => Err(StreamError::Protocol("unexpected transcribe frame type".into())),
    }
}

fn message_payload_len(message: &Message) -> Option<usize> {
    match message {
        Message::Text(text) => Some(text.len()),
        Message::Binary(bytes) => Some(bytes.len()),
        _ => None,
    }
}

fn close_code(frame: Option<&CloseFrame>) -> u16 {
    frame.map(|frame| u16::from(frame.code)).unwrap_or(1005)
}

fn voice_close(code: u16) -> Result<Option<TranscribeEvent>, StreamError> {
    match code {
        1000 | 1001 | 1005 => Ok(None),
        4401 => Err(StreamError::Unauthorized),
        4403 => Err(StreamError::Forbidden),
        other => Err(StreamError::Closed { code: other }),
    }
}

fn voice_close_speak(code: u16) -> Result<Option<SpeakEvent>, StreamError> {
    match code {
        1000 | 1001 | 1005 => Ok(None),
        4401 => Err(StreamError::Unauthorized),
        4403 => Err(StreamError::Forbidden),
        other => Err(StreamError::Closed { code: other }),
    }
}

fn transcribe_state_label(state: u8) -> &'static str {
    match state {
        OPEN => "open",
        EOS_SENT => "eos-sent",
        _ => "terminal",
    }
}

fn speak_state_label(state: u8) -> &'static str {
    match state {
        DONE_SENT => "done-sent",
        CANCELED => "canceled",
        _ => "open",
    }
}

fn classify_connect_error(err: WsError, secrets: &[String]) -> StreamError {
    match err {
        WsError::Http(response) => match response.status().as_u16() {
            401 => StreamError::Unauthorized,
            403 => StreamError::Forbidden,
            status => StreamError::Connect(format!("HTTP {status}")),
        },
        other => StreamError::Connect(redact_text(&other.to_string(), secrets)),
    }
}

fn redact_text(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        out = out.replace(secret, "<redacted>");
    }
    out = redact_query_value(&out, "token");
    out = redact_query_value(&out, "display_ticket");
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect::<String>();
        out.push('…');
    }
    out
}

fn redact_query_value(input: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find(&needle) {
        out.push_str(&rest[..index + needle.len()]);
        let after = &rest[index + needle.len()..];
        let end = after
            .find(|c: char| c == '&' || c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(after.len());
        out.push_str("<redacted>");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_unbounded_or_inverted_limits() {
        let base = StreamConfig::default();
        assert!(base.validate().is_ok());
        assert!(
            StreamConfig {
                max_message_bytes: 0,
                ..base
            }
            .validate()
            .is_err()
        );
        assert!(
            StreamConfig {
                max_binary_frame_bytes: base.max_message_bytes + 1,
                ..base
            }
            .validate()
            .is_err()
        );
        assert!(
            StreamConfig {
                read_timeout: Duration::ZERO,
                ..base
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn display_close_mapping_is_explicit() {
        assert_eq!(DisplayClose::from_code(4000), DisplayClose::ControlTaken);
        assert_eq!(DisplayClose::from_code(4001), DisplayClose::DesktopGone);
        assert_eq!(DisplayClose::from_code(4401), DisplayClose::BadTicket);
        assert_eq!(DisplayClose::from_code(4403), DisplayClose::NotAllowed);
        assert_eq!(DisplayClose::from_code(1003), DisplayClose::ProtocolError);
        assert_eq!(DisplayClose::from_code(1000), DisplayClose::Normal);
        assert_eq!(DisplayClose::from_code(4999), DisplayClose::Other(4999));
        assert_eq!(DisplayClose::BadTicket.code(), 4401);
    }

    #[test]
    fn url_building_is_loopback_only_and_encodes_profile() {
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: 41_000,
            token: "t+o/k".into(),
        };
        let url = voice_url(&endpoint, SPEAK_WS_PATH, "team a+b/ü").unwrap();
        assert_eq!(
            url,
            "ws://127.0.0.1:41000/api/audio/speak-stream?token=t%2Bo%2Fk&profile=team%20a%2Bb%2F%C3%BC"
        );
        assert!(
            voice_url(&endpoint, SPEAK_WS_PATH, "")
                .unwrap()
                .ends_with("?token=t%2Bo%2Fk")
        );

        let remote = Endpoint {
            host: "10.0.0.2".into(),
            ..endpoint.clone()
        };
        assert!(matches!(
            voice_url(&remote, SPEAK_WS_PATH, ""),
            Err(StreamError::InvalidEndpoint)
        ));
        let untokened = Endpoint {
            token: String::new(),
            ..endpoint
        };
        assert!(matches!(
            voice_url(&untokened, SPEAK_WS_PATH, ""),
            Err(StreamError::InvalidEndpoint)
        ));

        let ticket_url = display_url(&remote, "x").unwrap_err();
        assert!(matches!(ticket_url, StreamError::InvalidEndpoint));
    }

    #[test]
    fn errors_redact_secrets_and_query_values() {
        let secrets = vec!["tok-SECRET".to_owned(), "tkt-SECRET".to_owned()];
        let text = redact_text(
            "connect ws://127.0.0.1:1/api/ws?token=tok-SECRET&display_ticket=tkt-SECRET failed tok-SECRET",
            &secrets,
        );
        assert!(!text.contains("tok-SECRET"), "{text}");
        assert!(!text.contains("tkt-SECRET"), "{text}");
        assert!(text.contains("<redacted>"));
        assert_eq!(redact_query_value("?token=abc&x=1", "token"), "?token=<redacted>&x=1");
    }
}
