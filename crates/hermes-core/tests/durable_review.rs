//! Data-integrity regressions for durable state.
use hermes_core::{commands::PendingPrompt, durable::DurableState};

fn dir(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("durable-review-{}-{label}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn stale_writer_must_not_silently_overwrite_another_instances_queue() {
    let dir = dir("writers");
    let path = dir.join("state.json");
    let mut one = DurableState::open(&path, "fixture-backend").unwrap();
    let mut two = DurableState::open(&path, "fixture-backend").unwrap();
    one.set_queue(
        "chat-a",
        vec![PendingPrompt {
            text: "one".into(),
            display: "one".into(),
        }],
        true,
    )
    .unwrap();
    two.set_queue(
        "chat-b",
        vec![PendingPrompt {
            text: "two".into(),
            display: "two".into(),
        }],
        true,
    )
    .unwrap();
    one.save().unwrap();
    assert!(
        two.save().is_err(),
        "stale snapshot silently overwrote another instance's queued prompt"
    );
    assert_eq!(
        DurableState::open(&path, "fixture-backend")
            .unwrap()
            .entries("chat-a")
            .len(),
        1
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_schema_error_must_not_echo_private_prompt_values() {
    let dir = dir("schema");
    let path = dir.join("state.json");
    std::fs::write(&path,br#"{"version":1,"backends":{"fixture":{"chats":{"chat":{"paused":"fixture-private-prompt","entries":[],"results":[]}}}}}"#).unwrap();
    let error = DurableState::open(&path, "fixture").unwrap_err();
    assert!(
        !format!("{error:?} {error}").contains("fixture-private-prompt"),
        "schema error exposed saved private data"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
