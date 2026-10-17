//! Live checks for the voice area (`/api/audio/*` + `voice.*` RPCs) against a PRIVATE test
//! backend.
//!
//! ```sh
//! scripts/live-test-backend.sh    # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_voice -- --ignored --nocapture
//! ```
//!
//! Only routes that make no provider call, no network request and no capture are called live:
//! `GET /api/audio/voice-config` (read; the reply may carry provider keys — never printed),
//! `GET /api/audio/voice-live/status`, the two lease *release* paths (a never-acquired lease is a
//! documented no-op) and the read-only `voice.toggle status` RPC. Speak, transcribe, the
//! ElevenLabs list, GPT-Live sessions and the streaming sockets are builder/unit-tested only.

mod common;

use common::{Checks, backend, tag};
use hermes_core::api::voice::{self, VoiceToggle};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn voice_area_live() {
    let (rest, client) = backend().await;
    let mut checks = Checks::default();
    let tag = tag();

    // ── voice-config: a read-only, network-free resolution. The reply can carry provider
    // API keys in direct mode, so only non-secret fields are ever printed.
    match voice::voice_config(&rest, "").await {
        Ok(c) => {
            let detail = format!(
                "ok={} stt.mode={} tts.mode={} stt.wire={:?} tts.wire={:?} stt.streaming={:?} \
                 (keys withheld)",
                c.ok, c.stt.mode, c.tts.mode, c.stt.wire, c.tts.wire, c.stt.streaming
            );
            let known =
                matches!(c.stt.mode.as_str(), "direct" | "relay") && matches!(c.tts.mode.as_str(), "direct" | "relay");
            checks.check("GET /api/audio/voice-config", c.ok && known, detail);
        }
        Err(e) => checks.check("GET /api/audio/voice-config", false, e.to_string()),
    }

    // ── GPT-Live readiness: local config read, never returns the key.
    match voice::voice_live_status(&rest, "").await {
        Ok(s) => {
            let known = matches!(s.mode.as_str(), "chained" | "gpt-live");
            checks.check(
                "GET /api/audio/voice-live/status",
                s.ok && known && !s.model.is_empty(),
                format!(
                    "mode={} available={} model={} voice={} reason={:?}",
                    s.mode, s.available, s.model, s.voice, s.reason
                ),
            );
        }
        Err(e) => checks.check("GET /api/audio/voice-live/status", false, e.to_string()),
    }

    // ── TTS lease release: releasing a lease nobody acquired is a documented no-op
    // (`release_tts_lease`), so no engine is loaded or unloaded here.
    let tts_lease = format!("desktop:read-aloud:test-{tag}");
    match voice::tts_lease(&rest, "", &tts_lease, false).await {
        Ok(r) => checks.check(
            "POST /api/audio/tts-lease (release)",
            r.ok && r.lease == tts_lease && !r.active && r.leases.is_some(),
            format!(
                "lease={} active={} leases={:?} released={:?}",
                r.lease, r.active, r.leases, r.released
            ),
        ),
        Err(e) => checks.check("POST /api/audio/tts-lease (release)", false, e.to_string()),
    }

    // ── STT lease release: same, and release never unloads the shared engine.
    let stt_lease = format!("desktop:voice-input:test-{tag}");
    match voice::stt_lease(&rest, "", &stt_lease, false).await {
        Ok(r) => checks.check(
            "POST /api/audio/stt-lease (release)",
            r.ok && r.lease == stt_lease && !r.active && r.leases.is_some(),
            format!("lease={} active={} leases={:?}", r.lease, r.active, r.leases),
        ),
        Err(e) => checks.check("POST /api/audio/stt-lease (release)", false, e.to_string()),
    }

    // ── An empty lease is rejected before any warm-up (400), on both routes.
    match rest
        .post(
            "/api/audio/tts-lease",
            &serde_json::json!({ "lease": "", "active": false }),
        )
        .await
    {
        Ok(v) => checks.check(
            "POST /api/audio/tts-lease empty lease → 400",
            false,
            format!("expected 400, got 200: {v}"),
        ),
        Err(e) => {
            let status = match &e {
                hermes_core::http::HttpError::Status { status, body } => Some((*status, body.clone())),
                _ => None,
            };
            let ok = status.as_ref().is_some_and(|(s, _)| *s == 400);
            checks.check(
                "POST /api/audio/tts-lease empty lease → 400",
                ok,
                match status {
                    Some((s, b)) => format!("HTTP {s}: {b}"),
                    None => e.to_string(),
                },
            );
        }
    }

    // ── voice.toggle status: read-only (flips nothing, opens no capture; the requirements
    //      probe only reports what is installed).
    match voice::voice_toggle(&client, VoiceToggle::Status).await {
        Ok(s) => checks.check(
            "RPC voice.toggle status",
            !s.record_key.is_empty(),
            format!(
                "enabled={} tts={} record_key={} available={:?} audio_available={:?} stt_available={:?}",
                s.enabled, s.tts, s.record_key, s.available, s.audio_available, s.stt_available
            ),
        ),
        Err(e) => checks.check("RPC voice.toggle status", false, e.to_string()),
    }

    checks.finish();
}
