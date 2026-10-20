//! Input contracts taken from tui_gateway/agent_callbacks.py, not provider calls.
use hermes_core::input_requests::{self, InputKind};
use serde_json::json;

#[test]
fn vault_verification_code_uses_existing_masked_card_and_empty_decline_contract() {
    assert!(input_requests::handles("vault.code"));
    let prompt = input_requests::parse(
        "fixture-request",
        "vault.code",
        &json!({
            "session_id":"fixture-chat", "site":"example.test", "hint":"One-time verification code"
        }),
    )
    .expect("verification-code prompt");
    assert_eq!(prompt.kind, InputKind::Masked);
    assert_eq!(prompt.session_id, "fixture-chat");
    assert!(prompt.detail.contains("example.test"));
    assert!(prompt.detail.contains("One-time verification code"));
    assert_eq!(input_requests::skip_response(&prompt), json!({"value":""}));
    assert!(prompt.answers.is_empty());
    assert_eq!(
        input_requests::value_response("fixture-code"),
        json!({"value":"fixture-code"})
    );
}
