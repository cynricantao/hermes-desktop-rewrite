//! Pure, UI-agnostic transcript state.
//!
//! The UI owns one [`Transcript`] per visible session and feeds it gateway events with
//! [`Transcript::apply`]. Every mutation reports which rows changed ([`Change`]) so the
//! view layer can patch a virtualised list model in place instead of rebuilding it —
//! that is what keeps streaming jank-free on long conversations.
//!
//! Event semantics mirror the Electron renderer
//! (`apps/desktop/src/app/session/hooks/use-message-stream`):
//! * `message.start` opens an assistant turn; `message.delta` appends text;
//!   `message.complete` finalises it (its `text` wins over accumulated deltas).
//! * `reasoning.delta` is model reasoning (shown in the Thinking block).
//! * `thinking.delta` is spinner/status chatter — it only updates the live status line.
//! * `tool.start` / `tool.complete` open/close a tool row keyed by `tool_id`.

use serde_json::Value;

use crate::protocol::{GatewayEvent, TranscriptMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// One-line description: command, path, query …
    pub context: String,
    pub state: ToolState,
    pub summary: String,
    pub output: String,
    pub inline_diff: Option<String>,
    pub duration_s: Option<f64>,
}

impl ToolCall {
    pub fn is_command(&self) -> bool {
        matches!(
            self.name.as_str(),
            "terminal" | "shell" | "bash" | "process" | "execute_code"
        )
    }
    pub fn is_edit(&self) -> bool {
        matches!(self.name.as_str(), "patch" | "write_file" | "edit_file" | "apply_patch")
    }
    /// `(+added, -removed)` counted from a unified diff.
    pub fn diff_counts(&self) -> (usize, usize) {
        let Some(d) = &self.inline_diff else { return (0, 0) };
        let mut a = 0;
        let mut r = 0;
        for l in d.lines() {
            if l.starts_with("+++") || l.starts_with("---") {
                continue;
            }
            if l.starts_with('+') {
                a += 1;
            } else if l.starts_with('-') {
                r += 1;
            }
        }
        (a, r)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text {
        role: Role,
        text: String,
        reasoning: String,
        streaming: bool,
    },
    /// Consecutive tool calls of one assistant turn are grouped into one activity row.
    Tools {
        calls: Vec<ToolCall>,
    },
    Notice {
        text: String,
        error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    None,
    /// Row `i` changed in place.
    Updated(usize),
    /// A row was appended at index `i`.
    Inserted(usize),
    /// Everything changed (history load).
    Reset,
}

/// Tools often return a JSON envelope (`{"output": "...", "exit_code": 0, "error": null}`);
/// show the human part. Anything else is returned unchanged.
pub fn unwrap_tool_output(raw: &str) -> String {
    let t = raw.trim_start();
    if !t.starts_with('{') {
        return raw.to_owned();
    }
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(t) else {
        return raw.to_owned();
    };
    let mut out = String::new();
    for key in ["output", "stdout", "content", "result"] {
        if let Some(Value::String(s)) = m.get(key) {
            out.push_str(s);
            break;
        }
    }
    if let Some(Value::String(e)) = m.get("error").or_else(|| m.get("stderr"))
        && !e.is_empty()
    {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(e);
    }
    if let Some(code) = m.get("exit_code").and_then(Value::as_i64)
        && code != 0
    {
        out.push_str(&format!("\n[exit {code}]"));
    }
    if out.is_empty() { raw.to_owned() } else { out }
}

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub blocks: Vec<Block>,
    /// Live status line ("Thinking…", provider waits). Empty when idle.
    pub status: String,
    pub running: bool,
    pub title: Option<String>,
}

fn text_of(payload: &Value, key: &str) -> String {
    payload.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace content with a stored transcript (`session.resume` / `session.history`).
    pub fn load_history(&mut self, messages: &[TranscriptMessage]) -> Change {
        self.blocks.clear();
        for m in messages {
            if m.display_kind.as_deref() == Some("hidden") {
                continue;
            }
            match m.role.as_str() {
                "user" => self.blocks.push(Block::Text {
                    role: Role::User,
                    // Stored `@file:` refs show as 📎 lines, like a turn sent in this window.
                    text: crate::composer::display_user_text(&m.display_text()),
                    reasoning: String::new(),
                    streaming: false,
                }),
                "assistant" => {
                    let text = m.display_text();
                    let reasoning = m.reasoning.clone().unwrap_or_default();
                    if !text.trim().is_empty() || !reasoning.is_empty() {
                        self.blocks.push(Block::Text {
                            role: Role::Assistant,
                            text,
                            reasoning,
                            streaming: false,
                        });
                    }
                }
                "tool" => {
                    let call = ToolCall {
                        id: m.tool_call_id.clone().unwrap_or_default(),
                        name: m.name.clone().unwrap_or_else(|| "tool".into()),
                        context: m.context.clone().unwrap_or_default(),
                        state: ToolState::Done,
                        summary: String::new(),
                        output: m.display_text(),
                        inline_diff: None,
                        duration_s: None,
                    };
                    self.push_tool(call);
                }
                "system" | "notice" => {
                    let t = m.display_text();
                    if !t.is_empty() {
                        self.blocks.push(Block::Notice { text: t, error: false });
                    }
                }
                _ => {}
            }
        }
        Change::Reset
    }

    /// Optimistically show the user's turn before the backend echoes anything.
    pub fn push_user(&mut self, text: &str) -> Change {
        self.blocks.push(Block::Text {
            role: Role::User,
            text: text.to_owned(),
            reasoning: String::new(),
            streaming: false,
        });
        self.running = true;
        self.status = "Thinking".into();
        Change::Inserted(self.blocks.len() - 1)
    }

    fn push_tool(&mut self, call: ToolCall) -> Change {
        if let Some(Block::Tools { calls }) = self.blocks.last_mut() {
            calls.push(call);
            return Change::Updated(self.blocks.len() - 1);
        }
        self.blocks.push(Block::Tools { calls: vec![call] });
        Change::Inserted(self.blocks.len() - 1)
    }

    /// Index of the open (streaming) assistant block, creating one if needed.
    fn open_assistant(&mut self) -> (usize, bool) {
        if let Some(i) = self.blocks.iter().rposition(|b| {
            matches!(
                b,
                Block::Text {
                    role: Role::Assistant,
                    streaming: true,
                    ..
                }
            )
        }) {
            // Only reuse it if nothing (e.g. a tool row) came after it.
            if i == self.blocks.len() - 1 {
                return (i, false);
            }
            if let Block::Text { streaming, .. } = &mut self.blocks[i] {
                *streaming = false;
            }
        }
        self.blocks.push(Block::Text {
            role: Role::Assistant,
            text: String::new(),
            reasoning: String::new(),
            streaming: true,
        });
        (self.blocks.len() - 1, true)
    }

    pub fn apply(&mut self, ev: &GatewayEvent) -> Change {
        let p = &ev.payload;
        match ev.kind.as_str() {
            "message.start" => {
                self.running = true;
                Change::None
            }
            "message.delta" => {
                let delta = text_of(p, "text");
                if delta.is_empty() {
                    return Change::None;
                }
                let (i, new) = self.open_assistant();
                if let Block::Text { text, .. } = &mut self.blocks[i] {
                    text.push_str(&delta);
                }
                self.status.clear();
                if new { Change::Inserted(i) } else { Change::Updated(i) }
            }
            "reasoning.delta" => {
                let delta = text_of(p, "text");
                if delta.is_empty() {
                    return Change::None;
                }
                let (i, new) = self.open_assistant();
                if let Block::Text { reasoning, .. } = &mut self.blocks[i] {
                    reasoning.push_str(&delta);
                }
                if new { Change::Inserted(i) } else { Change::Updated(i) }
            }
            "thinking.delta" | "status.update" => {
                let t = text_of(p, "text");
                if !t.trim().is_empty() {
                    self.status = t.trim().to_owned();
                }
                Change::None
            }
            "tool.generating" => {
                self.status = format!("Preparing {}", text_of(p, "name"));
                Change::None
            }
            "tool.start" => {
                if let Some(Block::Text { streaming, .. }) = self.blocks.last_mut() {
                    *streaming = false;
                }
                let name = text_of(p, "name");
                let context = p
                    .get("context")
                    .and_then(Value::as_str)
                    .or_else(|| p.get("preview").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned();
                self.status = format!("Running {name}");
                self.push_tool(ToolCall {
                    id: text_of(p, "tool_id"),
                    name,
                    context,
                    state: ToolState::Running,
                    summary: String::new(),
                    output: String::new(),
                    inline_diff: None,
                    duration_s: None,
                })
            }
            "tool.complete" => {
                let id = text_of(p, "tool_id");
                for (bi, b) in self.blocks.iter_mut().enumerate().rev() {
                    if let Block::Tools { calls } = b
                        && let Some(c) = calls.iter_mut().find(|c| c.id == id)
                    {
                        c.state = if p.get("error").is_some_and(|e| !e.is_null()) {
                            ToolState::Failed
                        } else {
                            ToolState::Done
                        };
                        c.summary = text_of(p, "summary");
                        c.output = p
                            .get("result_text")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or_else(|| {
                                p.get("result").map(|r| match r {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                            })
                            .unwrap_or_default();
                        c.output = unwrap_tool_output(&c.output);
                        c.inline_diff = p.get("inline_diff").and_then(Value::as_str).map(str::to_owned);
                        c.duration_s = p.get("duration_s").and_then(Value::as_f64);
                        self.status = "Thinking".into();
                        return Change::Updated(bi);
                    }
                }
                Change::None
            }
            "message.complete" => {
                self.running = false;
                self.status.clear();
                let final_text = p.get("text").and_then(Value::as_str).map(str::to_owned);
                let err = p.get("error").and_then(Value::as_str).map(str::to_owned);
                let reasoning_final = p.get("reasoning").and_then(Value::as_str).map(str::to_owned);
                let mut change = Change::None;
                // Reasoning already streamed into an earlier block of this turn must not be duplicated.
                let turn_start = self
                    .blocks
                    .iter()
                    .rposition(|b| matches!(b, Block::Text { role: Role::User, .. }))
                    .unwrap_or(0);
                let turn_has_reasoning = self.blocks[turn_start..].iter().any(
                    |b| matches!(b, Block::Text { role: Role::Assistant, reasoning, .. } if !reasoning.is_empty()),
                );
                if let Some(i) = self.blocks.iter().rposition(|b| {
                    matches!(
                        b,
                        Block::Text {
                            role: Role::Assistant,
                            streaming: true,
                            ..
                        }
                    )
                }) {
                    if let Block::Text {
                        text,
                        streaming,
                        reasoning,
                        ..
                    } = &mut self.blocks[i]
                    {
                        *streaming = false;
                        if let Some(t) = final_text.filter(|t| !t.is_empty()) {
                            *text = t;
                        }
                        if reasoning.is_empty()
                            && !turn_has_reasoning
                            && let Some(r) = reasoning_final
                        {
                            *reasoning = r;
                        }
                    }
                    change = Change::Updated(i);
                } else if let Some(t) = final_text.filter(|t| !t.trim().is_empty()) {
                    self.blocks.push(Block::Text {
                        role: Role::Assistant,
                        text: t,
                        reasoning: reasoning_final.unwrap_or_default(),
                        streaming: false,
                    });
                    change = Change::Inserted(self.blocks.len() - 1);
                }
                if let Some(e) = err.filter(|e| !e.is_empty()) {
                    self.blocks.push(Block::Notice { text: e, error: true });
                    change = Change::Inserted(self.blocks.len() - 1);
                }
                // Any tool still "running" when the turn ends was interrupted.
                for b in &mut self.blocks {
                    if let Block::Tools { calls } = b {
                        for c in calls.iter_mut().filter(|c| c.state == ToolState::Running) {
                            c.state = ToolState::Failed;
                        }
                    }
                }
                change
            }
            "error" => {
                self.running = false;
                self.status.clear();
                self.blocks.push(Block::Notice {
                    text: text_of(p, "message"),
                    error: true,
                });
                Change::Inserted(self.blocks.len() - 1)
            }
            "notice" => {
                self.blocks.push(Block::Notice {
                    text: text_of(p, "message"),
                    error: false,
                });
                Change::Inserted(self.blocks.len() - 1)
            }
            "session.title" => {
                self.title = Some(text_of(p, "title"));
                Change::None
            }
            "session.info" => {
                if let Some(r) = p.get("running").and_then(Value::as_bool) {
                    self.running = r;
                }
                Change::None
            }
            _ => Change::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_command_payloads_are_not_rendered_on_resume() {
        let messages: Vec<TranscriptMessage> = serde_json::from_value(serde_json::json!([
            {"role":"user", "text":"private expanded skill body", "display_kind":"hidden"},
            {"role":"assistant", "text":"Visible reply"}
        ]))
        .unwrap();
        let mut transcript = Transcript::new();
        transcript.load_history(&messages);
        assert_eq!(transcript.blocks.len(), 1);
        assert!(matches!(&transcript.blocks[0], Block::Text { text, .. } if text == "Visible reply"));
    }
    use serde_json::json;

    fn ev(kind: &str, payload: Value) -> GatewayEvent {
        GatewayEvent {
            kind: kind.into(),
            session_id: Some("s".into()),
            seq: None,
            payload,
        }
    }

    /// Replays the exact event order observed from a live backend (docs/ANALYSIS.md §4).
    #[test]
    fn live_pong_sequence() {
        let mut t = Transcript::new();
        assert_eq!(t.push_user("Reply with exactly: pong"), Change::Inserted(0));
        for e in [
            ev("sessions.changed", json!({})),
            ev("session.info", json!({"running": true})),
            ev("message.start", json!({})),
            ev("session.title", json!({"session_id":"s","title":"Pong"})),
            ev("thinking.delta", json!({"text":"(｡•́︿•̀｡) pondering..."})),
            ev("reasoning.delta", json!({"text":"The user wants "})),
            ev("reasoning.delta", json!({"text":"pong."})),
        ] {
            t.apply(&e);
        }
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.apply(&ev("message.delta", json!({"text":"po"}))), Change::Updated(1));
        t.apply(&ev("message.delta", json!({"text":"ng"})));
        assert_eq!(
            t.apply(&ev("message.complete", json!({"text":"pong"}))),
            Change::Updated(1)
        );
        match &t.blocks[1] {
            Block::Text {
                role,
                text,
                reasoning,
                streaming,
            } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(text, "pong");
                assert_eq!(reasoning, "The user wants pong.");
                assert!(!streaming);
            }
            b => panic!("{b:?}"),
        }
        assert!(!t.running);
        assert_eq!(t.title.as_deref(), Some("Pong"));
    }

    #[test]
    fn tool_calls_group_and_complete() {
        let mut t = Transcript::new();
        t.push_user("ls");
        t.apply(&ev("message.delta", json!({"text":"Let me look."})));
        assert_eq!(
            t.apply(&ev(
                "tool.start",
                json!({"tool_id":"a","name":"terminal","context":"ls -la"})
            )),
            Change::Inserted(2)
        );
        assert_eq!(
            t.apply(&ev(
                "tool.start",
                json!({"tool_id":"b","name":"terminal","context":"pwd"})
            )),
            Change::Updated(2)
        );
        assert_eq!(
            t.apply(&ev(
                "tool.complete",
                json!({"tool_id":"a","name":"terminal","result_text":"x","duration_s":0.2})
            )),
            Change::Updated(2)
        );
        // Text after tools opens a new assistant block.
        assert_eq!(
            t.apply(&ev("message.delta", json!({"text":"Done."}))),
            Change::Inserted(3)
        );
        t.apply(&ev("message.complete", json!({"text":"Done."})));
        let Block::Tools { calls } = &t.blocks[2] else { panic!() };
        assert_eq!(calls[0].state, ToolState::Done);
        assert_eq!(
            calls[1].state,
            ToolState::Failed,
            "unfinished tool is marked interrupted at turn end"
        );
        let Block::Text { text, .. } = &t.blocks[1] else {
            panic!()
        };
        assert_eq!(
            text, "Let me look.",
            "earlier text block is not overwritten by final text"
        );
    }

    #[test]
    fn tool_output_unwrap() {
        assert_eq!(
            unwrap_tool_output(r#"{"output":"hi\nthere","exit_code":0,"error":null}"#),
            "hi\nthere"
        );
        assert_eq!(
            unwrap_tool_output(r#"{"output":"","exit_code":2,"error":"boom"}"#),
            "boom\n[exit 2]"
        );
        assert_eq!(unwrap_tool_output("plain"), "plain");
        assert_eq!(unwrap_tool_output("{bad json"), "{bad json");
    }

    #[test]
    fn reasoning_not_duplicated_after_tools() {
        let mut t = Transcript::new();
        t.push_user("x");
        t.apply(&ev("reasoning.delta", json!({"text":"plan"})));
        t.apply(&ev("tool.start", json!({"tool_id":"a","name":"terminal"})));
        t.apply(&ev("tool.complete", json!({"tool_id":"a"})));
        t.apply(&ev("message.delta", json!({"text":"done"})));
        t.apply(&ev("message.complete", json!({"text":"done","reasoning":"plan"})));
        let Block::Text { reasoning, .. } = t.blocks.last().unwrap() else {
            panic!()
        };
        assert!(reasoning.is_empty());
    }

    #[test]
    fn diff_counts() {
        let c = ToolCall {
            id: "x".into(),
            name: "patch".into(),
            context: String::new(),
            state: ToolState::Done,
            summary: String::new(),
            output: String::new(),
            inline_diff: Some("--- a/f\n+++ b/f\n@@\n-old\n+new\n+more\n ctx".into()),
            duration_s: None,
        };
        assert_eq!(c.diff_counts(), (2, 1));
        assert!(c.is_edit());
    }

    #[test]
    fn history_load() {
        let msgs: Vec<TranscriptMessage> = serde_json::from_value(json!([
            {"role":"user","text":"hi"},
            {"role":"assistant","text":"","reasoning":""},
            {"role":"tool","name":"terminal","context":"ls","text":"a\nb","tool_call_id":"1"},
            {"role":"tool","name":"read_file","text":"…","tool_call_id":"2"},
            {"role":"assistant","text":"hello"}
        ]))
        .unwrap();
        let mut t = Transcript::new();
        assert_eq!(t.load_history(&msgs), Change::Reset);
        assert_eq!(t.blocks.len(), 3, "empty assistant dropped, two tools grouped");
    }

    #[test]
    fn error_completion_adds_notice() {
        let mut t = Transcript::new();
        t.push_user("x");
        t.apply(&ev("message.complete", json!({"text":"","error":"provider 429"})));
        assert!(matches!(t.blocks.last(), Some(Block::Notice { error: true, .. })));
    }
}
