//! Composer intents beyond plain send: ↑/↓ input history, edit-and-resend of an earlier
//! message, and file attachments from the "+" button. Pure rules live in
//! `hermes_core::composer`; this module talks to the backend and the window.

use std::time::Duration;

use hermes_core::attachments::{self, AttachedImage};
use hermes_core::composer::{self, Browse};
use hermes_core::protocol::TranscriptMessage;
use hermes_core::transcript::{Block, Role};
use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{Cmd, Controller, ui};
use crate::AppState;

#[derive(Debug, Clone)]
pub enum ComposerCmd {
    HistoryBack(String),
    HistoryForward,
    /// Edit the user message at this row (the controller checks it is still one); carries
    /// the composer's text at that moment, restored if the edit is cancelled.
    BeginEdit(i32, String),
    /// Send the edited text in place of the message at this row.
    EditSend(i32, String),
    CancelEdit,
    /// "+": open the desktop's file picker.
    PickFiles,
    /// Paths chosen in the picker (or handed over by a test).
    AttachPaths(Vec<String>),
    /// Reload the transcript of this runtime from the backend (after a failed edit).
    Resync(String),
}

/// A non-image file staged in the session workspace for the next turn.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StagedFile {
    pub label: String,
    /// `@file:<workspace path>`, sent ahead of the message text.
    pub ref_text: String,
}

impl ComposerState {
    /// A message was sent: the next ↑ starts from the newest turn again.
    pub fn browse_reset(&mut self) {
        self.browse.reset();
    }

    /// An edit is open: queued prompts wait, so they cannot land between the edit's
    /// target and its send (and be cut by it).
    pub fn editing(&self) -> bool {
        self.editing.is_some()
    }
}

#[derive(Debug, Default)]
pub(super) struct ComposerState {
    browse: Browse,
    /// Row and original text of the message being edited.
    editing: Option<(usize, String)>,
    /// What the composer held before Edit, restored on cancel.
    pre_edit_draft: String,
    /// The user saw "this also removes N later messages" for this (row, N, text); a second
    /// Enter with the same edit is the explicit consent to that deeper cut.
    deep_ack: Option<(usize, usize, String)>,
    pub files: Vec<StagedFile>,
}

pub(super) fn wire(ui_handle: &crate::AppWindow, tx: &tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let st = ui_handle.global::<AppState>();
    let send = |c: ComposerCmd| {
        let t = tx.clone();
        move || {
            let _ = t.send(Cmd::Composer(c.clone()));
        }
    };
    let t = tx.clone();
    st.on_history_back(move |text| {
        let _ = t.send(Cmd::Composer(ComposerCmd::HistoryBack(text.into())));
    });
    st.on_history_forward(send(ComposerCmd::HistoryForward));
    let (t, weak) = (tx.clone(), ui_handle.as_weak());
    st.on_edit_message(move |row, _| {
        let draft = weak
            .upgrade()
            .map(|w| w.global::<AppState>().get_draft().to_string())
            .unwrap_or_default();
        let _ = t.send(Cmd::Composer(ComposerCmd::BeginEdit(row, draft)));
    });
    let t = tx.clone();
    st.on_edit_send(move |row, text| {
        let _ = t.send(Cmd::Composer(ComposerCmd::EditSend(row, text.into())));
    });
    st.on_cancel_edit(send(ComposerCmd::CancelEdit));
    st.on_attach_files(send(ComposerCmd::PickFiles));
}

/// Run the first file picker found on the desktop. `Ok(None)` = the user cancelled.
fn run_picker() -> Result<Option<Vec<String>>, String> {
    // Test hook (debug builds only): newline-separated paths stand in for the picker.
    if cfg!(debug_assertions)
        && let Ok(paths) = std::env::var("HERMES_DESKTOP_PICKER_PATHS")
    {
        return Ok(Some(composer::picker_paths(&paths)));
    }
    let graphical = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if !graphical || std::env::var_os("HERMES_DESKTOP_NO_PICKER").is_some() {
        return Err("no desktop display for a file picker".into());
    }
    let candidates: [(&str, &[&str]); 2] = [
        (
            "zenity",
            &[
                "--file-selection",
                "--multiple",
                "--separator=\n",
                "--title=Attach files",
            ],
        ),
        (
            "kdialog",
            &["--getopenfilename", ".", "--multiple", "--separate-output"],
        ),
    ];
    for (bin, args) in candidates {
        match std::process::Command::new(bin).args(args).output() {
            Ok(out) if out.status.success() => {
                return Ok(Some(composer::picker_paths(&String::from_utf8_lossy(&out.stdout))));
            }
            // zenity and kdialog both exit 1 on Cancel; anything else is a failure.
            Ok(out) if out.status.code() == Some(1) => return Ok(None),
            Ok(out) => {
                return Err(format!(
                    "{bin} failed ({}): {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{bin}: {e}")),
        }
    }
    Err("no file picker found (install zenity or kdialog)".into())
}

impl Controller {
    pub(super) async fn composer_cmd(&mut self, cmd: ComposerCmd) {
        match cmd {
            ComposerCmd::HistoryBack(current) => {
                let ring = self.user_ring();
                if let Some(text) = self.composer.browse.back(&current, &ring) {
                    self.set_composer_text(text, true);
                }
            }
            ComposerCmd::HistoryForward => {
                let ring = self.user_ring();
                if let Some(text) = self.composer.browse.forward(&ring) {
                    let browsing = self.composer.browse.browsing();
                    self.set_composer_text(text, browsing);
                }
            }
            ComposerCmd::BeginEdit(row, draft) => self.begin_edit(row, draft),
            ComposerCmd::CancelEdit => {
                self.end_edit();
                let draft = std::mem::take(&mut self.composer.pre_edit_draft);
                self.set_composer_text(draft, false);
                Box::pin(self.drain_queue()).await;
            }
            ComposerCmd::EditSend(row, text) => {
                self.edit_send(row, text).await;
                // Prompts queued during the edit go once it has ended (sent, unchanged or
                // refused) and nothing is running.
                if !self.composer.editing() && !self.transcript.running {
                    Box::pin(self.drain_queue()).await;
                }
            }
            ComposerCmd::PickFiles => {
                let tx = self.commands.clone();
                tokio::task::spawn_blocking(move || match run_picker() {
                    Ok(Some(paths)) if !paths.is_empty() => {
                        let _ = tx.send(Cmd::Composer(ComposerCmd::AttachPaths(paths)));
                    }
                    Ok(_) => {}
                    Err(why) => {
                        tracing::info!("file picker unavailable: {why}");
                        let _ = tx.send(Cmd::Banner(
                            format!("Could not open a file picker ({why}). Paste images with Ctrl+V."),
                            true,
                        ));
                    }
                });
            }
            ComposerCmd::AttachPaths(paths) => self.attach_paths(paths).await,
            ComposerCmd::Resync(sid) => self.resync(sid).await,
        }
    }

    /// The composer text changed (typing, paste, our own history step).
    pub(super) fn composer_text_changed(&mut self, text: &str) {
        let was = self.composer.browse.browsing();
        self.composer.browse.text_changed(text);
        if was && !self.composer.browse.browsing() {
            ui(&self.weak, |w| w.global::<AppState>().set_browsing(false));
        }
    }

    /// A chat was opened or closed: history browsing and editing belong to the old one.
    pub(super) fn composer_reset(&mut self) {
        self.composer.browse.reset();
        self.end_edit();
        ui(&self.weak, |w| w.global::<AppState>().set_browsing(false));
    }

    fn end_edit(&mut self) {
        self.composer.editing = None;
        self.composer.deep_ack = None;
        ui(&self.weak, |w| {
            let st = w.global::<AppState>();
            st.set_editing_row(-1);
            st.set_edit_note("".into());
        });
    }

    /// Images or files are staged: an edit would carry them silently, so it waits.
    fn staged_blocks_edit(&self) -> bool {
        if self.attachments.is_empty() && self.composer.files.is_empty() {
            return false;
        }
        self.banner(
            "Send or remove the staged attachments before editing a message.".into(),
            true,
        );
        true
    }

    fn user_ring(&self) -> Vec<String> {
        composer::history_ring(self.transcript.blocks.iter().filter_map(|b| match b {
            Block::Text {
                role: Role::User, text, ..
            } => Some(text.as_str()),
            _ => None,
        }))
    }

    fn set_composer_text(&self, text: String, browsing: bool) {
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_browsing(browsing);
            st.set_draft(text.into());
            st.set_caret_end_tick(st.get_caret_end_tick().wrapping_add(1));
        });
    }

    fn user_text_at(&self, row: usize) -> Option<&str> {
        match self.transcript.blocks.get(row) {
            Some(Block::Text {
                role: Role::User, text, ..
            }) => Some(text.as_str()),
            _ => None,
        }
    }

    fn begin_edit(&mut self, row: i32, draft: String) {
        if self.transcript.running {
            return self.banner("Stop the current reply before editing a message.".into(), true);
        }
        if self.staged_blocks_edit() {
            return;
        }
        let Some(row) = usize::try_from(row).ok() else { return };
        let Some(orig) = self.user_text_at(row).map(str::to_owned) else {
            return;
        };
        let text = composer::bubble_text(&orig);
        if self.composer.editing.is_none() {
            self.composer.pre_edit_draft = draft;
        }
        self.composer.browse.reset();
        self.composer.deep_ack = None;
        self.composer.editing = Some((row, orig));
        ui(&self.weak, |w| w.global::<AppState>().set_edit_note("".into()));
        let r = row as i32;
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_browsing(false);
            st.set_editing_row(r);
            st.set_draft(text.into());
            st.set_caret_end_tick(st.get_caret_end_tick().wrapping_add(1));
        });
    }

    fn restore_draft(&self, text: String) {
        ui(&self.weak, move |w| w.global::<AppState>().set_draft(text.into()));
    }

    /// Keep the edit open with `text` in the composer and `note` above it.
    fn keep_editing(&self, at: usize, text: String, note: String) {
        let r = at as i32;
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_editing_row(r);
            st.set_edit_note(note.into());
            st.set_draft(text.into());
            st.set_caret_end_tick(st.get_caret_end_tick().wrapping_add(1));
        });
    }

    async fn edit_send(&mut self, row: i32, text: String) {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let Some((at, orig)) = self
            .composer
            .editing
            .clone()
            .filter(|(r, o)| *r == row && self.user_text_at(*r) == Some(o.as_str()))
        else {
            self.end_edit();
            self.banner(
                "The chat changed while you were editing, so nothing was sent. Your text is back in the box.".into(),
                true,
            );
            return self.restore_draft(text);
        };
        let words = text.trim().to_owned();
        if words.is_empty() {
            return self.keep_editing(at, text, "Type the new message, or press Esc to cancel.".into());
        }
        // Unchanged: nothing to replace (Electron's rewind is a no-op here too).
        if words == composer::bubble_text(&orig) {
            self.end_edit();
            let draft = std::mem::take(&mut self.composer.pre_edit_draft);
            return self.set_composer_text(draft, false);
        }
        if self.staged_blocks_edit() {
            return self.keep_editing(at, text, String::new());
        }
        if self.transcript.running {
            self.banner("Stop the current reply before editing a message.".into(), true);
            return self.keep_editing(at, text, String::new());
        }
        let Some(sid) = self.session.clone() else {
            self.end_edit();
            self.banner(
                "This chat is not open on the backend, so nothing was sent.".into(),
                true,
            );
            return self.restore_draft(text);
        };
        let local_users: Vec<String> = self
            .transcript
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text {
                    role: Role::User, text, ..
                } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let ordinal = self.transcript.blocks[..at]
            .iter()
            .filter(|b| matches!(b, Block::Text { role: Role::User, .. }))
            .count();
        let history = self.client.request("session.history", json!({"session_id": sid})).await;
        let messages = history
            .ok()
            .and_then(|v| v.get("messages").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        // The chat may have moved on while the history was read (new chat, another edit,
        // a turn started): only the edit still on screen may be sent.
        if self.composer.editing.as_ref().map(|(r, _)| *r) != Some(at)
            || self.session.as_deref() != Some(sid.as_str())
            || self.transcript.running
            || self.user_text_at(at) != Some(orig.as_str())
        {
            self.end_edit();
            self.banner(
                "The chat changed while the edit was being checked, so nothing was sent. Your text is back in the box."
                    .into(),
                true,
            );
            return self.restore_draft(text);
        }
        let target = match composer::resolve_edit_target(&messages, &local_users, ordinal) {
            Ok(t) => t,
            Err(why) => {
                self.end_edit();
                self.banner(
                    match why {
                        composer::EditRefusal::NotEditable => "That message cannot be edited (it was a command or a steer).",
                        composer::EditRefusal::NotSaved => "That message is not saved yet, so it cannot be edited. Try again in a moment.",
                        composer::EditRefusal::OutOfSync => "The saved chat does not match the screen (it may have changed elsewhere), so nothing was replaced. Reopen the chat and try again.",
                    }
                    .to_owned()
                        + " Your text is back in the box.",
                    true,
                );
                return self.restore_draft(text);
            }
        };
        // Cutting later turns needs explicit consent: the first Enter explains, the second
        // (same edit, same number of later turns) confirms.
        let deep_ok = target.later == 0 || self.composer.deep_ack.as_ref() == Some(&(at, target.later, words.clone()));
        if !deep_ok {
            self.composer.deep_ack = Some((at, target.later, words));
            let n = target.later;
            let note = format!(
                "This also removes the {n} later message{} and {} replies. Press Enter again to send, Esc to cancel.",
                if n == 1 { "" } else { "s" },
                if n == 1 { "its" } else { "their" }
            );
            return self.keep_editing(at, text, note);
        }
        self.end_edit();
        self.composer.pre_edit_draft.clear();
        // The backend cuts at the durable row; show the same cut locally.
        self.transcript.blocks.truncate(at);
        // The replacement carries the turn's files again; show them as the original did.
        self.transcript
            .push_user(&composer::display_user_text(&composer::with_refs(&target.refs, &text)));
        self.mark(hermes_core::transcript::Change::Reset);
        self.flush();
        let params = composer::edit_submit_params(&sid, &text, &target, target.later > 0);
        let (client, commands) = (self.client.clone(), self.commands.clone());
        tokio::spawn(async move {
            if let Err(e) = client
                .request_with_timeout("prompt.submit", params, Duration::from_secs(1800))
                .await
            {
                let _ = commands.send(Cmd::SubmissionFailed {
                    session_id: sid.clone(),
                    text,
                    message: e.to_string(),
                });
                // The cut may not have happened: show what the backend really holds.
                let _ = commands.send(Cmd::Composer(ComposerCmd::Resync(sid)));
            }
        });
    }

    async fn resync(&mut self, sid: String) {
        if self.session.as_deref() != Some(sid.as_str()) {
            return;
        }
        let Ok(v) = self.client.request("session.history", json!({"session_id": sid})).await else {
            return;
        };
        let messages: Vec<TranscriptMessage> = v
            .get("messages")
            .cloned()
            .and_then(|m| serde_json::from_value(m).ok())
            .unwrap_or_default();
        if self.session.as_deref() == Some(sid.as_str()) && !self.transcript.running {
            let ch = self.transcript.load_history(&messages);
            self.mark(ch);
            self.flush();
        }
    }

    async fn attach_paths(&mut self, paths: Vec<String>) {
        let Some(sid) = self.ensure_session().await else { return };
        for path in paths {
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            if composer::is_image_path(&path) {
                let (method, params) = attachments::attach_path_params(&sid, &path);
                match self.client.request(method, params).await {
                    Ok(v) => {
                        let a: AttachedImage = serde_json::from_value(v).unwrap_or_default();
                        if a.attached {
                            self.attachments.push(a);
                        } else {
                            self.banner(
                                format!("{name} was not attached: {}", a.message.unwrap_or_default()),
                                true,
                            );
                        }
                    }
                    Err(e) => self.banner(format!("Could not attach {name}: {e}"), true),
                }
                continue;
            }
            let (method, params) = attachments::file_attach_params(&sid, &path);
            match self.client.request(method, params).await {
                Ok(v) => match v.get("ref_text").and_then(Value::as_str).filter(|r| !r.is_empty()) {
                    Some(r) if v.get("attached").and_then(Value::as_bool) == Some(true) => {
                        self.composer.files.push(StagedFile {
                            label: name,
                            ref_text: r.to_owned(),
                        });
                    }
                    _ => self.banner(format!("{name} was not attached."), true),
                },
                Err(e) => self.banner(format!("Could not attach {name}: {e}"), true),
            }
        }
        self.publish_attachments();
    }
}
