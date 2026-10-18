//! Composer daily-driver features against the isolated test backend, no model needed:
//! scroll-to-bottom, ↑/↓ input history, copy message, edit-and-resend and file attach.
use super::*;
use serde_json::json;

async fn seed(title: &str, turns: &[(&str, &str)]) -> String {
    let mut messages = Vec::new();
    for (q, a) in turns {
        messages.push(json!({"role": "user", "content": q}));
        messages.push(json!({"role": "assistant", "content": a}));
    }
    let r = backend_request(
        "session.create",
        json!({"source": "desktop", "title": title, "messages": messages}),
    )
    .await
    .unwrap_or_default();
    let _ = backend_request("session.close", json!({"session_id": r["session_id"]})).await;
    r["stored_session_id"].as_str().unwrap_or_default().to_owned()
}

async fn open(ui: &AppWindow, id: &str, rows_at_least: usize) -> bool {
    let st = ui.global::<AppState>();
    if !wait_until(15, || st.get_sessions().iter().any(|i| i.id == id)).await {
        return false;
    }
    st.invoke_open_session(id.into());
    wait_until(15, || st.get_rows().row_count() >= rows_at_least).await
}

fn user_texts(v: &serde_json::Value) -> Vec<String> {
    v["messages"]
        .as_array()
        .or_else(|| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|m| m["role"] == "user")
                .map(|m| {
                    m["content"]
                        .as_str()
                        .or_else(|| m["text"].as_str())
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();
    let tag = format!("{:x}", std::process::id());

    // ── Scroll-to-bottom ─────────────────────────────────────────────────────────
    let long: Vec<(String, String)> = (0..14)
        .map(|i| {
            (
                format!("Long fixture question {i}"),
                format!("Long fixture answer {i}.\n\n{}", "A line of padding text. ".repeat(12)),
            )
        })
        .collect();
    let pairs: Vec<(&str, &str)> = long.iter().map(|(q, a)| (q.as_str(), a.as_str())).collect();
    let long_id = seed(&format!("Composer long {tag}"), &pairs).await;
    let opened = open(ui, &long_id, 28).await;
    sleep(400).await;
    let hidden_at_end = by_label(ui, "Scroll to bottom").is_none();
    screenshot(ui, "composer-opened");
    sleep(800).await;
    screenshot(ui, "composer-opened-settled");
    let size = ui.window().size().to_logical(ui.window().scale_factor());
    let at = slint::LogicalPosition::new(size.width * 0.6, size.height * 0.4);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    for _ in 0..8 {
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.0,
            delta_y: 400.0,
        });
        sleep(60).await;
    }
    let shown = wait_until(3, || by_label(ui, "Scroll to bottom").is_some()).await;
    screenshot(ui, "composer-scrolled");
    let clicked = click(ui, "Scroll to bottom").await;
    let gone = wait_until(3, || by_label(ui, "Scroll to bottom").is_none()).await;
    record(
        log,
        "composer: scroll-to-bottom appears when scrolled up and returns to the end",
        opened && hidden_at_end && shown && clicked && gone,
        format!(
            "opened={opened} hidden-at-end={hidden_at_end} shown-after-scroll={shown} clicked={clicked} gone={gone}"
        ),
    );

    // ── ↑/↓ history ─────────────────────────────────────────────────────────────
    let (q1, q2) = (format!("History first {tag}"), format!("History second {tag}"));
    let hist_id = seed(
        &format!("Composer history {tag}"),
        &[(&q1, "Seeded answer one."), (&q2, "Seeded answer two.")],
    )
    .await;
    let opened = open(ui, &hist_id, 4).await;
    if let Some(input) = by_label(ui, "Message") {
        input.single_click(PointerEventButton::Left).await;
    }
    st.set_draft("".into());
    sleep(200).await;
    let mut seen = Vec::new();
    for k in [Key::UpArrow, Key::UpArrow, Key::UpArrow, Key::DownArrow, Key::DownArrow] {
        press(ui, k);
        sleep(250).await;
        seen.push(st.get_draft().to_string());
    }
    let want = vec![q2.clone(), q1.clone(), q1.clone(), q2.clone(), String::new()];
    record(
        log,
        "composer: ↑/↓ walks this chat's messages and restores the empty draft",
        opened && seen == want,
        format!("seen={seen:?}"),
    );
    press(ui, Key::UpArrow);
    sleep(250).await;
    type_text(ui, "!");
    sleep(250).await;
    let typed_ends = !st.get_browsing() && st.get_draft().ends_with('!');
    record(
        log,
        "composer: typing ends history browsing",
        typed_ends,
        format!("browsing={} draft={:?}", st.get_browsing(), st.get_draft()),
    );
    st.set_draft("".into());

    // ── Copy message ─────────────────────────────────────────────────────────────
    // The testing backend's clipboard is in-process: nothing reaches the desktop's.
    let copied = click(ui, "Copy message").await && wait_until(3, || by_label(ui, "Copied").is_some()).await;
    record(
        log,
        "composer: Copy message switches to Copied",
        copied,
        format!("copied={copied}"),
    );

    // ── Edit and resend ─────────────────────────────────────────────────────────
    let edit_id = seed(
        &format!("Composer edit {tag}"),
        &[(&q1, "Seeded answer one."), (&q2, "Seeded answer two.")],
    )
    .await;
    let opened = open(ui, &edit_id, 4).await;
    let rows_before = st.get_rows().row_count();
    // Row 0 is the first user turn; its Edit button is the first one listed.
    let clicked = click(ui, "Edit message").await;
    let editing = wait_until(3, || st.get_editing_row() == 0 && st.get_draft() == q1.as_str()).await;
    record(
        log,
        "composer: Edit puts the message in the composer in edit mode",
        opened && clicked && editing,
        format!("editing-row={} draft={:?}", st.get_editing_row(), st.get_draft()),
    );
    // Esc cancels.
    press(ui, Key::Escape);
    let cancelled = wait_until(3, || st.get_editing_row() == -1 && st.get_draft().is_empty()).await;
    record(
        log,
        "composer: Esc cancels editing",
        cancelled,
        format!("editing-row={}", st.get_editing_row()),
    );
    let _ = click(ui, "Edit message").await;
    wait_until(3, || st.get_editing_row() == 0).await;
    let edited = format!("Edited first {tag}");
    st.set_draft(edited.clone().into());
    sleep(150).await;
    press(ui, Key::Return);
    let local_cut = wait_until(5, || {
        let r = st.get_rows();
        r.row_count() >= 1 && r.row_data(0).is_some_and(|x| x.kind == 0 && x.text == edited.as_str())
    })
    .await;
    // The backend has no model, so the reply fails; the cut and the new turn must persist.
    sleep(3000).await;
    let stored = backend_rest("GET", &format!("/api/sessions/{edit_id}/messages"), None).await;
    let users = stored.as_ref().map(user_texts).unwrap_or_default();
    record(
        log,
        "composer: edit-and-resend replaces the turn and what followed (backend read-back)",
        local_cut && users == vec![edited.clone()],
        format!(
            "rows {rows_before}→{} local-cut={local_cut} stored users={users:?} err={:?}",
            st.get_rows().row_count(),
            stored.err()
        ),
    );
    screenshot(ui, "composer-edit");

    // ── File attach ─────────────────────────────────────────────────────────────
    // `HERMES_DESKTOP_PICKER_PATHS` (set by the runner) stands in for the picker.
    let paths = std::env::var("HERMES_DESKTOP_PICKER_PATHS").unwrap_or_default();
    let names: Vec<String> = paths
        .lines()
        .filter_map(|p| p.rsplit('/').next())
        .map(str::to_owned)
        .collect();
    let file_id = seed(
        &format!("Composer attach {tag}"),
        &[("Attach fixture", "Seeded answer.")],
    )
    .await;
    let opened = open(ui, &file_id, 2).await;
    let clicked = click(ui, "Add files").await;
    let chips = wait_until(10, || {
        let a: Vec<String> = st.get_attachments().iter().map(|s| s.to_string()).collect();
        !names.is_empty() && names.iter().all(|n| a.contains(n))
    })
    .await;
    record(
        log,
        "composer: + attaches the picked files as chips",
        opened && clicked && chips,
        format!(
            "picked={names:?} chips={:?}",
            st.get_attachments().iter().map(|s| s.to_string()).collect::<Vec<_>>()
        ),
    );
    st.set_draft("Read the attached files".into());
    sleep(150).await;
    press(ui, Key::Return);
    sleep(3000).await;
    let stored = backend_rest("GET", &format!("/api/sessions/{file_id}/messages"), None).await;
    let users = stored.as_ref().map(user_texts).unwrap_or_default();
    let last = users.last().cloned().unwrap_or_default();
    let refs_ok = last.contains("@file:")
        && names
            .iter()
            .filter(|n| !n.ends_with(".png"))
            .all(|n| last.contains(n.as_str()))
        && last.ends_with("Read the attached files");
    let cleared = st.get_attachments().row_count() == 0;
    record(
        log,
        "composer: the sent turn carries @file refs ahead of the text, chips clear",
        refs_ok && cleared,
        format!("last user turn={last:?} chips-cleared={cleared}"),
    );
    for id in [long_id, hist_id, edit_id, file_id] {
        let _ = backend_rest("DELETE", &format!("/api/sessions/{id}"), None).await;
    }
}
