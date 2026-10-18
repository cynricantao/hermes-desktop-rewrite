//! Sidebar chat actions: pin, rename, archive (REST `PATCH /api/sessions/{id}`), pinned
//! flags for grouping, and message-content search hits (`GET /api/sessions/search`).

use hermes_core::api::sessions::{self, SessionListQuery, SessionPatch, SessionSearchQuery};
use hermes_core::protocol::SessionListRow;
use slint::ComponentHandle;

use super::{Cmd, Controller, ui};
use crate::AppState;

/// Wire the row-action callbacks (UI thread).
pub(super) fn wire(ui_handle: &crate::AppWindow, tx: &tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let st = ui_handle.global::<AppState>();
    let t = tx.clone();
    st.on_pin_session(move |id, on| {
        let _ = t.send(Cmd::PinSession(id.into(), on));
    });
    let t = tx.clone();
    st.on_rename_session(move |id, title| {
        let _ = t.send(Cmd::RenameSession(id.into(), title.into()));
    });
    let t = tx.clone();
    st.on_archive_session(move |id| {
        let _ = t.send(Cmd::ArchiveSession(id.into()));
    });
}

/// Shortest query sent to the backend's full-text search (title filtering is local).
const MIN_SEARCH: usize = 2;

impl Controller {
    async fn patch_session(&mut self, id: &str, patch: SessionPatch, what: &str) -> bool {
        let Some(rest) = self.rest() else {
            self.banner(format!("Could not {what}: the backend's web API is unavailable."), true);
            return false;
        };
        match sessions::session_patch(&rest, id, &patch).await {
            Ok(_) => true,
            Err(e) => {
                self.banner(format!("Could not {what}: {e}"), true);
                false
            }
        }
    }

    pub(super) async fn pin_session(&mut self, id: String, on: bool) {
        let patch = SessionPatch {
            pinned: Some(on),
            ..Default::default()
        };
        if self
            .patch_session(&id, patch, if on { "pin the chat" } else { "unpin the chat" })
            .await
        {
            if on {
                self.pinned.insert(id);
            } else {
                self.pinned.remove(&id);
            }
            self.publish_sessions();
        }
    }

    pub(super) async fn rename_session(&mut self, id: String, title: String) {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return self.banner("A chat title cannot be empty.".into(), true);
        }
        let patch = SessionPatch {
            title: Some(title.clone()),
            ..Default::default()
        };
        if self.patch_session(&id, patch, "rename the chat").await {
            if let Some(row) = self.all_sessions.iter_mut().find(|r| r.id == id) {
                row.title = title.clone();
            }
            if self.stored.as_deref() == Some(id.as_str()) {
                self.transcript.title = Some(title.clone());
                ui(&self.weak, move |w| {
                    w.global::<AppState>().set_session_title(title.into())
                });
            }
            self.publish_sessions();
        }
    }

    pub(super) async fn archive_session(&mut self, id: String) {
        let patch = SessionPatch {
            archived: Some(true),
            ..Default::default()
        };
        if self.patch_session(&id, patch, "archive the chat").await {
            if self.stored.as_deref() == Some(id.as_str()) {
                Box::pin(self.handle_cmd(Cmd::NewChat)).await;
            }
            self.all_sessions.retain(|r| r.id != id);
            self.publish_sessions();
            self.banner("Chat archived. Restore it from Settings › Sessions.".into(), false);
        }
    }

    /// Delete a stored chat. Runtimes this window holds for it are stopped and closed first
    /// (the backend refuses to delete a chat with a live runtime), as the Electron app does.
    pub(super) async fn delete_session(&mut self, id: String) {
        let Some(rest) = self.rest() else {
            return self.banner(
                "Could not delete the chat: the backend's web API is unavailable.".into(),
                true,
            );
        };
        let mut runtimes: Vec<String> = self
            .runtime_stores
            .iter()
            .filter(|(_, stored)| **stored == id)
            .map(|(runtime, _)| runtime.clone())
            .collect();
        if self.stored.as_deref() == Some(id.as_str()) {
            runtimes.extend(self.session.clone());
            Box::pin(self.handle_cmd(Cmd::NewChat)).await;
        }
        runtimes.sort();
        runtimes.dedup();
        for sid in &runtimes {
            let _ = self
                .client
                .request("session.interrupt", serde_json::json!({"session_id": sid}))
                .await;
            let _ = self
                .client
                .request("session.close", serde_json::json!({"session_id": sid}))
                .await;
            self.runtime_stores.remove(sid);
        }
        match sessions::session_delete_rest(&rest, &id).await {
            Ok(_) => {
                self.all_sessions.retain(|r| r.id != id);
                self.search_hits.retain(|r| r.id != id);
                self.pinned.remove(&id);
                self.drafts.remove(&id);
                self.inputs
                    .retain(|p| p.session_id != id && !runtimes.contains(&p.session_id));
                self.publish_sessions();
            }
            Err(e) => self.banner(format!("Could not delete the chat: {e}"), true),
        }
    }

    /// Pinned flags for the listed chats. `session.list` does not carry them, so they come
    /// from the REST list (most recent 100; older pinned chats show unpinned).
    pub(super) async fn refresh_pins(&mut self) {
        let Some(rest) = self.rest() else { return };
        let q = SessionListQuery {
            order: "recent".into(),
            limit: Some(100),
            ..Default::default()
        };
        match sessions::sessions_list(&rest, &q).await {
            Ok(page) => {
                self.pinned = page.sessions.into_iter().filter(|r| r.pinned).map(|r| r.id).collect();
            }
            Err(e) => tracing::warn!("pinned sessions read failed: {e}"),
        }
    }

    /// Sidebar query: titles filter at once; message content is searched on the backend
    /// and shown under "Found in messages". A late answer for an older query is dropped.
    pub(super) async fn search_sessions(&mut self, q: String) {
        self.query = q.clone();
        self.search_hits.clear();
        self.publish_sessions();
        let q = q.trim().to_owned();
        if q.chars().count() < MIN_SEARCH {
            return;
        }
        let Some(rest) = self.rest() else { return };
        let query = SessionSearchQuery {
            q: q.clone(),
            limit: Some(20),
            ..Default::default()
        };
        let hits = match sessions::sessions_search(&rest, &query).await {
            Ok(r) => r.results,
            Err(e) => {
                tracing::warn!("session search failed: {e}");
                return;
            }
        };
        if self.query.trim() != q {
            return;
        }
        self.search_hits = hits
            .into_iter()
            .filter(|h| !h.archived)
            .map(|h| {
                let id = if h.session_id.is_empty() { h.id } else { h.session_id };
                // The backend marks the match as >>>word<<< for highlighting; plain text here.
                let snippet = h.snippet.replace(">>>", "").replace("<<<", "");
                let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                SessionListRow {
                    id,
                    title: h.title.unwrap_or_default(),
                    preview: if snippet.is_empty() {
                        h.preview.unwrap_or_default()
                    } else {
                        snippet
                    },
                    started_at: h.started_at.unwrap_or_default(),
                    ..Default::default()
                }
            })
            .collect();
        self.publish_sessions();
    }
}
