//! Credential formatting must be safe without contacting a provider.
use hermes_core::api::voice::VoiceConfig;
use serde_json::json;

#[test]
fn nested_voice_config_debug_never_contains_provider_credentials() {
    let config: VoiceConfig = serde_json::from_value(json!({
        "ok": true,
        "stt": {"mode": "direct", "api_key": "fixture-stt-private-value", "extra_body": {"authorization": "fixture-extra-private-value"}},
        "tts": {"mode": "direct", "api_key": "fixture-tts-private-value", "base_url": "https://fixture-user:fixture-password@example.test/?token=fixture-query-private-value"}
    })).unwrap();
    for rendered in [format!("{config:?}"), format!("{config:#?}")] {
        for secret in [
            "fixture-stt-private-value",
            "fixture-tts-private-value",
            "fixture-extra-private-value",
            "fixture-password",
            "fixture-query-private-value",
        ] {
            assert!(!rendered.contains(secret), "voice debug output exposed a credential");
        }
    }
    assert_eq!(config.stt.api_key.as_deref(), Some("fixture-stt-private-value"));
}
