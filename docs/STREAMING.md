# Streaming socket clients

`hermes_core::streaming` provides UI-free clients for the backend's binary WebSocket
routes, using the existing Tokio and tungstenite dependencies.

| Client | Backend route | Traffic |
|---|---|---|
| `TranscribeStream` | `/api/audio/transcribe-stream` | sample-rate JSON, s16le mono PCM, one EOS; partial/final/error replies |
| `SpeakStream` | `/api/audio/speak-stream` | incremental text, one done, stop/cancel; format metadata, PCM, end/fallback (an `error` frame is also accepted defensively; `audio.py` does not send one today) |
| `DisplayStream` | `/api/display/ws` | raw binary RFB bytes; typed lease/auth/desktop close outcomes |

## Boundaries

These are usable transport clients, not audio-device or screen widgets. Mic capture,
speaker playback, RFB decoding/rendering, native setup and remote HTTPS/OAuth/SSH are
not included in this batch. A screen must explicitly connect the client and consume
its events. No background producer or unbounded internal event queue is spawned.

Connections are restricted to an explicit loopback `backend::Endpoint`; voice uses
its session token and profile query, display uses the single-use ticket returned by
`display.observe`. The display route/path cannot be overridden by an untrusted reply.
Do not log ticket-bearing URLs. Client Debug output and protocol failures redact
credentials rather than echoing server frame content.

`StreamConfig` sets connection/read/write budgets, inbound message limits and binary
chunk limits. Defaults admit a whole PCM sentence up to 16 MiB, because the synchronous
TTS provider sends one sentence per WebSocket message, not 64 KiB chunks. Callers may
choose lower limits for their use case; configured limits still reject oversized input
before full buffering. Audio chunks must have an even number of bytes for 16-bit mono samples.
A TTS start frame supplies the sample rate before any PCM. The backend forwards the
provider's rate unclamped, so `start` accepts 1–192 kHz (dictation stays 8–48 kHz);
the caller resamples for its output device. The caller owns the PCM
buffer after each receive and must apply its own bounded playback buffering.

## Lifecycle and recovery

- Clients own their sockets. Drop the client when finished. `close` requests an
  orderly close and wakes waiting reads; it is a best-effort handshake, not proof
  the peer completed its work.
- Reads and writes can proceed concurrently. Outbound voice controls serialize their
  state changes so text/PCM cannot be queued behind a terminal done/EOS marker.
- `SpeakStream::stop` signals barge-in and wakes a pending receive. Late audio is
  discarded. `cancel` also marks the stream closed and wakes readers, even when the
  stop frame could not be written. After a failed write no close frame can be sent,
  so only dropping the stream releases the socket. A `stop` future dropped before it
  completes may not reach the server; follow it with `cancel` or drop the stream.
  Stop does not rewind audio the device already played; playback cancellation
  belongs to the caller.
- Cancelling or timing out a write has an unknown wire outcome. The connection is
  poisoned instead of being reused for an automatic retry. Drop it. No replay or
  automatic reconnection is performed.
- Display closes `4000`, `4001`, `4401`, `4403` map to control taken, desktop gone,
  bad ticket and policy refusal. Return to an explicit observe/watch decision.
  Never silently take the human-control lease after a dropped connection.

## Verification

`cargo test --locked -p hermes-core --test streaming_protocol --test streaming_review -j 2`
exercises actual loopback WebSocket handshakes/frames. The checks cover STT/TTS happy
paths, PCM/frame validation, rate/metadata rejection, EOS/done/stop, late-PCM barge-in,
read/connect timeouts, display byte exchange and semantic closes, forbidden handshakes,
profile encoding and credential redaction. These are deterministic protocol fixtures,
not real microphone/speaker/VNC/provider verification.

Live private-backend checks must use an explicit isolated test home/port and must not
start audio synthesis, model downloads or sandbox installation merely to test metadata.
