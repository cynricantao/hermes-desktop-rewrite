# Voice API (`api::voice`)

Typed client for the backend's voice feature area: the `/api/audio/*` REST routes plus the
`voice.record`, `voice.toggle` and `voice.tts` JSON-RPC methods. UI-free; the controller maps
these into the voice conversation / dictation / read-aloud screens.

Sources of truth: `hermes_cli/web_routers/audio.py`, `hermes_cli/web_models.py`,
`tui_gateway/methods_voice.py`, `apps/shared/src/gateway-contract.openrpc.json`, and the
Electron calls in `apps/desktop/src/api/system.ts`, `apps/desktop/src/lib/voice-live.ts`,
`apps/desktop/src/lib/voice-client-direct.ts`.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Client-direct voice config | `voice_config_request` / `voice_config` | `GET /api/audio/voice-config?profile=` | **Reveals secrets**: a `direct` verdict carries the provider `api_key` (client-memory only), `wire` names the protocol the client speaks (`openai-multipart`/`xai-stt`/`elevenlabs-stt`, `openai-speech`/`elevenlabs-tts`); `mode: "relay"` + `reason` otherwise. `stt.streaming` says whether live dictation is available. Local/command/plugin providers always answer relay. |
| GPT-Live readiness | `voice_live_status_request` / `voice_live_status` | `GET /api/audio/voice-live/status?profile=` | Which engine to mount (`chained` \| `gpt-live`) and whether GPT-Live can start; non-secret local config read (never returns the key). |
| GPT-Live session | `voice_live_session_request` / `voice_live_session` | `POST /api/audio/voice-live/session?profile=` body `{sdp, history?}` | **Network** + **spends provider calls** (billed OpenAI Live session, $0.05/min). The SDP offer must be byte-exact (trailing CRLF included); `history` is the optional prior-turn array (`{type:"message", role, content:[{type, text}]}`). Reply: `{ok, session:{id}, transport:{type, sdp}}` → feed `transport.sdp` to `setRemoteDescription`. 45 s timeout. |
| ElevenLabs voice list | `elevenlabs_voices_request` / `elevenlabs_voices` | `GET /api/audio/elevenlabs/voices?profile=` | **Network** (backend calls `api.elevenlabs.io`; key stays server-side). `available:false` when no key / rejected key (`error:"unauthorized"`); 502 when the list cannot be loaded. 15 s timeout. |
| Relay speak | `speak_request` / `speak` | `POST /api/audio/speak?profile=` body `{text}` | **Spends provider calls**; reply is base64 audio (`data_url`, `mime_type`, `provider`). Timeout scales with text length (`speak_timeout`: 35 ms/char, 180 s floor, 600 s cap — desktop parity). |
| Relay transcribe | `transcribe_request` / `transcribe` | `POST /api/audio/transcribe?profile=` body `{data_url, mime_type?}` | **Spends provider calls**. `data_url` is base64 audio (`data:audio/webm;base64,…`, ≤ 25 MiB decoded, `audio/*` or `video/webm`); `""` transcript = silence (not an error). Timeout `transcribe_timeout`: 0.1 ms/char over the data URL, 180 s floor, 600 s cap. |
| TTS engine lease | `tts_lease_request` / `tts_lease` | `POST /api/audio/tts-lease?profile=` body `{lease, active}` | Acquiring **loads a local model** and may **download** one on first use; releasing the last lease unloads resident models after the keep-warm window. Warm-up failures stay HTTP 200 with `action:"error"` + `error`. 180 s timeout. |
| STT engine lease | `stt_lease_request` / `stt_lease` | `POST /api/audio/stt-lease?profile=` body `{lease, active}` | Acquiring **pre-loads the local STT model** (first-use download); release never unloads (the engine is shared with the gateway/CLI). 180 s timeout. |
| Voice mode toggle | `voice_toggle_call` / `voice_toggle` | RPC `voice.toggle` `{action}` | `status` (read-only report + requirement probe), `on` / `off` (mode; `off` also tears down the continuous loop and the mic), `tts` (speech output; needs mode on — error 4014). Reply: `{enabled, record_key, tts, stop_hint?, available?, audio_available?, stt_available?, details?}`. |
| Push-to-talk | `voice_record_call` / `voice_record` | RPC `voice.record` `{action, session_id?}` | **Spawns a capture**: `start` opens the *backend host's* microphone (and cuts in-flight TTS), `stop` forces transcription. Reply `{status: recording\|stopped\|busy, reason?}` (`busy`+`reason:"wake_owned"` = another surface owns the mic); the text arrives as `voice.transcript` / `voice.partial` / `voice.status` events addressed to `session_id`. |
| Backend TTS | `voice_tts_call` / `voice_tts` | RPC `voice.tts` `{text}` | **Spends provider calls**; fire-and-forget (`{status:"speaking"}`), barge-in aware. Error 4020 on empty text. |

Helpers: `speak_timeout(text)`, `transcribe_timeout(data_url)`, `TTS_LEASE_TIMEOUT`,
`STT_LEASE_TIMEOUT`, `VOICE_LIVE_SESSION_TIMEOUT`, `ELEVENLABS_VOICES_TIMEOUT` (all mirror
`apps/desktop/src/api/system.ts`, which budgets past this crate's 10 s REST default).

Response types (`VoiceConfig`/`VoiceEndpoint`, `VoiceLiveStatus`, `VoiceLiveSession`,
`ElevenLabsVoices`, `SpeakReply`, `TranscribeReply`, `TtsLeaseReply`, `SttLeaseReply`,
`VoiceStatus`, `VoiceRecordReply`, `VoiceTtsReply`) are lenient: `#[serde(default)]`, so a
missing or extra field never fails a decode.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_voice.rs` — 6 checks, run against
`HERMES_DESKTOP_PORT=37273` (`scripts/live-test-backend.sh`):

* `GET /api/audio/voice-config` (decodes + `mode` is `direct`/`relay`; **keys never printed**)
* `GET /api/audio/voice-live/status` (decodes, `mode` is `chained`/`gpt-live`, model named)
* `POST /api/audio/tts-lease` with `active:false` for a throwaway lease (documented no-op)
* `POST /api/audio/stt-lease` with `active:false` for a throwaway lease (documented no-op)
* `POST /api/audio/tts-lease` with an empty lease → HTTP 400 `lease is required`
* RPC `voice.toggle` `status` (read-only; `record_key` non-empty)

## Not run live (and why)

| Endpoint | Why not |
|---|---|
| `POST /api/audio/speak`, RPC `voice.tts` | Spend TTS provider calls. |
| `POST /api/audio/transcribe` | Spends an STT provider call. |
| `GET /api/audio/elevenlabs/voices` | Network call to the ElevenLabs API. |
| `POST /api/audio/voice-live/session` | Network + a billed OpenAI Live session; needs a real WebRTC SDP offer. |
| `POST /api/audio/tts-lease` / `stt-lease` with `active:true` | Preloads (and can download) a local model / lazily installs an SDK. |
| RPC `voice.record` `start`/`stop` | Opens the backend host's microphone and transcribes; `stop` also forces a transcription pass. |
| RPC `voice.toggle` `on`/`off`/`tts` | Change process voice state and tear down a live capture (only `status` is read-only). |

## Not found / not implemented

* **`PUT /api/audio/voice-config`** — not served. The backend only has
  `GET /api/audio/voice-config` (`hermes_cli/web_routers/audio.py`); a grep of
  `hermes_cli/` finds no write route, so there is no config write to mirror (nothing to restore
  after a live write either).
* **`WS /api/audio/transcribe-stream`**, **`WS /api/audio/speak-stream`** — served, but they are
  WebSockets with binary frames: neither the `Rest` nor the JSON-RPC transport can express them.
  Exact wire format (module docs, `api::voice`): transcribe-stream = client
  `{"sample_rate": N}` (8000–48000) → binary s16le mono PCM → `{"eos": true}`; server
  `{"type":"partial","text"}` / `{"type":"final","transcript","provider"}` /
  `{"type":"error","message"}` (also when no live STT exists → fall back to `transcribe`).
  speak-stream = client `{"text"}` deltas / `{"done":true}` / `{"stop":true}`; server
  `{"type":"start","sample_rate","channels":1}` + binary int16 PCM + `{"type":"end"}`, or
  `{"type":"fallback"}` when sentence synthesis produced no audio (then POST the reply to
  `speak`).
* **RPC `profile` param** — the openrpc contract lists `profile` on `voice.toggle`,
  `voice.record` and `voice.tts`, but the handlers never read it and the methods are not
  `_profile_scoped`: voice state is process-global (one mic / one speaker). The builders send
  only `action` / `text` / `session_id` rather than a param that does nothing.
* No other `/api/audio*` routes exist: `hermes_cli/web_routers/audio.py` defines exactly the ten
  endpoints above — eight request/response routes plus the two WebSockets (`web_server.py` only
  includes the router).
