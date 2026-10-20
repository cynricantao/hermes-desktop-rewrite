//! Durable queue / failed-prompt / background-result state.
//!
//! Everything here uses an explicit temporary path and a synthetic backend
//! identity. No test reads prefs, config, the real backend or the session store.

use std::fs;
use std::path::{Path, PathBuf};

use hermes_core::commands::{PendingPrompt, PromptQueues};
use hermes_core::durable::{
    self, DurableError, DurableState, MAX_FILE_BYTES, MAX_PROMPT_BYTES, MAX_QUEUE_ENTRIES, MAX_RESULTS_PER_CHAT,
};

fn prompt(text: &str) -> PendingPrompt {
    PendingPrompt {
        text: text.into(),
        display: text.into(),
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("durable-state-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn state(path: &Path, backend: &str) -> DurableState {
    DurableState::open(path, backend).unwrap()
}

#[test]
fn restart_restores_queue_pause_and_pending_without_resending() {
    let dir = temp("restart");
    let path = dir.join("nested").join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-test-root"), Some("work"));
    let mut store = state(&path, &backend);
    store
        .set_queue(
            "stored-a",
            vec![
                prompt("first"),
                PendingPrompt {
                    text: "PRIVATE_SKILL_BODY".into(),
                    display: "/skill".into(),
                },
            ],
            false,
        )
        .unwrap();
    store
        .record_pending("stored-a", prompt("in-flight"), Some("socket closed"))
        .unwrap();
    store
        .record_result("stored-a", "task-1", "task-1", "background answer", false)
        .unwrap();
    store.save().unwrap();

    let mut restored = state(&path, &backend);
    assert!(restored.force_recovery_pauses().unwrap());
    assert!(restored.paused("stored-a"));
    assert_eq!(restored.entries("stored-a").len(), 2);
    assert_eq!(restored.entries("stored-a")[1].display, "/skill");
    assert_eq!(
        restored.pending("stored-a").unwrap().error.as_deref(),
        Some("socket closed")
    );
    assert_eq!(restored.results("stored-a").len(), 1);

    // Recovery itself never executes anything: the queue is byte-for-byte intact.
    restored.save().unwrap();
    let again = state(&path, &backend);
    assert!(again.paused("stored-a"));
    assert_eq!(again.entries("stored-a").len(), 2);
    assert!(again.pending("stored-a").is_some());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn scope_is_bound_to_backend_and_stored_chat_not_runtime() {
    let dir = temp("scope");
    let path = dir.join("state.json");
    let a = durable::backend_id(Path::new("/tmp/hermes-a"), Some("default"));
    let b = durable::backend_id(Path::new("/tmp/hermes-b"), Some("default"));
    assert_eq!(a, durable::backend_id(Path::new("/tmp/hermes-a"), None));
    assert_ne!(a, b);
    assert_ne!(a, durable::backend_id(Path::new("/tmp/hermes-a"), Some("work")));

    let mut one = state(&path, &a);
    one.set_queue("same-stored-id", vec![prompt("backend-a")], false)
        .unwrap();
    one.save().unwrap();

    let mut two = state(&path, &b);
    assert!(two.entries("same-stored-id").is_empty());
    two.set_queue("same-stored-id", vec![prompt("backend-b")], false)
        .unwrap();
    two.set_queue("other-chat", vec![prompt("other")], false).unwrap();
    two.save().unwrap();

    let reread_a = state(&path, &a);
    assert_eq!(reread_a.entries("same-stored-id"), vec![prompt("backend-a")]);
    assert!(reread_a.entries("other-chat").is_empty());
    let reread_b = state(&path, &b);
    assert_eq!(reread_b.entries("same-stored-id"), vec![prompt("backend-b")]);
    assert_eq!(reread_b.entries("other-chat"), vec![prompt("other")]);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn corrupt_version_schema_and_oversized_files_fail_closed() {
    let dir = temp("corrupt");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-corrupt"), None);

    for (name, body, matches) in [
        ("invalid-json", b"{not json".to_vec(), 0u8),
        ("bad-version", br#"{"version":99,"backends":{}}"#.to_vec(), 1),
        (
            "unknown-field",
            br#"{"version":1,"backends":{},"private_prompt":"PRIVATE_SKILL_BODY"}"#.to_vec(),
            2,
        ),
    ] {
        fs::write(&path, &body).unwrap();
        let err = DurableState::open(&path, &backend).unwrap_err();
        match matches {
            0 => assert!(matches!(err, DurableError::Corrupt(_)), "{name}: {err}"),
            1 => assert!(
                matches!(err, DurableError::UnsupportedVersion { found: 99 }),
                "{name}: {err}"
            ),
            _ => assert!(matches!(err, DurableError::Schema(_)), "{name}: {err}"),
        }
        assert_eq!(fs::read(&path).unwrap(), body, "{name} file was overwritten");
    }

    let oversized = vec![b' '; MAX_FILE_BYTES as usize + 1];
    fs::write(&path, &oversized).unwrap();
    let err = DurableState::open(&path, &backend).unwrap_err();
    assert!(matches!(err, DurableError::TooLarge { .. }), "{err}");
    assert_eq!(fs::read(&path).unwrap().len(), oversized.len());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn failed_and_unknown_inflight_prompts_stay_paused_until_explicit_resume() {
    let dir = temp("inflight");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-inflight"), None);
    let mut store = state(&path, &backend);
    store.set_queue("chat", vec![prompt("queued")], false).unwrap();
    store.record_pending("chat", prompt("unknown-submit"), None).unwrap();
    store.save().unwrap();

    let mut restored = state(&path, &backend);
    restored.force_recovery_pauses().unwrap();
    assert!(restored.paused("chat"));
    assert_eq!(restored.pending("chat").unwrap().prompt, prompt("unknown-submit"));
    assert_eq!(restored.entries("chat"), vec![prompt("queued")]);
    // Simulate the UI's explicit /queue resume; only then may the queue drain.
    restored.set_paused("chat", false).unwrap();
    assert!(!restored.paused("chat"));
    assert_eq!(restored.entries("chat").len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn queue_management_round_trips_edits_pops_and_clear() {
    let dir = temp("manage");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-manage"), None);
    let mut q = PromptQueues::default();
    for text in ["one", "two", "three"] {
        q.push("chat", prompt(text));
    }
    q.manage("chat", "move 3 1").unwrap();
    q.manage("chat", "edit 2 changed prompt").unwrap();
    q.manage("chat", "rm 3").unwrap();
    let mut store = state(&path, &backend);
    store.set_queue("chat", q.entries("chat"), false).unwrap();
    store.save().unwrap();

    let mut restored = state(&path, &backend);
    restored.set_paused("chat", true).unwrap();
    restored.save().unwrap();
    let mut restored = state(&path, &backend);
    assert_eq!(
        restored.entries("chat"),
        vec![prompt("three"), prompt("changed prompt")]
    );
    assert!(restored.paused("chat"));
    let mut q = PromptQueues::default();
    q.set_entries("chat", restored.entries("chat"));
    assert_eq!(q.pop("chat"), Some(prompt("three")));
    restored.set_queue("chat", q.entries("chat"), false).unwrap();
    restored.save().unwrap();
    let mut restored = state(&path, &backend);
    assert_eq!(restored.entries("chat"), vec![prompt("changed prompt")]);
    restored.set_queue("chat", Vec::new(), false).unwrap();
    restored.save().unwrap();
    assert!(state(&path, &backend).entries("chat").is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn background_results_are_deduped_persisted_and_taken_once() {
    let dir = temp("results");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-results"), None);
    let mut store = state(&path, &backend);
    assert!(store.record_result("chat", "id-1", "task", "answer", false).unwrap());
    assert!(!store.record_result("chat", "id-1", "task", "answer", false).unwrap());
    assert!(store.record_result("chat", "id-2", "task", "error", true).unwrap());
    store.set_queue("chat", vec![prompt("queued")], true).unwrap();
    store.save().unwrap();

    let mut restored = state(&path, &backend);
    let results = restored.take_results("chat").unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].text, "answer");
    assert!(results[1].error);
    assert_eq!(restored.entries("chat"), vec![prompt("queued")]);
    assert!(restored.paused("chat"));
    restored.save().unwrap();
    assert!(state(&path, &backend).results("chat").is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn state_file_and_directory_are_private_and_temp_files_are_cleaned() {
    let dir = temp("perms");
    let nested = dir.join("private");
    let path = nested.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-perms"), None);
    let mut store = state(&path, &backend);
    store.set_queue("chat", vec![prompt("text")], false).unwrap();
    store.save().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let leftovers: Vec<_> = fs::read_dir(&nested)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unwritable_save_is_an_error_keeps_the_old_file_and_cleans_up() {
    let dir = temp("unwritable");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-unwritable"), None);
    let mut store = state(&path, &backend);
    store.set_queue("chat", vec![prompt("old")], false).unwrap();
    store.save().unwrap();
    let old = fs::read(&path).unwrap();

    // Rename onto a directory always fails (works even when the tests run as root).
    let blocked = dir.join("blocked.json");
    let mut blocked_store = DurableState::open(&blocked, &backend).unwrap();
    fs::create_dir(&blocked).unwrap();
    blocked_store.set_queue("chat", vec![prompt("new")], false).unwrap();
    let err = blocked_store.save().unwrap_err();
    assert!(
        matches!(err, DurableError::Invalid("state path (not a regular file)")),
        "{err}"
    );

    // The original file is untouched and no temp file survives a failed save.
    assert_eq!(fs::read(&path).unwrap(), old);
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());

    // A read-only parent is the ordinary "disk refuses the write" case.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let mut changed = state(&path, &backend);
        changed.set_queue("chat", vec![prompt("changed")], false).unwrap();
        if fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("probe"))
            .is_err()
        {
            assert!(changed.save().is_err());
            assert_eq!(fs::read(&path).unwrap(), old);
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn limits_and_ids_are_explicit_and_debug_redacts_text() {
    let dir = temp("limits");
    let path = dir.join("state.json");
    let backend = durable::backend_id(Path::new("/tmp/hermes-limits"), None);
    let mut store = state(&path, &backend);
    let too_many: Vec<_> = (0..=MAX_QUEUE_ENTRIES).map(|i| prompt(&format!("p{i}"))).collect();
    let err = store.set_queue("chat", too_many, false).unwrap_err();
    assert!(matches!(err, DurableError::TooLarge { .. }), "{err}");
    let err = store
        .set_queue("chat", vec![prompt(&"x".repeat(MAX_PROMPT_BYTES + 1))], false)
        .unwrap_err();
    assert!(matches!(err, DurableError::TooLarge { .. }), "{err}");
    for i in 0..MAX_RESULTS_PER_CHAT {
        store
            .record_result("chat", &format!("id-{i}"), "task", "answer", false)
            .unwrap();
    }
    let err = store
        .record_result("chat", "too-many", "task", "answer", false)
        .unwrap_err();
    assert!(matches!(err, DurableError::TooLarge { .. }), "{err}");
    let err = store
        .record_result("chat", "PRIVATE_SKILL_BODY", "task", "answer", false)
        .unwrap_err();
    assert!(matches!(err, DurableError::TooLarge { .. }), "{err}");

    store
        .record_pending("chat", prompt("PRIVATE_SKILL_BODY"), None)
        .unwrap();
    let debug = format!("{store:?} {:?}", prompt("PRIVATE_SKILL_BODY"));
    assert!(
        !debug.contains("PRIVATE_SKILL_BODY"),
        "prompt leaked through Debug: {debug}"
    );
    let id_a = durable::result_id("task", Some(7), "answer", "");
    let id_b = durable::result_id("task", Some(7), "other", "error");
    assert_eq!(id_a, id_b);
    assert_ne!(id_a, durable::result_id("task", None, "answer", ""));
    let _ = fs::remove_dir_all(dir);
}
