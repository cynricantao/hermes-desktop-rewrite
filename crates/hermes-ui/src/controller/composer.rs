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
    /// Edit the user message at this row (the controller checks it is still one).
    BeginEdit(i32),
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
}

#[derive(Debug, Default)]
pub(super) struct ComposerState {
    browse: Browse,
    /// Row and original text of the message being edited.
    editing: Option<(usize, String)>,
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
    let t = tx.clone();
    st.on_edit_message(move |row, _| {
        let _ = t.send(Cmd::Composer(ComposerCmd::BeginEdit(row)));
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
    // Test/dev hook: newline-separated paths stand in for the picker (no dialog opens).
    if let Ok(paths) = std::env::var("HERMES_DESKTOP_PICKER_PATHS") {
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
            // Exit 1 with no output = Cancel.
            Ok(_) => return Ok(None),
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
            ComposerCmd::BeginEdit(row) => self.begin_edit(row),
            ComposerCmd::CancelEdit => {
                self.composer.editing = None;
                ui(&self.weak, |w| {
                    let st = w.global::<AppState>();
                    st.set_editing_row(-1);
                    st.set_draft("".into());
                });
            }
            ComposerCmd::EditSend(row, text) => self.edit_send(row, text).await,
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
        self.composer.editing = None;
        ui(&self.weak, |w| {
            let st = w.global::<AppState>();
            st.set_browsing(false);
            st.set_editing_row(-1);
        });
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

    fn begin_edit(&mut self, row: i32) {
        if self.transcript.running {
            return self.banner("Stop the current reply before editing a message.".into(), true);
        }
        let Some(row) = usize::try_from(row).ok() else { return };
        let Some(orig) = self.user_text_at(row).map(str::to_owned) else {
            return;
        };
        let text = composer::bubble_text(&orig);
        self.composer.browse.reset();
        self.composer.editing = Some((row, orig));
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

    async fn edit_send(&mut self, row: i32, text: String) {
        let editing = self.composer.editing.take();
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let Some((at, orig)) = editing.filter(|(r, o)| *r == row && self.user_text_at(*r) == Some(o.as_str())) else {
            self.banner(
                "The chat changed while you were editing, so nothing was sent. Your text is back in the box.".into(),
                true,
            );
            return self.restore_draft(text);
        };
        if self.transcript.running {
            self.banner("Stop the current reply before editing a message.".into(), true);
            return self.restore_draft(text);
        }
        let Some(sid) = self.session.clone() else {
            self.banner(
                "This chat is not open on the backend, so nothing was sent.".into(),
                true,
            );
            return self.restore_draft(text);
        };
        let newest = !self.transcript.blocks[at + 1..]
            .iter()
            .any(|b| matches!(b, Block::Text { role: Role::User, .. }));
        let history = self.client.request("session.history", json!({"session_id": sid})).await;
        let messages = history
            .ok()
            .and_then(|v| v.get("messages").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        let Some(target) = composer::resolve_edit_target(&messages, &composer::bubble_text(&orig), newest) else {
            self.banner(
                "Could not find that message in the saved chat, so nothing was replaced. Your text is back in the box."
                    .into(),
                true,
            );
            return self.restore_draft(text);
        };
        // The backend cuts at the durable row; show the same cut locally.
        self.transcript.blocks.truncate(at);
        self.transcript.push_user(&text);
        self.mark(hermes_core::transcript::Change::Reset);
        self.flush();
        let params = composer::edit_submit_params(&sid, &text, &target);
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
