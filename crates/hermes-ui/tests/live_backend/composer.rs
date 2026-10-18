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

/// Move the pointer over the message whose text starts with `text` (row actions show on
/// hover only, and are not clickable while hidden).
async fn hover_message(ui: &AppWindow, text: &str) -> bool {
    sleep(150).await;
    let Some(e) = ElementHandle::find_by_accessible_label(ui, text).next() else {
        return false;
    };
    let (p, s) = (e.absolute_position(), e.size());
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0),
    });
    sleep(200).await;
    true
}

/// Sweep the pointer up the transcript's right side (where user bubbles sit) from the
/// bottom until an "Edit message" action shows: hovers the newest user message.
async fn hover_last_user_message(ui: &AppWindow) -> bool {
    let size = ui.window().size().to_logical(ui.window().scale_factor());
    let mut y = size.height - 160.0;
    while y > 60.0 {
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(size.width * 0.72, y),
        });
        sleep(40).await;
        if by_label(ui, "Edit message").is_some() {
            return true;
        }
        y -= 16.0;
    }
    false
}

async fn stored_users(id: &str) -> Vec<String> {
    backend_rest("GET", &format!("/api/sessions/{id}/messages"), None)
        .await
        .as_ref()
        .map(user_texts)
        .unwrap_or_default()
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
    // Actions are hidden (and not clickable) until the row is hovered.
    let hidden_before = by_label(ui, "Copy message").is_none();
    let hovered = hover_message(ui, &q2).await;
    // The testing backend's clipboard is in-process: nothing reaches the desktop's.
    let copied = click(ui, "Copy message").await && wait_until(3, || by_label(ui, "Copied").is_some()).await;
    record(
        log,
        "composer: Copy message appears on hover and switches to Copied",
        hidden_before && hovered && copied,
        format!("hidden-before-hover={hidden_before} hovered={hovered} copied={copied}"),
    );

    // ── Edit and resend ─────────────────────────────────────────────────────────
    let edit_id = seed(
        &format!("Composer edit {tag}"),
        &[(&q1, "Seeded answer one."), (&q2, "Seeded answer two.")],
    )
    .await;
    let opened = open(ui, &edit_id, 4).await;
    let rows_before = st.get_rows().row_count();
    // Opening restores the chat's saved draft a moment later; type after that.
    sleep(800).await;
    st.set_draft("half typed".into());
    sleep(200).await;
    let clicked = hover_message(ui, &q1).await && click(ui, "Edit message").await;
    let editing = wait_until(3, || st.get_editing_row() == 0 && st.get_draft() == q1.as_str()).await;
    record(
        log,
        "composer: Edit puts the message in the composer in edit mode",
        opened && clicked && editing,
        format!("editing-row={} draft={:?}", st.get_editing_row(), st.get_draft()),
    );
    // Esc cancels and gives back what was typed before.
    press(ui, Key::Escape);
    let cancelled = wait_until(3, || st.get_editing_row() == -1 && st.get_draft() == "half typed").await;
    record(
        log,
        "composer: Esc cancels editing and restores the earlier draft",
        cancelled,
        format!("editing-row={} draft={:?}", st.get_editing_row(), st.get_draft()),
    );
    st.set_draft("".into());
    // Enter on an unchanged message sends nothing and cuts nothing.
    let _ = hover_message(ui, &q1).await && click(ui, "Edit message").await;
    wait_until(3, || st.get_editing_row() == 0).await;
    press(ui, Key::Return);
    let ended = wait_until(3, || st.get_editing_row() == -1).await;
    sleep(1000).await;
    let users = stored_users(&edit_id).await;
    record(
        log,
        "composer: an unchanged edit sends nothing",
        ended && users == vec![q1.clone(), q2.clone()] && st.get_rows().row_count() == rows_before,
        format!(
            "ended={ended} stored users={users:?} rows={}",
            st.get_rows().row_count()
        ),
    );
    // A changed first turn also removes the later turn: the first Enter only explains.
    let _ = hover_message(ui, &q1).await && click(ui, "Edit message").await;
    wait_until(3, || st.get_editing_row() == 0).await;
    let edited = format!("Edited first {tag}");
    st.set_draft(edited.clone().into());
    sleep(150).await;
    press(ui, Key::Return);
    let asked = wait_until(5, || st.get_edit_note().contains("1 later message")).await;
    sleep(800).await;
    let users_after_first = stored_users(&edit_id).await;
    record(
        log,
        "composer: an edit that removes later turns asks first and sends nothing yet",
        asked
            && st.get_editing_row() == 0
            && st.get_draft() == edited.as_str()
            && users_after_first == vec![q1.clone(), q2.clone()],
        format!(
            "note={:?} draft={:?} stored users={users_after_first:?}",
            st.get_edit_note(),
            st.get_draft()
        ),
    );
    screenshot(ui, "composer-edit-confirm");
    press(ui, Key::Return);
    let local_cut = wait_until(5, || {
        let r = st.get_rows();
        r.row_count() >= 1 && r.row_data(0).is_some_and(|x| x.kind == 0 && x.text == edited.as_str())
    })
    .await;
    // The backend has no model, so the reply fails; the cut and the new turn must persist.
    sleep(3000).await;
    let users = stored_users(&edit_id).await;
    record(
        log,
        "composer: the second Enter replaces the turn and what followed (backend read-back)",
        local_cut && users == vec![edited.clone()],
        format!(
            "rows {rows_before}→{} local-cut={local_cut} stored users={users:?}",
            st.get_rows().row_count()
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
    // An edit the backend refuses (this turn failed without a model, so the runtime never
    // kept it: 4018) must change nothing and give the text back.
    wait_until(30, || !st.get_running()).await;
    let before = stored_users(&file_id).await;
    let clicked = hover_last_user_message(ui).await && click(ui, "Edit message").await;
    let editing = wait_until(3, || {
        st.get_editing_row() >= 0 && st.get_draft() == "Read the attached files"
    })
    .await;
    let refused_text = format!("Refused edit {tag}");
    st.set_draft(refused_text.clone().into());
    sleep(150).await;
    press(ui, Key::Return);
    let restored = wait_until(10, || {
        st.get_draft() == refused_text.as_str() && !st.get_banner().is_empty()
    })
    .await;
    let after = stored_users(&file_id).await;
    record(
        log,
        "composer: an edit the backend refuses cuts nothing and returns the text",
        clicked && editing && restored && after == before,
        format!(
            "restored={restored} banner={:?} stored before={} after={}",
            st.get_banner(),
            before.len(),
            after.len()
        ),
    );
    st.set_draft("".into());

    // A saved turn that carried a file: shown as words + 📎, matched to its stored refs.
    let ref_turn = "@file:docs/fixture-notes.txt\n\nCheck the fixture notes";
    let ref_id = seed(&format!("Composer ref edit {tag}"), &[(ref_turn, "Seeded answer.")]).await;
    let opened = open(ui, &ref_id, 2).await;
    sleep(800).await;
    let shown = st
        .get_rows()
        .row_data(0)
        .map(|r| r.text.to_string())
        .unwrap_or_default();
    let clicked = hover_last_user_message(ui).await && click(ui, "Edit message").await;
    let editing = wait_until(3, || {
        st.get_editing_row() == 0 && st.get_draft() == "Check the fixture notes"
    })
    .await;
    let new_text = format!("Summarise the notes instead {tag}");
    st.set_draft(new_text.clone().into());
    sleep(150).await;
    press(ui, Key::Return);
    sleep(3000).await;
    let users = stored_users(&ref_id).await;
    record(
        log,
        "composer: a saved turn with a file ref shows 📎 and can be edited",
        opened
            && shown == "Check the fixture notes\n📎 fixture-notes.txt"
            && clicked
            && editing
            && users == vec![new_text.clone()],
        format!(
            "shown={shown:?} editing={editing} stored users={users:?} banner={:?}",
            st.get_banner()
        ),
    );

    // Switching from a scrolled-up chat lands the next chat on its newest turn.
    let opened = open(ui, &long_id, 28).await;
    sleep(400).await;
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
    let up = wait_until(3, || by_label(ui, "Scroll to bottom").is_some()).await;
    let switched = open(ui, &hist_id, 4).await;
    sleep(600).await;
    let at_end = by_label(ui, "Scroll to bottom").is_none();
    record(
        log,
        "composer: switching from a scrolled-up chat lands on the next chat's newest turn",
        opened && up && switched && at_end,
        format!("scrolled-up={up} switched={switched} at-end={at_end}"),
    );

    for id in [long_id, hist_id, edit_id, file_id, ref_id] {
        let _ = backend_rest("DELETE", &format!("/api/sessions/{id}"), None).await;
    }
}
