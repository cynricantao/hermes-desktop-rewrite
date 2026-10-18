//! Blocking questions the agent asks the user through server→client requests: `clarify`
//! (1–5 questions with optional choices), and the single masked-value prompts `sudo`,
//! `display.install.sudo`, `secret` and `vault.unlock_prompt`.
//!
//! Wire contract (`tui_gateway/server.py::_clarify_block`, `_ask`, `server_requests.py`):
//! * clarify params `{session_id, questions: [{qid, question, choices|null, multi_select}],
//!   answers?}`; `answers` only rides on a reconnect replay (answers already locked).
//!   The reply is `{answers: {qid: text | JSON-list text | null}}`; `{}` skips the batch.
//! * the masked prompts reply `{value: text}`; an empty value means "skipped".
//!
//! Pure state only: the controller owns delivery and the UI only renders [`PendingInput`].

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The backend appends this to the agent's recommended first choice
/// (`tools/clarify_tool.py::mark_recommended`).
pub const RECOMMENDED: &str = "(Recommended)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    Clarify,
    /// A masked single value: sudo password, secret/API key, vault master password.
    Masked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub qid: String,
    pub question: String,
    /// Empty = open-ended (free text only).
    pub choices: Vec<String>,
    pub multi: bool,
}

/// One open request, plus the user's progress through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInput {
    /// Server request id (`srq-…`), answered with `Client::respond`.
    pub id: String,
    pub method: String,
    /// Runtime session id that raised it; empty for app-level prompts.
    pub session_id: String,
    pub kind: InputKind,
    pub title: String,
    /// Extra context (masked prompts: the command or variable).
    pub detail: String,
    pub questions: Vec<Question>,
    /// Answers so far: `None` = skipped question.
    pub answers: BTreeMap<String, Option<String>>,
    /// Multi-select toggles for the current question.
    pub selected: Vec<bool>,
}

/// Choice text without the backend's recommendation label.
pub fn bare_choice(choice: &str) -> &str {
    let t = choice.trim();
    match t.len().checked_sub(RECOMMENDED.len()) {
        Some(cut) if t.is_char_boundary(cut) && t[cut..].eq_ignore_ascii_case(RECOMMENDED) => t[..cut].trim_end(),
        _ => t,
    }
}

pub fn is_recommended(choice: &str) -> bool {
    bare_choice(choice).len() != choice.trim().len()
}

fn s(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn parse_questions(params: &Value) -> Vec<Question> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let qid = s(q, "qid");
            let question = s(q, "question");
            if qid.is_empty() || question.is_empty() {
                return None;
            }
            let choices: Vec<String> = q
                .get("choices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map(str::to_owned)
                .collect();
            let multi = q.get("multi_select").and_then(Value::as_bool) == Some(true) && !choices.is_empty();
            Some(Question {
                qid,
                question,
                choices,
                multi,
            })
        })
        .collect()
}

/// Methods this client answers with an input card.
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        "clarify" | "sudo" | "display.install.sudo" | "secret" | "vault.unlock_prompt"
    )
}

/// Parse one server request. `None` when the method is not an input prompt or a clarify
/// carries no usable question (the caller answers `{}` so the tool never stalls).
pub fn parse(id: &str, method: &str, params: &Value) -> Option<PendingInput> {
    let session_id = s(params, "session_id");
    let base = |kind, title: String, detail: String| PendingInput {
        id: id.to_owned(),
        method: method.to_owned(),
        session_id: session_id.clone(),
        kind,
        title,
        detail,
        questions: Vec::new(),
        answers: BTreeMap::new(),
        selected: Vec::new(),
    };
    let mut p = match method {
        "clarify" => {
            let questions = parse_questions(params);
            if questions.is_empty() {
                return None;
            }
            let mut p = base(InputKind::Clarify, "Hermes has a question".into(), String::new());
            // Reconnect replay: answers the server already locked stay answered.
            if let Some(locked) = params.get("answers").and_then(Value::as_object) {
                for (qid, v) in locked {
                    if questions.iter().any(|q| &q.qid == qid) {
                        p.answers.insert(qid.clone(), v.as_str().map(str::to_owned));
                    }
                }
            }
            p.questions = questions;
            p
        }
        "sudo" => base(InputKind::Masked, "Administrator password needed".into(), {
            let cmd = s(params, "command");
            if cmd.is_empty() {
                "A command needs sudo.".into()
            } else {
                cmd
            }
        }),
        "display.install.sudo" => base(
            InputKind::Masked,
            "Administrator password needed".into(),
            "Installing display packages needs sudo.".into(),
        ),
        "secret" => {
            let prompt = s(params, "prompt");
            let var = s(params, "env_var");
            let title = if prompt.is_empty() {
                "A secret value is needed".into()
            } else {
                prompt
            };
            base(
                InputKind::Masked,
                title,
                if var.is_empty() {
                    String::new()
                } else {
                    format!("Saved as {var}")
                },
            )
        }
        "vault.unlock_prompt" => {
            let name = s(params, "display_name");
            let backend = s(params, "backend");
            base(
                InputKind::Masked,
                format!("Unlock {}", if name.is_empty() { &backend } else { &name }),
                "Master password for the password manager.".into(),
            )
        }
        _ => return None,
    };
    p.reset_selection();
    Some(p)
}

impl PendingInput {
    /// Index of the first unanswered question, `None` when every one has an answer.
    pub fn current_index(&self) -> Option<usize> {
        self.questions.iter().position(|q| !self.answers.contains_key(&q.qid))
    }

    pub fn current(&self) -> Option<&Question> {
        self.current_index().map(|i| &self.questions[i])
    }

    /// "2 of 3" for batches, empty for a single question.
    pub fn step_label(&self) -> String {
        match (self.current_index(), self.questions.len()) {
            (Some(i), n) if n > 1 => format!("{} of {n}", i + 1),
            _ => String::new(),
        }
    }

    fn reset_selection(&mut self) {
        self.selected = vec![false; self.current().map_or(0, |q| q.choices.len())];
    }

    fn record(&mut self, answer: Option<String>) -> bool {
        let Some(qid) = self.current().map(|q| q.qid.clone()) else {
            return false;
        };
        self.answers.insert(qid, answer);
        self.reset_selection();
        true
    }

    /// Click on choice `i`: answers a single-select question, toggles a multi-select one.
    pub fn choose(&mut self, i: usize) -> bool {
        let Some(q) = self.current().cloned() else { return false };
        let Some(choice) = q.choices.get(i) else { return false };
        if q.multi {
            if let Some(v) = self.selected.get_mut(i) {
                *v = !*v;
            }
            return true;
        }
        self.record(Some(bare_choice(choice).to_owned()))
    }

    /// Confirm the multi-select toggles (nothing toggled = no answer yet).
    pub fn confirm_multi(&mut self) -> bool {
        let Some(q) = self.current().cloned() else { return false };
        let picked: Vec<&str> = q
            .choices
            .iter()
            .zip(&self.selected)
            .filter(|(_, on)| **on)
            .map(|(c, _)| bare_choice(c))
            .collect();
        if !q.multi || picked.is_empty() {
            return false;
        }
        self.record(Some(json!(picked).to_string()))
    }

    /// Typed answer ("Other", or an open-ended question). Text naming a choice (any case)
    /// becomes that choice, as the Electron card does. Blank text is ignored.
    pub fn answer_text(&mut self, text: &str) -> bool {
        let text = text.trim();
        let Some(q) = self.current().cloned() else { return false };
        if text.is_empty() {
            return false;
        }
        let answer = q
            .choices
            .iter()
            .map(|c| bare_choice(c))
            .find(|c| c.eq_ignore_ascii_case(text))
            .unwrap_or(text)
            .to_owned();
        self.record(Some(if q.multi { json!([answer]).to_string() } else { answer }))
    }

    /// Skip just the current question of a batch.
    pub fn skip_question(&mut self) -> bool {
        self.record(None)
    }

    pub fn finished(&self) -> bool {
        self.kind == InputKind::Clarify && self.current_index().is_none()
    }

    /// Reply for a finished clarify batch.
    pub fn clarify_response(&self) -> Value {
        json!({ "answers": self.answers })
    }
}

/// Reply for a masked prompt (`""` = skipped).
pub fn value_response(value: &str) -> Value {
    json!({ "value": value })
}

/// Reply that declines the request without an answer.
pub fn skip_response(p: &PendingInput) -> Value {
    match p.kind {
        InputKind::Clarify => json!({}),
        InputKind::Masked => value_response(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clarify() -> PendingInput {
        parse(
            "srq-1",
            "clarify",
            &json!({"session_id": "s1", "questions": [
                {"qid": "q0", "question": "Scope?", "choices": ["Small (Recommended)", "Large"], "multi_select": false},
                {"qid": "q1", "question": "Which?", "choices": ["A", "B", "C"], "multi_select": true},
                {"qid": "q2", "question": "Anything else?", "choices": null},
            ]}),
        )
        .unwrap()
    }

    #[test]
    fn recommended_label_is_stripped() {
        assert_eq!(bare_choice("Small (Recommended)"), "Small");
        assert_eq!(bare_choice("Small (recommended) "), "Small");
        assert_eq!(bare_choice("Large"), "Large");
        assert!(is_recommended("Small (Recommended)"));
        assert!(!is_recommended("Large"));
        // Multi-byte text right before the label must not panic.
        assert_eq!(bare_choice("é"), "é");
    }

    #[test]
    fn batch_walks_questions_and_builds_reply() {
        let mut p = clarify();
        assert_eq!(p.session_id, "s1");
        assert_eq!(p.step_label(), "1 of 3");
        assert!(p.choose(0));
        assert_eq!(p.step_label(), "2 of 3");
        // Multi-select: toggles, then confirm; confirm with nothing picked is refused.
        assert!(!p.confirm_multi());
        assert!(p.choose(0) && p.choose(2) && p.choose(0));
        assert_eq!(p.selected, vec![false, false, true]);
        assert!(p.choose(1));
        assert!(p.confirm_multi());
        assert!(!p.answer_text("   "));
        assert!(p.answer_text(" nope "));
        assert!(p.finished());
        assert_eq!(
            p.clarify_response(),
            json!({"answers": {"q0": "Small", "q1": "[\"B\",\"C\"]", "q2": "nope"}})
        );
    }

    #[test]
    fn typed_text_matches_a_choice_and_skips_are_null() {
        let mut p = clarify();
        assert!(p.answer_text("large"));
        assert!(p.answer_text("b"));
        assert!(p.skip_question());
        assert_eq!(
            p.clarify_response(),
            json!({"answers": {"q0": "Large", "q1": "[\"B\"]", "q2": null}})
        );
    }

    #[test]
    fn replayed_locks_resume_at_the_first_open_question() {
        let p = parse(
            "srq-2",
            "clarify",
            &json!({"session_id": "s1", "answers": {"q0": "Large", "zz": "ignored"}, "questions": [
                {"qid": "q0", "question": "Scope?", "choices": ["Small", "Large"]},
                {"qid": "q1", "question": "Why?"},
            ]}),
        )
        .unwrap();
        assert_eq!(p.current().unwrap().qid, "q1");
        assert_eq!(p.answers.len(), 1);
    }

    #[test]
    fn unusable_clarify_and_unknown_methods_are_rejected() {
        assert!(parse("x", "clarify", &json!({"questions": [{"qid": "", "question": "?"}]})).is_none());
        assert!(parse("x", "tour", &json!({})).is_none());
        assert!(!handles("tour"));
        assert!(handles("secret"));
    }

    #[test]
    fn masked_prompts_parse_and_reply_with_value() {
        let p = parse("srq-3", "sudo", &json!({"session_id": "s2", "command": "pacman -S x"})).unwrap();
        assert_eq!((p.kind.clone(), p.detail.as_str()), (InputKind::Masked, "pacman -S x"));
        assert_eq!(skip_response(&p), json!({"value": ""}));
        let p = parse(
            "srq-4",
            "secret",
            &json!({"prompt": "OpenAI key", "env_var": "OPENAI_API_KEY"}),
        )
        .unwrap();
        assert_eq!(p.session_id, "");
        assert_eq!(p.detail, "Saved as OPENAI_API_KEY");
        assert_eq!(value_response("hunter2"), json!({"value": "hunter2"}));
        let c = clarify();
        assert_eq!(skip_response(&c), json!({}));
        assert!(!c.finished());
    }
}
