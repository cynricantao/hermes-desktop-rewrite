//! Agent questions raised mid-turn (clarify, sudo, secret, vault unlock): parking per chat,
//! the card's intents, and replies. State rules live in `hermes_core::input_requests`.

use hermes_core::input_requests::{self, InputKind, PendingInput};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Controller, ui};
use crate::{InputChoice, InputState};

/// A value typed into a masked prompt. Its `Debug` never prints the text.
#[derive(Clone)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Input-card intents. Each carries the request id the card showed when it was clicked,
/// so an answer can never land on a request that replaced it in the meantime.
#[derive(Debug, Clone)]
pub enum InputCmd {
    Choose(String, i32),
    Text(String, Secret),
    Confirm(String),
    Skip(String),
    SkipQuestion(String),
}

/// Wire the card's callbacks (UI thread).
pub(super) fn wire(ui_handle: &crate::AppWindow, tx: &tokio::sync::mpsc::UnboundedSender<super::Cmd>) {
    let st = ui_handle.global::<InputState>();
    let send = |tx: &tokio::sync::mpsc::UnboundedSender<super::Cmd>, c| {
        let _ = tx.send(super::Cmd::Input(c));
    };
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_choose(move |i| {
        if let Some(w) = w.upgrade() {
            send(
                &t,
                InputCmd::Choose(w.global::<InputState>().get_request_id().into(), i),
            );
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_submit_text(move |text| {
        if let Some(w) = w.upgrade() {
            let s = w.global::<InputState>();
            if text.trim().is_empty() {
                return;
            }
            // Clear at once: a typed password must not linger in the field.
            s.set_value(SharedString::new());
            send(&t, InputCmd::Text(s.get_request_id().into(), Secret(text.into())));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_confirm(move || {
        if let Some(w) = w.upgrade() {
            send(&t, InputCmd::Confirm(w.global::<InputState>().get_request_id().into()));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_skip(move || {
        if let Some(w) = w.upgrade() {
            let s = w.global::<InputState>();
            s.set_value(SharedString::new());
            send(&t, InputCmd::Skip(s.get_request_id().into()));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_skip_question(move || {
        if let Some(w) = w.upgrade() {
            send(
                &t,
                InputCmd::SkipQuestion(w.global::<InputState>().get_request_id().into()),
            );
        }
    });
}

impl Controller {
    /// Stored identity of a runtime session id (resume can replace the runtime id).
    fn stored_of(&self, runtime: &str) -> String {
        self.runtime_stores
            .get(runtime)
            .cloned()
            .unwrap_or_else(|| runtime.to_owned())
    }

    /// Whether a parked request belongs on the open chat (app-level requests show anywhere).
    fn input_is_here(&self, p: &PendingInput) -> bool {
        p.session_id.is_empty()
            || self.session.as_deref() == Some(p.session_id.as_str())
            || self.stored.as_deref() == Some(self.stored_of(&p.session_id).as_str())
    }

    /// A server request that is an input prompt. Returns false for other methods.
    pub(super) fn input_request(&mut self, id: &str, method: &str, params: &Value) -> bool {
        if !input_requests::handles(method) {
            return false;
        }
        if self.inputs.iter().any(|p| p.id == id) {
            return true;
        }
        let Some(p) = input_requests::parse(id, method, params) else {
            // A clarify with no usable question: answer at once so the tool never stalls.
            self.client.respond(id, Ok(serde_json::json!({})));
            return true;
        };
        if !self.input_is_here(&p) {
            self.banner(
                "Another chat is waiting for your answer. Open it to reply.".into(),
                false,
            );
        }
        self.inputs.push(p);
        self.publish_input();
        true
    }

    /// `request.cancel`: the backend withdrew it (timeout, Stop, answered elsewhere).
    pub(super) fn cancel_input(&mut self, id: &str) {
        let before = self.inputs.len();
        self.inputs.retain(|p| p.id != id);
        if self.inputs.len() != before {
            self.publish_input();
        }
    }

    /// Re-show requests `session.resume` reports as still open.
    pub(super) fn replay_open_requests(&mut self, open: &[Value]) {
        for r in open {
            let (Some(id), Some(method)) = (
                r.get("id").and_then(Value::as_str),
                r.get("method").and_then(Value::as_str),
            ) else {
                continue;
            };
            let params = r.get("params").cloned().unwrap_or(Value::Null);
            if method == "approval" {
                if self.pending_approval.as_deref() != Some(id) {
                    self.server_request(id.to_owned(), method.to_owned(), params);
                }
            } else {
                self.input_request(id, method, &params);
            }
        }
    }

    pub(super) fn input_cmd(&mut self, cmd: InputCmd) {
        let id = match &cmd {
            InputCmd::Choose(id, _)
            | InputCmd::Text(id, _)
            | InputCmd::Confirm(id)
            | InputCmd::Skip(id)
            | InputCmd::SkipQuestion(id) => id.clone(),
        };
        let Some(pos) = self.inputs.iter().position(|p| p.id == id) else {
            return;
        };
        if let InputCmd::Skip(_) = cmd {
            let p = self.inputs.remove(pos);
            self.client.respond(&p.id, Ok(input_requests::skip_response(&p)));
            self.publish_input();
            return;
        }
        let p = &mut self.inputs[pos];
        if p.kind == InputKind::Masked {
            if let InputCmd::Text(_, Secret(value)) = cmd {
                let p = self.inputs.remove(pos);
                self.client.respond(&p.id, Ok(input_requests::value_response(&value)));
                self.publish_input();
            }
            return;
        }
        let changed = match cmd {
            InputCmd::Choose(_, i) => usize::try_from(i).is_ok_and(|i| p.choose(i)),
            InputCmd::Text(_, Secret(text)) => p.answer_text(&text),
            InputCmd::Confirm(_) => p.confirm_multi(),
            InputCmd::SkipQuestion(_) => p.skip_question(),
            InputCmd::Skip(_) => false,
        };
        if p.finished() {
            let p = self.inputs.remove(pos);
            self.client.respond(&p.id, Ok(p.clarify_response()));
        }
        if changed {
            self.publish_input();
        }
    }

    /// Render the first request that belongs on the open chat, or hide the card.
    pub(super) fn publish_input(&self) {
        let shown = self.inputs.iter().find(|p| self.input_is_here(p)).cloned();
        ui(&self.weak, move |w| {
            let s = w.global::<InputState>();
            let Some(p) = shown else {
                s.set_visible(false);
                s.set_request_id(SharedString::new());
                s.set_value(SharedString::new());
                return;
            };
            if s.get_request_id().as_str() != p.id {
                s.set_value(SharedString::new());
            }
            let q = p.current();
            let choices: Vec<InputChoice> = q
                .map(|q| {
                    q.choices
                        .iter()
                        .enumerate()
                        .map(|(i, c)| InputChoice {
                            label: input_requests::bare_choice(c).into(),
                            recommended: input_requests::is_recommended(c),
                            selected: p.selected.get(i).copied().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default();
            s.set_masked(p.kind == InputKind::Masked);
            s.set_request_id(p.id.as_str().into());
            s.set_title(p.title.as_str().into());
            s.set_detail(p.detail.as_str().into());
            s.set_question(q.map(|q| q.question.as_str()).unwrap_or_default().into());
            s.set_step(p.step_label().into());
            s.set_multi(q.is_some_and(|q| q.multi));
            s.set_batch(p.questions.len() > 1);
            s.set_choices(ModelRc::new(VecModel::from(choices)));
            s.set_visible(true);
        });
    }
}
