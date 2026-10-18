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

/// Which card and question an intent was made on: the request id and the question id the
/// card showed when it was clicked (empty for masked prompts). A double click or a stale
/// press is dropped instead of answering the next question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub request: String,
    pub question: String,
}

/// Input-card intents.
#[derive(Debug, Clone)]
pub enum InputCmd {
    Choose(Shown, i32),
    Text(Shown, Secret),
    Confirm(Shown),
    Skip(Shown),
    SkipQuestion(Shown),
}

fn shown(s: &InputState) -> Shown {
    Shown {
        request: s.get_request_id().into(),
        question: s.get_question_id().into(),
    }
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
            send(&t, InputCmd::Choose(shown(&w.global::<InputState>()), i));
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
            send(&t, InputCmd::Text(shown(&s), Secret(text.into())));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_confirm(move || {
        if let Some(w) = w.upgrade() {
            send(&t, InputCmd::Confirm(shown(&w.global::<InputState>())));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_skip(move || {
        if let Some(w) = w.upgrade() {
            let s = w.global::<InputState>();
            s.set_value(SharedString::new());
            send(&t, InputCmd::Skip(shown(&s)));
        }
    });
    let (t, w) = (tx.clone(), ui_handle.as_weak());
    st.on_skip_question(move || {
        if let Some(w) = w.upgrade() {
            send(&t, InputCmd::SkipQuestion(shown(&w.global::<InputState>())));
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
        let parsed = input_requests::parse(id, method, params);
        if let Some(existing) = self.inputs.iter_mut().find(|p| p.id == id) {
            if let Some(replay) = &parsed {
                existing.adopt_locks(replay);
                self.publish_input();
            }
            return true;
        }
        let Some(p) = parsed else {
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
        let at = match &cmd {
            InputCmd::Choose(s, _)
            | InputCmd::Text(s, _)
            | InputCmd::Confirm(s)
            | InputCmd::Skip(s)
            | InputCmd::SkipQuestion(s) => s.clone(),
        };
        let Some(pos) = self.inputs.iter().position(|p| p.id == at.request) else {
            return;
        };
        // Per-question intents must target the question still on screen.
        let current = self.inputs[pos].current().map(|q| q.qid.clone()).unwrap_or_default();
        if !matches!(cmd, InputCmd::Skip(_)) && current != at.question {
            return;
        }
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
                s.set_question_id(SharedString::new());
                s.set_value(SharedString::new());
                return;
            };
            let q = p.current();
            let qid = q.map(|q| q.qid.as_str()).unwrap_or_default();
            // A new request or a new question starts with an empty field.
            if s.get_request_id().as_str() != p.id || s.get_question_id().as_str() != qid {
                s.set_value(SharedString::new());
            }
            s.set_question_id(qid.into());
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
