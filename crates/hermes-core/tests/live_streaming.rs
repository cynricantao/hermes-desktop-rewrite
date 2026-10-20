//! Live checks for the raw audio/display sockets against a PRIVATE test backend only.
//!
//! ```sh
//! scripts/live-test-backend.sh        # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_streaming -- --ignored --nocapture
//! ```
//!
//! Speak-stream asks the profile's configured TTS provider for one short sentence, so it uses
//! that provider (network for cloud voices). The PCM it returns is fed back into the live
//! dictation socket, which uses the configured STT. No microphone, speaker or screen is used.
//! Which path ran (real audio, fallback, or no live STT) is printed, not assumed.
mod common;

use std::time::Duration;

use hermes_core::api::display;
use hermes_core::streaming::{
    MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, SpeakEvent, SpeakStream, StreamConfig, TranscribeEvent, TranscribeStream,
};

const SENTENCE: &str = "Hello from the Hermes desktop streaming test.";

fn config() -> StreamConfig {
    StreamConfig {
        read_timeout: Duration::from_secs(90),
        ..StreamConfig::default()
    }
}

/// Speak one sentence. Returns `(sample_rate, pcm)` when real audio arrived.
async fn speak(endpoint: &hermes_core::backend::Endpoint, c: &mut common::Checks) -> Option<(u32, Vec<u8>)> {
    let stream = match SpeakStream::connect(endpoint, "", config()).await {
        Ok(stream) => stream,
        Err(e) => {
            c.check("speak-stream connects with the session token", false, e.to_string());
            return None;
        }
    };
    c.check("speak-stream connects with the session token", true, "loopback ws");
    let pushed = stream.push_text(SENTENCE).await.and(stream.finish().await);
    c.check(
        "speak-stream accepts text and done",
        pushed.is_ok(),
        format!("{pushed:?}"),
    );
    let (mut rate, mut pcm, mut ending) = (None, Vec::new(), String::from("closed"));
    loop {
        match stream.recv().await {
            Ok(Some(SpeakEvent::Start { sample_rate, .. })) => rate = Some(sample_rate),
            Ok(Some(SpeakEvent::Pcm(bytes))) => pcm.extend_from_slice(&bytes),
            Ok(Some(SpeakEvent::End)) => ending = "end".into(),
            Ok(Some(SpeakEvent::Fallback)) => ending = "fallback".into(),
            Ok(Some(SpeakEvent::Error { message })) => ending = format!("error: {message}"),
            Ok(None) => break,
            Err(e) => {
                ending = format!("stream error: {e}");
                break;
            }
        }
    }
    let _ = stream.close().await;
    let real_audio = ending == "end" && rate.is_some() && !pcm.is_empty();
    let detail = format!(
        "ending={ending} rate={rate:?} pcm_bytes={} (~{:.1}s)",
        pcm.len(),
        rate.map(|r| pcm.len() as f64 / 2.0 / r as f64).unwrap_or(0.0)
    );
    c.check(
        "speak-stream ends with real PCM or an explicit fallback",
        real_audio || ending == "fallback",
        detail,
    );
    real_audio.then(|| (rate.unwrap(), pcm))
}

/// Feed real speech PCM into live dictation and read the transcript.
async fn transcribe(endpoint: &hermes_core::backend::Endpoint, rate: u32, pcm: &[u8], c: &mut common::Checks) {
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate) {
        println!("[SKIP] transcribe-stream: TTS rate {rate} is outside the dictation range");
        return;
    }
    let stream = match TranscribeStream::connect(endpoint, "", rate, config()).await {
        Ok(stream) => stream,
        Err(e) => {
            c.check(
                "transcribe-stream connects and sends the sample rate",
                false,
                e.to_string(),
            );
            return;
        }
    };
    c.check(
        "transcribe-stream connects and sends the sample rate",
        true,
        format!("{rate} Hz"),
    );
    // 100 ms chunks, the size a microphone callback would deliver.
    let chunk = (rate as usize / 10) * 2;
    let mut sent = Ok(());
    for part in pcm.chunks(chunk) {
        sent = stream.send_pcm(part).await;
        if sent.is_err() {
            break;
        }
    }
    let sent = sent.and(stream.finish().await);
    c.check(
        "transcribe-stream accepts PCM chunks and one EOS",
        sent.is_ok(),
        format!("{sent:?}"),
    );
    let mut outcome = String::from("closed without a result");
    let mut transcript = None;
    loop {
        match stream.recv().await {
            Ok(Some(TranscribeEvent::Partial { .. })) => continue,
            Ok(Some(TranscribeEvent::Final {
                transcript: t,
                provider,
            })) => {
                outcome = format!("final from {provider}: {t:?}");
                transcript = Some(t);
                break;
            }
            Ok(Some(TranscribeEvent::Error { message })) => {
                outcome = format!("error (no live STT, caller falls back to upload): {message}");
                break;
            }
            Ok(None) => break,
            Err(e) => {
                outcome = format!("stream error: {e}");
                break;
            }
        }
    }
    let _ = stream.close().await;
    match transcript {
        Some(text) => {
            let heard = text.to_lowercase();
            c.check(
                "live dictation transcribes the spoken sentence",
                heard.contains("hermes") || heard.contains("desktop") || heard.contains("hello"),
                outcome,
            );
        }
        None => c.check(
            "live dictation answers with a transcript or an explicit error",
            outcome.starts_with("error"),
            outcome,
        ),
    }
}

/// Without a desktop stack the screen cannot start; the client must get a typed refusal.
async fn display_check(client: &hermes_core::Client, c: &mut common::Checks) {
    let status = display::display_status(client, "").await;
    let Ok(status) = status else {
        c.check("display.status answers", false, format!("{status:?}"));
        return;
    };
    c.check(
        "display.status answers",
        true,
        format!(
            "supported={} installed={} running={} missing={}",
            status.supported,
            status.installed,
            status.running,
            status.missing.len()
        ),
    );
    if status.running {
        // Real screen: the first server bytes of an RFB session are its version banner.
        match display::display_observe(client, "", "").await {
            Ok(observe) => {
                let endpoint = hermes_core::backend::discover_or_spawn(None)
                    .await
                    .unwrap()
                    .endpoint
                    .clone();
                let stream =
                    hermes_core::streaming::DisplayStream::connect_observed(&endpoint, &observe, config()).await;
                let banner = match &stream {
                    Ok(s) => s
                        .recv()
                        .await
                        .map(|e| format!("{e:?}"))
                        .unwrap_or_else(|e| e.to_string()),
                    Err(e) => e.to_string(),
                };
                c.check(
                    "display socket returns the RFB banner",
                    banner.contains("82, 70, 66"),
                    banner,
                );
            }
            Err(e) => c.check("display.observe mints a ticket", false, e.to_string()),
        }
    } else {
        println!("[SKIP] real screen session: the desktop stack is not running on this host");
    }
}

#[tokio::test]
#[ignore = "needs HERMES_DESKTOP_PORT for a private test backend; uses the profile's TTS/STT"]
async fn live_streaming_sockets() {
    common::require_private_port();
    let (_rest, client) = common::backend().await;
    let backend = hermes_core::backend::discover_or_spawn(None).await.expect("backend");
    let endpoint = backend.endpoint.clone();
    std::mem::forget(backend);
    let mut c = common::Checks::default();
    if let Some((rate, pcm)) = speak(&endpoint, &mut c).await {
        transcribe(&endpoint, rate, &pcm, &mut c).await;
    }
    display_check(&client, &mut c).await;
    c.finish();
}
