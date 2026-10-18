//! Sidebar chat actions against the real, isolated Hermes test backend (no inference):
//! pin, rename, content search, archive and delete, each read back through REST.
use super::*;
use serde_json::json;

/// Hover a sidebar row so its actions button shows, click that button, and wait until the
/// row's actions are open. Rows move when the list re-sorts, so one retry re-reads them.
async fn open_actions(ui: &AppWindow, title: &str, id: &str) -> bool {
    let st = ui.global::<AppState>();
    for _ in 0..2 {
        sleep(200).await;
        let Some(row) = by_label(ui, title) else { continue };
        let (p, s) = (row.absolute_position(), row.size());
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0),
        });
        if click(ui, &format!("Chat actions: {title}")).await && wait_until(3, || st.get_session_menu() == id).await {
            return true;
        }
    }
    false
}

/// The app's own REST delete (snapshot + quoted identity), run outside the UI.
async fn core_delete(id: &str) -> Result<serde_json::Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = id.to_owned();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            let backend = hermes_core::backend::discover_or_spawn(None)
                .await
                .map_err(|e| e.to_string())?;
            let e = &backend.endpoint;
            let rest = hermes_core::http::Rest::new(e.host.clone(), e.port, e.token.clone());
            hermes_core::api::sessions::session_delete_rest(&rest, &id)
                .await
                .map_err(|e| e.to_string())
        });
        let _ = tx.send(result);
    });
    rx.await.map_err(|e| e.to_string())?
}

async fn detail(id: &str) -> Result<serde_json::Value, String> {
    backend_rest("GET", &format!("/api/sessions/{id}"), None).await
}

fn truthy(v: &serde_json::Value) -> bool {
    v.as_bool().unwrap_or(false) || v.as_i64().unwrap_or(0) != 0
}

pub(super) async fn section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();
    let tag = format!("{:x}", std::process::id());
    let (alpha, beta) = (
        format!("Sidebar fixture alpha {tag}"),
        format!("Sidebar fixture beta {tag}"),
    );
    let word = format!("zebraquartz{tag}");
    let mut ids = Vec::new();
    for (title, text) in [(&alpha, "Seeded alpha question"), (&beta, word.as_str())] {
        let r = backend_request(
            "session.create",
            json!({"source": "desktop", "title": title, "messages": [
                {"role": "user", "content": text},
                {"role": "assistant", "content": "Seeded fixture answer, not a model response."}
            ]}),
        )
        .await;
        let v = r.unwrap_or_default();
        ids.push(v["stored_session_id"].as_str().unwrap_or_default().to_owned());
        // Leave an ordinary stored chat behind, not a runtime held by the seeding connection.
        let _ = backend_request("session.close", json!({"session_id": v["session_id"]})).await;
    }
    let (alpha_id, beta_id) = (ids[0].clone(), ids[1].clone());
    let listed = wait_until(15, || {
        let s = st.get_sessions();
        s.iter().any(|i| i.id == alpha_id) && s.iter().any(|i| i.id == beta_id)
    })
    .await;
    record(
        log,
        "sidebar: seeded fixture chats listed",
        listed,
        format!("{alpha_id} {beta_id}"),
    );
    if !listed {
        return;
    }

    // Pin from the row's actions.
    let opened = open_actions(ui, &alpha, &alpha_id).await;
    let clicked = opened && click(ui, "Pin chat").await;
    let pinned_ui = wait_until(10, || {
        st.get_sessions()
            .iter()
            .any(|i| i.id == alpha_id && i.pinned && i.group == "Pinned")
    })
    .await;
    let read = detail(&alpha_id).await.unwrap_or_default();
    record(
        log,
        "sidebar: pin persists and leads the list",
        clicked && pinned_ui && truthy(&read["pinned"]),
        format!("opened={opened} ui={pinned_ui} rest pinned={}", read["pinned"]),
    );
    screenshot(ui, "sidebar-actions");

    // Unpin from the same menu (the pinned flag is exact for every listed chat).
    let opened = open_actions(ui, &alpha, &alpha_id).await;
    let clicked = opened && click(ui, "Unpin chat").await;
    let unpinned_ui = wait_until(10, || {
        st.get_sessions()
            .iter()
            .any(|i| i.id == alpha_id && !i.pinned && i.group != "Pinned")
    })
    .await;
    let read = detail(&alpha_id).await.unwrap_or_default();
    record(
        log,
        "sidebar: unpin persists",
        clicked && unpinned_ui && !truthy(&read["pinned"]),
        format!("opened={opened} ui={unpinned_ui} rest pinned={}", read["pinned"]),
    );

    // Esc cancels a rename without saving.
    let opened = open_actions(ui, &alpha, &alpha_id).await && click(ui, "Rename chat").await;
    sleep(150).await;
    let typed = by_label(ui, "Chat title")
        .map(|e| e.set_accessible_value("Not saved"))
        .is_some();
    press(ui, Key::Escape);
    let closed = wait_until(5, || st.get_renaming_session().is_empty()).await;
    sleep(300).await;
    let read = detail(&alpha_id).await.unwrap_or_default();
    record(
        log,
        "sidebar: Esc cancels a rename without saving",
        opened && typed && closed && read["title"] == alpha.as_str(),
        format!("closed={closed} rest title={}", read["title"]),
    );

    // Inline rename.
    let renamed = format!("Sidebar renamed {tag}");
    let opened = open_actions(ui, &alpha, &alpha_id).await && click(ui, "Rename chat").await;
    sleep(150).await;
    let typed = by_label(ui, "Chat title")
        .map(|e| e.set_accessible_value(&renamed))
        .is_some();
    press(ui, Key::Return);
    let shown = wait_until(10, || {
        st.get_sessions().iter().any(|i| i.id == alpha_id && i.title == renamed)
    })
    .await;
    let read = detail(&alpha_id).await.unwrap_or_default();
    screenshot(ui, "sidebar-rename");
    record(
        log,
        "sidebar: inline rename is saved and read back",
        opened && typed && shown && read["title"] == renamed,
        format!(
            "opened={opened} typed={typed} shown={shown} renaming={:?} rest title={}",
            st.get_renaming_session(),
            read["title"]
        ),
    );

    // Message-content search finds the chat whose title does not match: the title filter
    // first empties the list, then the backend's full-text hit appears in its own group.
    let typed = by_label(ui, "Search chats")
        .map(|e| e.set_accessible_value(&word))
        .is_some();
    let via_content = wait_until(15, || {
        st.get_sessions()
            .iter()
            .any(|i| i.id == beta_id && i.group == "Found in messages")
    })
    .await;
    let title_only_hidden = !st
        .get_sessions()
        .iter()
        .any(|i| i.id == alpha_id && i.group != "Found in messages");
    record(
        log,
        "sidebar: search finds a chat by message content",
        typed && via_content && title_only_hidden,
        format!(
            "typed={typed} query={:?} content-group={via_content} rows={:?}",
            st.get_session_search(),
            st.get_sessions()
                .iter()
                .map(|i| (i.title.to_string(), i.group.to_string()))
                .take(4)
                .collect::<Vec<_>>()
        ),
    );
    // Archive the chat straight from its "Found in messages" hit (its row shows the title
    // with the matching snippet).
    let hit_title = st
        .get_sessions()
        .iter()
        .find(|i| i.id == beta_id)
        .map(|i| i.title.to_string())
        .unwrap_or_else(|| beta.clone());
    let opened = open_actions(ui, &hit_title, &beta_id).await && click(ui, "Archive chat").await;
    let gone = wait_until(10, || !st.get_sessions().iter().any(|i| i.id == beta_id)).await;
    let read = detail(&beta_id).await.unwrap_or_default();
    record(
        log,
        "sidebar: archive (from a search hit) hides the chat and is persisted",
        opened && gone && truthy(&read["archived"]),
        format!(
            "opened={opened} gone={gone} menu={:?} search={:?} rest archived={}",
            st.get_session_menu(),
            st.get_session_search(),
            read["archived"]
        ),
    );

    if let Some(search) = by_label(ui, "Search chats") {
        search.set_accessible_value("");
    }
    wait_until(10, || st.get_sessions().iter().any(|i| i.id == alpha_id)).await;

    // Archiving the open chat leaves it and closes this window's runtime for it.
    let gamma = format!("Sidebar fixture gamma {tag}");
    let r = backend_request(
        "session.create",
        json!({"source": "desktop", "title": gamma, "messages": [
            {"role": "user", "content": "Seeded gamma question"},
            {"role": "assistant", "content": "Seeded fixture answer, not a model response."}
        ]}),
    )
    .await
    .unwrap_or_default();
    let gamma_id = r["stored_session_id"].as_str().unwrap_or_default().to_owned();
    let _ = backend_request("session.close", json!({"session_id": r["session_id"]})).await;
    wait_until(15, || st.get_sessions().iter().any(|i| i.id == gamma_id)).await;
    st.invoke_open_session(gamma_id.clone().into());
    let open_here = wait_until(10, || {
        st.get_active_session() == gamma_id && !st.get_runtime_session().is_empty()
    })
    .await;
    let runtime = st.get_runtime_session().to_string();
    let opened = open_here && open_actions(ui, &gamma, &gamma_id).await && click(ui, "Archive chat").await;
    let left = wait_until(10, || st.get_active_session().is_empty()).await;
    sleep(500).await;
    let probe = backend_request("session.interrupt", json!({"session_id": runtime})).await;
    let read = detail(&gamma_id).await.unwrap_or_default();
    record(
        log,
        "sidebar: archiving the open chat leaves it and closes its runtime",
        opened && left && probe.is_err() && truthy(&read["archived"]),
        format!(
            "open={open_here} left={left} runtime {runtime} after archive: {:?} rest archived={}",
            probe
                .map(|_| "still live")
                .map_err(|e| e.chars().take(60).collect::<String>()),
            read["archived"]
        ),
    );

    // Delete needs a second, confirming click. The chat is open here first, so the app must
    // stop and close its own runtime before the backend accepts the delete.
    st.invoke_open_session(alpha_id.clone().into());
    let open_here = wait_until(10, || {
        st.get_active_session() == alpha_id && !st.get_runtime_session().is_empty()
    })
    .await;
    let opened = open_here && open_actions(ui, &renamed, &alpha_id).await && click(ui, "Delete chat").await;
    sleep(400).await;
    let survived_first = detail(&alpha_id).await.is_ok() && st.get_sessions().iter().any(|i| i.id == alpha_id);
    let confirmed = click(ui, "Confirm delete chat").await;
    let removed = wait_until(10, || !st.get_sessions().iter().any(|i| i.id == alpha_id)).await;
    let read_after = detail(&alpha_id).await;
    let left_chat = wait_until(5, || st.get_active_session().is_empty()).await;
    record(
        log,
        "sidebar: delete asks once more, then removes the open chat",
        opened && survived_first && confirmed && removed && read_after.is_err() && left_chat,
        format!(
            "open={open_here} survived first click={survived_first} left chat={left_chat} banner={:?} rest after={}",
            st.get_banner(),
            read_after.err().unwrap_or_else(|| "still present".into())
        ),
    );
    // Deleting a chat that is already gone is an idempotent success, not an error.
    let again = core_delete(&alpha_id).await;
    record(
        log,
        "sidebar: deleting an already-deleted chat succeeds",
        again.is_ok(),
        format!("{again:?}"),
    );
    // Clean up the archived fixtures.
    for id in [&beta_id, &gamma_id] {
        let _ = core_delete(id).await;
    }
}
