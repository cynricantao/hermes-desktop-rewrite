//! Voice: audio REST (`/api/audio/*`: speak, transcribe, voice config, ElevenLabs voices,
//! TTS/STT leases, GPT-Live sessions) and the voice JSON-RPCs (`voice.record`, `voice.toggle`,
//! `voice.tts`).
//!
//! Routes, bodies and reply fields mirror `hermes_cli/web_routers/audio.py`,
//! `hermes_cli/web_models.py` and `tui_gateway/methods_voice.py`.
//!
//! # Streaming sockets (wire format documented, not expressible here)
//! Two endpoints are WebSockets with binary frames, which the two transports in this crate do
//! not carry; a UI needs a raw socket for them (the Electron desktop uses
//! `apps/desktop/src/lib/{voice-stream,voice-playback}.ts`):
//!
//! * `WS /api/audio/transcribe-stream?profile=` — live dictation. Client sends
//!   `{"sample_rate": N}` first (8000–48000), then binary s16le mono PCM frames, then
//!   `{"eos": true}`; disconnect cancels. Server answers `{"type": "partial", "text": …}`,
//!   `{"type": "final", "transcript": …, "provider": …}` or `{"type": "error", "message": …}`
//!   (the last one also when no live STT is configured, so the client falls back to
//!   [`transcribe`] with its recorded blob).
//! * `WS /api/audio/speak-stream?profile=` — streaming reply audio. Client sends
//!   `{"text": "…"}` deltas, `{"done": true}` at end of reply, `{"stop": true}` or a
//!   disconnect for barge-in. Server answers `{"type": "start", "sample_rate": N,
//!   "channels": 1}` (sent with the first PCM frame), binary int16 PCM chunks, then
//!   `{"type": "end"}` — or `{"type": "fallback"}` when sentence synthesis produced no audio
//!   (the client then POSTs the whole reply to [`speak`]).
//!
//! # RPC `profile` param
//! The openrpc contract lists `profile` on all three voice methods, but the gateway handlers
//! (`tui_gateway/methods_voice.py`) never read it — voice state is process-global (one mic, one
//! speaker) and the methods are not `_profile_scoped`. The builders therefore send only `action`
//! / `text` / `session_id`.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::{ApiError, Call, Method, Request, call_as, call_of, decode, profile_query, req};
use crate::http::Rest;
use crate::rpc::Client;

// ── Timeouts (desktop parity) ───────────────────────────────────────────────────
// `apps/desktop/src/api/system.ts` budgets provider round-trips far past the client default
// (10 s here): TTS/STT leases pre-load local models (first-use download), speak/transcribe run
// a whole provider call, and the ElevenLabs list round-trips their API.

/// `POST /api/audio/tts-lease` — acquiring pre-loads the configured engine (desktop:
/// `AUDIO_TTS_LEASE_REQUEST_TIMEOUT_MS`).
pub const TTS_LEASE_TIMEOUT: Duration = Duration::from_secs(180);
/// `POST /api/audio/stt-lease` — acquiring pre-loads the local STT model (desktop:
/// `AUDIO_STT_LEASE_REQUEST_TIMEOUT_MS`).
pub const STT_LEASE_TIMEOUT: Duration = Duration::from_secs(180);
/// `POST /api/audio/voice-live/session` — the vendor exchanges the SDP offer (desktop: 45 s).
pub const VOICE_LIVE_SESSION_TIMEOUT: Duration = Duration::from_secs(45);
/// `GET /api/audio/elevenlabs/voices` — the backend itself allows its upstream call 10 s.
pub const ELEVENLABS_VOICES_TIMEOUT: Duration = Duration::from_secs(15);

const SPEAK_MIN_TIMEOUT_MS: u64 = 180_000;
const SPEAK_MAX_TIMEOUT_MS: u64 = 600_000;
const SPEAK_TIMEOUT_MS_PER_CHAR: u64 = 35;
const TRANSCRIBE_MIN_TIMEOUT_MS: u64 = 180_000;
const TRANSCRIBE_MAX_TIMEOUT_MS: u64 = 600_000;
const TRANSCRIBE_TIMEOUT_MS_PER_CHAR: f64 = 0.1;

/// Desktop parity with `audioSpeakRequestTimeoutMs`: 35 ms per text char, 180 s floor, 600 s cap.
pub fn speak_timeout(text: &str) -> Duration {
    let per_char = text.chars().count() as u64 * SPEAK_TIMEOUT_MS_PER_CHAR;
    Duration::from_millis(SPEAK_MIN_TIMEOUT_MS.max(per_char).min(SPEAK_MAX_TIMEOUT_MS))
}

/// Desktop parity with `audioTranscribeRequestTimeoutMs`: the payload is the base64 data URL,
/// so 0.1 ms per char (180 s floor, 600 s cap) scales with the recording.
pub fn transcribe_timeout(data_url: &str) -> Duration {
    let per_char = (data_url.chars().count() as f64 * TRANSCRIBE_TIMEOUT_MS_PER_CHAR).ceil() as u64;
    Duration::from_millis(TRANSCRIBE_MIN_TIMEOUT_MS.max(per_char).min(TRANSCRIBE_MAX_TIMEOUT_MS))
}

/// Send a prepared [`Request`] with an explicit timeout and decode the reply.
async fn send_timed_as<T: DeserializeOwned>(rest: &Rest, r: &Request, timeout: Duration) -> Result<T, ApiError> {
    let v = rest
        .send_with_timeout(r.method.as_str(), &r.path, r.body.as_ref(), timeout)
        .await?;
    decode(v)
}

// ── Voice config (client-direct STT/TTS) ────────────────────────────────────────

/// One direction of `/api/audio/voice-config`. `mode: "relay"` carries only `reason`; a direct
/// verdict carries the provider endpoint the client may call itself (`wire` names the protocol:
/// `openai-multipart` / `xai-stt` / `elevenlabs-stt` for STT, `openai-speech` / `elevenlabs-tts`
/// for TTS), plus `streaming` on the STT half when live dictation is available.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceEndpoint {
    pub mode: String,
    pub reason: Option<String>,
    pub wire: Option<String>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    /// **Secret**: the provider API key for direct mode (never persisted client-side).
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub min_len: Option<i64>,
    /// Free-form provider extras the client forwards verbatim (`tts.openai.extra_body`).
    pub extra_body: Option<Value>,
    pub language: Option<String>,
    pub timeout_s: Option<f64>,
    /// Free-form Whisper-silence contract (`phrases` + `repeat_regex`).
    pub hallucination_filter: Option<Value>,
    pub streaming: Option<bool>,
}

impl std::fmt::Debug for VoiceEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Provider URLs and opaque extras can contain credentials too. Do not only
        // redact api_key and accidentally expose an Authorization override or URL token.
        f.debug_struct("VoiceEndpoint")
            .field("mode", &self.mode)
            .field("provider", &self.provider)
            .field("has_api_key", &self.api_key.is_some())
            .field("has_base_url", &self.base_url.is_some())
            .field("has_extra_body", &self.extra_body.is_some())
            .field("streaming", &self.streaming)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub ok: bool,
    pub stt: VoiceEndpoint,
    pub tts: VoiceEndpoint,
}

/// The active profile's STT/TTS config for client-direct voice. **Reveals secrets** (direct
/// mode carries the provider API key).
pub fn voice_config_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/audio/voice-config{}", profile_query(profile)),
        None,
    )
}

/// Fetch the client-direct voice config; `profile` `""` = default.
pub async fn voice_config(rest: &Rest, profile: &str) -> Result<VoiceConfig, ApiError> {
    crate::api::send_as(rest, &voice_config_request(profile)).await
}

// ── GPT-Live (full-duplex voice chat mode) ──────────────────────────────────────

/// Which voice chat mode the profile selected (`chained` | `gpt-live`) and whether GPT-Live can
/// start. Non-secret: the key never leaves the host.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceLiveStatus {
    pub ok: bool,
    pub mode: String,
    pub available: bool,
    pub reason: Option<String>,
    pub model: String,
    pub voice: String,
}

/// Which conversation engine to mount (`chained` = STT → turn → TTS, `gpt-live` = WebRTC).
pub fn voice_live_status_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/audio/voice-live/status{}", profile_query(profile)),
        None,
    )
}

/// Read the GPT-Live readiness verdict (local config resolution, no provider call).
pub async fn voice_live_status(rest: &Rest, profile: &str) -> Result<VoiceLiveStatus, ApiError> {
    crate::api::send_as(rest, &voice_live_status_request(profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LiveSession {
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LiveTransport {
    #[serde(rename = "type")]
    pub r#type: String,
    pub sdp: String,
}

/// The vendor's answer to an SDP offer: the session id and the SDP answer to hand to
/// `RTCPeerConnection.setRemoteDescription`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceLiveSession {
    pub ok: bool,
    pub session: Option<LiveSession>,
    pub transport: Option<LiveTransport>,
}

/// Exchange a WebRTC SDP offer for a GPT-Live session. `history` seeds the voice model with
/// prior turns (`[{ "type": "message", "role": …, "content": [{ "type": …, "text": … }] }]`);
/// pass `None` to start empty.
///
/// **Network** and **spends provider calls**: billed OpenAI Live session time ($0.05/min); the
/// project key stays on the host. Requires the offer byte-exact (trailing CRLF included).
pub fn voice_live_session_request(profile: &str, sdp: &str, history: Option<Value>) -> Request {
    let mut body = json!({ "sdp": sdp });
    if let Some(history) = history {
        body["history"] = history;
    }
    req(
        Method::Post,
        format!("/api/audio/voice-live/session{}", profile_query(profile)),
        Some(body),
    )
}

/// Create a GPT-Live session from an SDP offer (see [`voice_live_session_request`]).
pub async fn voice_live_session(
    rest: &Rest,
    profile: &str,
    sdp: &str,
    history: Option<Value>,
) -> Result<VoiceLiveSession, ApiError> {
    send_timed_as(
        rest,
        &voice_live_session_request(profile, sdp, history),
        VOICE_LIVE_SESSION_TIMEOUT,
    )
    .await
}

// ── ElevenLabs voice list ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ElevenLabsVoice {
    pub voice_id: String,
    pub name: String,
    /// Display label from the backend: `"<name> (<category>)"`.
    pub label: String,
}

/// `available: false` when no ElevenLabs key is configured (or the key is rejected —
/// then `error: "unauthorized"`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ElevenLabsVoices {
    pub available: bool,
    pub voices: Vec<ElevenLabsVoice>,
    pub error: Option<String>,
}

/// Voices for the `tts.elevenlabs.voice_id` dropdown. **Network**: the backend calls
/// `api.elevenlabs.io` (the API key stays server-side); 502 when the list cannot be loaded.
pub fn elevenlabs_voices_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/audio/elevenlabs/voices{}", profile_query(profile)),
        None,
    )
}

/// List ElevenLabs voices (see [`elevenlabs_voices_request`]).
pub async fn elevenlabs_voices(rest: &Rest, profile: &str) -> Result<ElevenLabsVoices, ApiError> {
    send_timed_as(rest, &elevenlabs_voices_request(profile), ELEVENLABS_VOICES_TIMEOUT).await
}

// ── Relay synthesis + transcription ─────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpeakReply {
    pub ok: bool,
    /// `data:<mime>;base64,<audio>` — playable without exposing the host file path.
    pub data_url: String,
    pub mime_type: String,
    pub provider: Option<String>,
}

/// Synthesize `text` through the profile's TTS chain. **Spends provider calls** (the reply is
/// base64 audio, several MB for long text).
pub fn speak_request(profile: &str, text: &str) -> Request {
    req(
        Method::Post,
        format!("/api/audio/speak{}", profile_query(profile)),
        Some(json!({ "text": text })),
    )
}

/// Speak `text` via the relay endpoint, with the desktop's size-scaled timeout
/// ([`speak_timeout`]).
pub async fn speak(rest: &Rest, profile: &str, text: &str) -> Result<SpeakReply, ApiError> {
    send_timed_as(rest, &speak_request(profile, text), speak_timeout(text)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TranscribeReply {
    pub ok: bool,
    /// `""` means silence (the backend maps a provider "empty transcript" to this, so a voice
    /// loop re-listens instead of showing an error).
    pub transcript: String,
    pub provider: Option<String>,
}

/// Transcribe one recording. `data_url` is a base64 audio data URL
/// (`data:audio/webm;base64,…`, ≤ 25 MiB decoded, mime `audio/*` or `video/webm`); `mime_type`
/// overrides the header's mime when given. **Spends provider calls**: the backend writes the
/// blob to a temp file and runs the profile's STT chain.
pub fn transcribe_request(profile: &str, data_url: &str, mime_type: Option<&str>) -> Request {
    let mut body = json!({ "data_url": data_url });
    if let Some(mime) = mime_type {
        body["mime_type"] = json!(mime);
    }
    req(
        Method::Post,
        format!("/api/audio/transcribe{}", profile_query(profile)),
        Some(body),
    )
}

/// Transcribe a recorded blob (see [`transcribe_request`]), with the desktop's size-scaled
/// timeout ([`transcribe_timeout`]).
pub async fn transcribe(
    rest: &Rest,
    profile: &str,
    data_url: &str,
    mime_type: Option<&str>,
) -> Result<TranscribeReply, ApiError> {
    send_timed_as(
        rest,
        &transcribe_request(profile, data_url, mime_type),
        transcribe_timeout(data_url),
    )
    .await
}

// ── Engine leases (warm-up / release) ───────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TtsLeaseReply {
    pub ok: bool,
    pub lease: String,
    pub active: bool,
    /// Live lease holders after this call (`null` when the call itself failed).
    pub leases: Option<i64>,
    /// Warm-up outcome: `loaded` | `cached` | `installed` | `warmed` | `noop` | `error`.
    pub action: Option<String>,
    pub provider: Option<String>,
    pub warmed: Option<bool>,
    pub elapsed_ms: Option<i64>,
    /// Resident local models dropped inline on release.
    pub released: Option<i64>,
    /// Warm-up failure detail — the toggle still succeeds (`ok: true`).
    pub error: Option<String>,
}

/// Warm up (`active: true`) or release (`active: false`) the TTS engine for one surface.
/// `lease` names the holder (`desktop:read-aloud`, `desktop:conversation`; `desktop:…` in this
/// client). Acquiring **loads a local model** and may **download** one on first use; releasing a
/// never-acquired lease is a no-op.
pub fn tts_lease_request(profile: &str, lease: &str, active: bool) -> Request {
    req(
        Method::Post,
        format!("/api/audio/tts-lease{}", profile_query(profile)),
        Some(json!({ "lease": lease, "active": active })),
    )
}

/// TTS engine lease (see [`tts_lease_request`]), with the desktop's 180 s warm-up timeout.
pub async fn tts_lease(rest: &Rest, profile: &str, lease: &str, active: bool) -> Result<TtsLeaseReply, ApiError> {
    send_timed_as(rest, &tts_lease_request(profile, lease, active), TTS_LEASE_TIMEOUT).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SttLeaseReply {
    pub ok: bool,
    pub lease: String,
    pub active: bool,
    pub leases: Option<i64>,
    /// Warm-up outcome: `loaded` | `cached` | `noop` | `error`; release carries none.
    pub action: Option<String>,
    /// Whether the configured engine was actually warmed (`false` for cloud providers).
    pub warmed: Option<bool>,
    pub provider: Option<String>,
    pub error: Option<String>,
}

/// Warm up (`active: true`) or release (`active: false`) local STT for one voice-input session.
/// Acquiring **pre-loads the local model** (first-use download); release never unloads (the
/// engine is shared with the gateway).
pub fn stt_lease_request(profile: &str, lease: &str, active: bool) -> Request {
    req(
        Method::Post,
        format!("/api/audio/stt-lease{}", profile_query(profile)),
        Some(json!({ "lease": lease, "active": active })),
    )
}

/// STT engine lease (see [`stt_lease_request`]), with the desktop's 180 s warm-up timeout.
pub async fn stt_lease(rest: &Rest, profile: &str, lease: &str, active: bool) -> Result<SttLeaseReply, ApiError> {
    send_timed_as(rest, &stt_lease_request(profile, lease, active), STT_LEASE_TIMEOUT).await
}

// ── RPC: voice.toggle / voice.record / voice.tts ────────────────────────────────

/// `voice.toggle` actions. `Status` only reports (and probes requirements); `On`/`Off` flip
/// voice *mode* (off also tears down the continuous loop and the mic); `Tts` flips speech output
/// and needs mode on first (error 4014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceToggle {
    Status,
    On,
    Off,
    Tts,
}

impl VoiceToggle {
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceToggle::Status => "status",
            VoiceToggle::On => "on",
            VoiceToggle::Off => "off",
            VoiceToggle::Tts => "tts",
        }
    }
}

/// The `/voice` status payload (every toggle branch answers with it).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceStatus {
    pub enabled: bool,
    pub record_key: String,
    pub tts: bool,
    /// Spoken stop hint, on `On` only.
    pub stop_hint: Option<String>,
    /// Requirement probe, on `Status` only.
    pub available: Option<bool>,
    pub audio_available: Option<bool>,
    pub stt_available: Option<bool>,
    pub details: Option<String>,
}

/// Flip or report voice mode. **`Off` tears down a running capture**; `Status` is read-only.
pub fn voice_toggle_call(action: VoiceToggle) -> Call {
    call_of("voice.toggle", json!({ "action": action.as_str() }))
}

/// `voice.toggle` (see [`voice_toggle_call`]).
pub async fn voice_toggle(client: &Client, action: VoiceToggle) -> Result<VoiceStatus, ApiError> {
    call_as(client, &voice_toggle_call(action)).await
}

/// `voice.record` actions: `Start` arms VAD-bounded push-to-talk, `Stop` forces transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceRecord {
    Start,
    Stop,
}

impl VoiceRecord {
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceRecord::Start => "start",
            VoiceRecord::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceRecordReply {
    /// `recording` | `stopped` | `busy` (`busy` when another surface owns the mic).
    pub status: String,
    pub reason: Option<String>,
}

/// Push-to-talk on the backend host. `session_id` addresses the `voice.transcript` /
/// `voice.status` / `voice.partial` events that follow; `""` targets the active session.
///
/// **Spawns a capture**: `Start` opens the backend host's microphone (and cuts in-flight TTS),
/// `Stop` ends it and transcribes; the transcript arrives as a `voice.transcript` event, not in
/// this reply. `Start` needs voice mode on (error 4015).
pub fn voice_record_call(action: VoiceRecord, session_id: &str) -> Call {
    let mut params = json!({ "action": action.as_str() });
    if !session_id.is_empty() {
        params["session_id"] = json!(session_id);
    }
    call_of("voice.record", params)
}

/// Push-to-talk (see [`voice_record_call`]).
pub async fn voice_record(
    client: &Client,
    action: VoiceRecord,
    session_id: &str,
) -> Result<VoiceRecordReply, ApiError> {
    call_as(client, &voice_record_call(action, session_id)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceTtsReply {
    /// `speaking`.
    pub status: String,
}

/// Speak `text` through the backend TTS engine (barge-in aware, fire-and-forget: the reply only
/// acknowledges). **Spends provider calls**; error 4020 on empty text.
pub fn voice_tts_call(text: &str) -> Call {
    call_of("voice.tts", json!({ "text": text }))
}

/// Backend TTS (see [`voice_tts_call`]).
pub async fn voice_tts(client: &Client, text: &str) -> Result<VoiceTtsReply, ApiError> {
    call_as(client, &voice_tts_call(text)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn voice_config_builder() {
        let r = voice_config_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/audio/voice-config", None)
        );
        assert_eq!(voice_config_request("swe").path, "/api/audio/voice-config?profile=swe");
    }

    #[test]
    fn voice_config_decodes_direct_and_relay() {
        let v: VoiceConfig = decode(json!({
            "ok": true,
            "stt": {
                "mode": "direct", "wire": "openai-multipart", "provider": "openai",
                "base_url": "https://api.openai.com/v1", "api_key": "sk-test", "model": "whisper-1",
                "language": "en", "timeout_s": 60.0,
                "hallucination_filter": { "phrases": ["thank you"], "repeat_regex": "^(?:thank you)+$" },
                "streaming": true
            },
            "tts": { "mode": "relay", "reason": "provider 'edge' has no client wire" }
        }))
        .unwrap();
        assert!(v.ok);
        assert_eq!(v.stt.mode, "direct");
        assert_eq!(v.stt.wire.as_deref(), Some("openai-multipart"));
        assert_eq!(v.stt.model.as_deref(), Some("whisper-1"));
        assert_eq!(v.stt.timeout_s, Some(60.0));
        assert_eq!(v.stt.streaming, Some(true));
        assert_eq!(v.tts.mode, "relay");
        assert_eq!(v.tts.reason.as_deref(), Some("provider 'edge' has no client wire"));
        assert!(v.tts.wire.is_none() && v.tts.api_key.is_none());
        // Lenient: an older/other backend's shape still decodes.
        let empty: VoiceConfig = decode(json!({})).unwrap();
        assert_eq!(empty, VoiceConfig::default());
    }

    #[test]
    fn voice_live_status_builder_and_decode() {
        let r = voice_live_status_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/audio/voice-live/status", None)
        );
        assert_eq!(
            voice_live_status_request("swe").path,
            "/api/audio/voice-live/status?profile=swe"
        );
        let s: VoiceLiveStatus = decode(json!({
            "ok": true, "mode": "chained", "available": false,
            "reason": "no OpenAI API key (set OPENAI_API_KEY or voice.gpt_live.api_key)",
            "model": "gpt-live-1", "voice": "marin"
        }))
        .unwrap();
        assert_eq!(s.mode, "chained");
        assert!(!s.available);
        assert_eq!(s.model, "gpt-live-1");
        assert!(decode::<VoiceLiveStatus>(json!({})).unwrap().mode.is_empty());
    }

    #[test]
    fn voice_live_session_builder_and_decode() {
        let r = voice_live_session_request("", "v=0\r\n", None);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/audio/voice-live/session",
                Some(json!({ "sdp": "v=0\r\n" }))
            )
        );
        let history = json!([{ "type": "message", "role": "user",
                              "content": [{ "type": "input_text", "text": "hi" }] }]);
        let r = voice_live_session_request("swe", "v=0\r\n", Some(history.clone()));
        assert_eq!(r.path, "/api/audio/voice-live/session?profile=swe");
        assert_eq!(r.body.unwrap()["history"], history);
        let s: VoiceLiveSession = decode(json!({
            "ok": true, "session": { "id": "sess_abc" },
            "transport": { "type": "webrtc", "sdp": "v=0\r\n" }
        }))
        .unwrap();
        assert_eq!(s.session.as_ref().unwrap().id, "sess_abc");
        assert_eq!(s.transport.unwrap().r#type, "webrtc");
        assert!(decode::<VoiceLiveSession>(json!({})).unwrap().transport.is_none());
    }

    #[test]
    fn elevenlabs_requests_and_decode() {
        let r = elevenlabs_voices_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/audio/elevenlabs/voices?profile=swe", None)
        );
        let v: ElevenLabsVoices = decode(json!({
            "available": true,
            "voices": [{ "voice_id": "V1", "name": "Rachel", "label": "Rachel (premade)" }]
        }))
        .unwrap();
        assert!(v.available);
        assert_eq!(v.voices[0].label, "Rachel (premade)");
        let none: ElevenLabsVoices =
            decode(json!({ "available": false, "voices": [], "error": "unauthorized" })).unwrap();
        assert_eq!(none.error.as_deref(), Some("unauthorized"));
        assert!(none.voices.is_empty());
        assert!(decode::<ElevenLabsVoices>(json!({})).unwrap().voices.is_empty());
    }

    #[test]
    fn speak_builder_and_decode() {
        let r = speak_request("", "hello");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/audio/speak", Some(json!({ "text": "hello" })))
        );
        assert_eq!(speak_request("swe", "x").path, "/api/audio/speak?profile=swe");
        let s: SpeakReply = decode(json!({
            "ok": true, "data_url": "data:audio/mpeg;base64,AAA", "mime_type": "audio/mpeg",
            "provider": "edge"
        }))
        .unwrap();
        assert_eq!(s.mime_type, "audio/mpeg");
        assert_eq!(s.provider.as_deref(), Some("edge"));
        assert!(decode::<SpeakReply>(json!({})).unwrap().data_url.is_empty());
    }

    #[test]
    fn transcribe_builder_and_decode() {
        let r = transcribe_request("", "data:audio/webm;base64,AAA", None);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/audio/transcribe",
                Some(json!({ "data_url": "data:audio/webm;base64,AAA" }))
            )
        );
        let r = transcribe_request("swe", "data:audio/wav;base64,AA", Some("audio/wav"));
        assert_eq!(r.path, "/api/audio/transcribe?profile=swe");
        assert_eq!(
            r.body.unwrap(),
            json!({ "data_url": "data:audio/wav;base64,AA", "mime_type": "audio/wav" })
        );
        let t: TranscribeReply = decode(json!({
            "ok": true, "transcript": "hello there", "provider": "openai"
        }))
        .unwrap();
        assert_eq!(t.transcript, "hello there");
        // A silent clip answers an empty transcript with no provider on an older backend.
        let empty: TranscribeReply = decode(json!({ "ok": true, "transcript": "" })).unwrap();
        assert!(empty.provider.is_none());
        assert!(decode::<TranscribeReply>(json!({})).unwrap().transcript.is_empty());
    }

    #[test]
    fn tts_lease_builder_and_decode() {
        let r = tts_lease_request("", "desktop:read-aloud", true);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/audio/tts-lease",
                Some(json!({ "lease": "desktop:read-aloud", "active": true }))
            )
        );
        assert_eq!(
            tts_lease_request("swe", "desktop:conversation", false).path,
            "/api/audio/tts-lease?profile=swe"
        );
        let warm: TtsLeaseReply = decode(json!({
            "ok": true, "lease": "desktop:read-aloud", "active": true, "leases": 1,
            "provider": "edge", "warmed": false, "action": "noop"
        }))
        .unwrap();
        assert_eq!(warm.leases, Some(1));
        assert_eq!(warm.action.as_deref(), Some("noop"));
        let release: TtsLeaseReply = decode(json!({
            "ok": true, "lease": "desktop:read-aloud", "active": false, "leases": 0,
            "released": 0
        }))
        .unwrap();
        assert_eq!(release.released, Some(0));
        // A failed warm-up reports 200 with the error in the body.
        let failed: TtsLeaseReply = decode(json!({
            "ok": true, "lease": "l", "active": true, "leases": null,
            "action": "error", "error": "engine missing"
        }))
        .unwrap();
        assert!(failed.leases.is_none());
        assert_eq!(failed.error.as_deref(), Some("engine missing"));
        assert!(decode::<TtsLeaseReply>(json!({})).unwrap().lease.is_empty());
    }

    #[test]
    fn stt_lease_builder_and_decode() {
        let r = stt_lease_request("", "desktop:voice-input:1", false);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/audio/stt-lease",
                Some(json!({ "lease": "desktop:voice-input:1", "active": false }))
            )
        );
        assert_eq!(
            stt_lease_request("swe", "l", true).path,
            "/api/audio/stt-lease?profile=swe"
        );
        let warm: SttLeaseReply = decode(json!({
            "ok": true, "lease": "desktop:voice-input:1", "active": true, "leases": 1,
            "provider": "local", "warmed": true, "action": "loaded"
        }))
        .unwrap();
        assert_eq!(warm.warmed, Some(true));
        assert_eq!(warm.provider.as_deref(), Some("local"));
        let release: SttLeaseReply = decode(json!({
            "ok": true, "lease": "desktop:voice-input:1", "active": false, "leases": 0
        }))
        .unwrap();
        assert!(release.action.is_none());
        assert!(decode::<SttLeaseReply>(json!({})).unwrap().leases.is_none());
    }

    #[test]
    fn rpc_builders() {
        let c = voice_toggle_call(VoiceToggle::Status);
        assert_eq!((c.method, c.params), ("voice.toggle", json!({ "action": "status" })));
        assert_eq!(voice_toggle_call(VoiceToggle::On).params, json!({ "action": "on" }));
        assert_eq!(voice_toggle_call(VoiceToggle::Off).params, json!({ "action": "off" }));
        assert_eq!(voice_toggle_call(VoiceToggle::Tts).params, json!({ "action": "tts" }));

        let c = voice_record_call(VoiceRecord::Start, "");
        assert_eq!((c.method, c.params), ("voice.record", json!({ "action": "start" })));
        let c = voice_record_call(VoiceRecord::Stop, "sess-1");
        assert_eq!(
            (c.method, c.params),
            ("voice.record", json!({ "action": "stop", "session_id": "sess-1" }))
        );

        let c = voice_tts_call("hi");
        assert_eq!((c.method, c.params), ("voice.tts", json!({ "text": "hi" })));
    }

    #[test]
    fn rpc_results_decode() {
        let t: VoiceStatus = decode(json!({
            "enabled": true, "record_key": "ctrl+b", "tts": false, "available": true,
            "audio_available": true, "stt_available": false, "details": "Audio capture: OK"
        }))
        .unwrap();
        assert!(t.enabled);
        assert_eq!(t.record_key, "ctrl+b");
        assert_eq!(t.details.as_deref(), Some("Audio capture: OK"));
        let on: VoiceStatus = decode(json!({
            "enabled": true, "record_key": "ctrl+b", "tts": true, "stop_hint": "stop"
        }))
        .unwrap();
        assert_eq!(on.stop_hint.as_deref(), Some("stop"));
        assert!(on.available.is_none());
        assert_eq!(decode::<VoiceStatus>(json!({})).unwrap(), VoiceStatus::default());

        let r: VoiceRecordReply = decode(json!({ "status": "busy", "reason": "wake_owned" })).unwrap();
        assert_eq!(r.status, "busy");
        assert_eq!(r.reason.as_deref(), Some("wake_owned"));
        assert!(decode::<VoiceRecordReply>(json!({})).unwrap().reason.is_none());

        let s: VoiceTtsReply = decode(json!({ "status": "speaking" })).unwrap();
        assert_eq!(s.status, "speaking");
        assert!(decode::<VoiceTtsReply>(json!({})).unwrap().status.is_empty());
    }

    #[test]
    fn timeouts_mirror_the_desktop() {
        assert_eq!(speak_timeout(""), Duration::from_secs(180));
        assert_eq!(speak_timeout("hello"), Duration::from_secs(180));
        // 35 ms per char: 10 000 chars → 350 s.
        assert_eq!(speak_timeout(&"x".repeat(10_000)), Duration::from_secs(350));
        // Cap: 600 s.
        assert_eq!(speak_timeout(&"x".repeat(100_000)), Duration::from_secs(600));

        assert_eq!(transcribe_timeout("data:,"), Duration::from_secs(180));
        // 0.1 ms per char: 3 000 000 chars → 300 s.
        assert_eq!(transcribe_timeout(&"a".repeat(3_000_000)), Duration::from_secs(300));
        assert_eq!(transcribe_timeout(&"a".repeat(8_000_000)), Duration::from_secs(600));

        assert_eq!(TTS_LEASE_TIMEOUT, Duration::from_secs(180));
        assert_eq!(STT_LEASE_TIMEOUT, Duration::from_secs(180));
        assert_eq!(VOICE_LIVE_SESSION_TIMEOUT, Duration::from_secs(45));
        assert_eq!(ELEVENLABS_VOICES_TIMEOUT, Duration::from_secs(15));
    }
}
